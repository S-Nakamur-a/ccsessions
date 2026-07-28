//! ccsessionsd — Claude Code のセッションを生き物として常時可視化する macOS 常駐オーバーレイ。
//!
//! `ccsessions hook` が書いたセッションファイルをポーリングし、1 セッション = 1 匹の
//! 生き物としてメニューバー上端（bar）または画面下部のドック風パネル（dock）に描く。
//!
//! # 設定はこのプロセスが持たない
//! 配置もデザインも、**設定の入口は Web UI（`ccsessions ui` / `make config`）だけ**。
//! かつてはメニューバーに status item（🐾）を出して、そこから配置・顔・動き・
//! 記号を切り替えていたが、入口が 2 つあると設定を 1 つ足すたびに AppKit の
//! メニュー（tag 分岐・チェックの貼り直し・顔ごとのプレビュー画像）と Web の
//! 両方を直すことになる。しかもメニューは開かないと見えず、画面収録権限が無い
//! 環境では目視で確かめられない。
//!
//! そこで daemon は**設定を読むだけ**にした。`config.toml` の mtime を poller が
//! 見ているので、UI が書けば数百 ms で反映される（`AppEvent::ConfigChanged` の
//! 1 本道）。daemon 自身が設定を書くのは dock をドラッグして位置が決まったときだけ。
//!
//! # ウィンドウが 2 枚ある理由
//! - **群れ窓**: 生き物を描く。ホバーを取るためマウスイベントを受けるので、
//!   帯の大きさぴったりにして周囲のメニューバー操作を奪わない。
//! - **カード窓**: ホバー時の詳細パネル。帯の外へはみ出すため別窓にし、
//!   クリック透過にして下の操作を一切邪魔しない。
//!
//! # 省電力
//! 生き物のアニメは CoreAnimation がレンダーサーバ側で回す。メインスレッドは
//! ポーリング間隔（既定 500ms）ごとに短く起きるだけで、毎フレーム描画はしない。

mod anim;
mod card;
mod creature;
mod ffi;
mod flock;
mod geometry;
mod layout;
mod screen;
mod text;
mod theme;
mod window;

use std::sync::Arc;
use std::time::Duration;

use objc2::rc::Retained;
use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};
use objc2_foundation::MainThreadMarker;
use tao::dpi::LogicalSize;
use tao::event::Event;
use tao::event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy};
use tao::platform::macos::{ActivationPolicy, EventLoopExtMacOS};
use tao::window::{Window, WindowBuilder};

use ccsessions_core::config::{self, BarAlign, Config, Placement};
use ccsessions_core::face::{FaceSpec, Registry};
use ccsessions_core::session::{DeadReason, Session};
use ccsessions_core::store;

use crate::flock::Flock;
use crate::geometry::{Rect, ScreenMetrics};
use crate::layout::{bar_fit, BarFit, Packing};
use crate::theme::Size;
use crate::window::HoverView;

/// セッションファイルと設定ファイルを見に行く間隔。
///
/// hook はイベントの瞬間にファイルを書くので、可視化の遅れはこの間隔がほぼそのまま。
/// 500ms なら「Enter を押したら生き物が動き出す」体感になり、かつ 116 プロジェクトぶんの
/// ディレクトリ走査でも負荷は無視できる。
const POLL_INTERVAL: Duration = Duration::from_millis(500);

/// 死んだセッションのファイルを掃除する間隔（ポーリング何回ごとか）。
///
/// 表示から外すのは `store::list_live` が毎回やるので、こちらは**ディスク上の
/// 後始末**が目的。急ぐ必要はないが、常駐している唯一のプロセスがここなので
/// 誰かが定期的にやらないとファイルが溜まり続ける（実際、`sweep` が 1 度も
/// 呼ばれていなかったために数か月ぶんのセッションファイルが残っていた）。
/// 120 回 × 500ms ＝ 60 秒。
const SWEEP_EVERY_TICKS: u32 = 120;

/// 経過時間の表示を進めるために、変化が無くてもセッションを送り直す間隔
/// （ポーリング何回ごとか）。20 回 × 500ms ＝ 10 秒。
const RESEND_EVERY_TICKS: u32 = 20;

/// メインループへ他スレッド・AppKit から流す内部イベント。
#[derive(Debug, Clone)]
pub enum AppEvent {
    /// poller がセッション集合の変化（または経過時間の更新契機）を検知した。
    ///
    /// 並びは `sort_for_display` 済み。`Arc` で渡すのは、変化のたびに
    /// セッション一覧を丸ごと複製しないため（poller は同じものを
    /// 「前回値」として保持し続ける）。
    Sessions(Arc<[Session]>),
    /// config.toml が書き換わった（外部エディタ・CLI 経由の変更を拾う）。
    ConfigChanged(Box<Config>),
    /// 群れ窓の上でマウスが動いた／出た。座標は窓ローカル（左下原点）。
    Hover(Option<(f64, f64)>),
    /// dock をドラッグして動かしている。値は**パネルの中心 x と下端 y**
    /// （画面座標）で、まだクランプされていない「提案」。
    DockDragged { x: f64, y: f64 },
    /// ドラッグが終わった。ここで初めて設定ファイルへ書く
    /// （押しっぱなしの移動中に毎回書くと、その回数だけ config の mtime が動き、
    /// poller が自分の書き込みを `ConfigChanged` として拾い返す）。
    DockDragEnded,
    /// ディスプレイ構成が変わった（外部モニタの抜き差し等）。
    ScreenChanged,
    /// `faces/` ディレクトリが書き換わった（ユーザ顔の追加・編集・削除）。
    FacesChanged(Box<Registry>),
}

fn main() {
    let demo = std::env::args().any(|a| a == "--demo");

    let mut event_loop = EventLoopBuilder::<AppEvent>::with_user_event().build();
    let proxy = event_loop.create_proxy();
    let mtm = MainThreadMarker::new().expect("main() runs on the main thread");

    // 常駐オーバーレイとしての振る舞いを tao のライフサイクルに乗せて設定する。
    // **`run()` より前でないと効かない。**
    //
    // - `Accessory` … Dock アイコンを出さない。
    // - `set_dock_visibility(false)` … 同上（tao 側の経路）。
    // - **`set_activate_ignoring_other_apps(false)` が最重要**。tao の既定は「起動時に
    //   他アプリを無視して自分をアクティブにする」で、これを切らないと
    //   **ログイン時や再起動のたびに、作業中のアプリからフォーカスを奪う**。
    //   常時表示の受動的なオーバーレイとしては致命的。
    //   （併せて窓は `with_visible(false)` で作り、後から `orderFront` する。tao は
    //   `applicationDidFinishLaunching` で「その時点で可視な窓」に
    //   `makeKeyAndOrderFront` を掛け直すため。`window::set_visible` のコメント参照）
    event_loop.set_activation_policy(ActivationPolicy::Accessory);
    event_loop.set_dock_visibility(false);
    event_loop.set_activate_ignoring_other_apps(false);
    // tao 経由の設定は `run()` 時に適用されるが、窓を作る前から accessory で
    // ある必要があるので、ここでも直接指定しておく（冪等）。
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);

    let mut overlay = Overlay::new(&event_loop, &proxy, mtm);
    let _screen_observer = screen::observe(proxy.clone(), mtm);

    // poller はプロセスが終わるまで回り続ける（この daemon には「終了」の入口が無く、
    // 止めるのは `make stop` / `launchctl bootout` の仕事）。
    {
        let proxy = proxy.clone();
        let cfg = overlay.cfg.clone();
        std::thread::spawn(move || poll_loop(proxy, cfg, demo));
    }

    event_loop.run(move |event, _target, control_flow| {
        *control_flow = ControlFlow::Wait;
        if let Event::UserEvent(e) = event {
            overlay.handle(e);
        }
    });
}

// ---------------------------------------------------------------------------
// Overlay — イベントループが持つ可変状態一式
// ---------------------------------------------------------------------------

/// ホバー中の生き物。
///
/// **索引と id を必ず一組で持つ。** 並びは安定させてあるが（`sort_for_display`）、
/// セッションが増減すれば索引はずれる。索引だけで追うと、カードが隣のセッションの
/// 内容を出したまま残る。
struct Hovered {
    index: usize,
    session_id: String,
}

/// 群れ窓・カード窓と、そこに映っている状態。
///
/// イベントループの実体はここのメソッド。`handle` の各分岐が短いのは、
/// 「環境が変わったら組み直して置き直す」という共通の手順を
/// [`Overlay::rebuild_and_reposition`] に畳んであるため。
struct Overlay {
    /// 生き物を描く窓。ホバーを取るのでマウスイベントを受ける。
    creatures_window: Window,
    /// ホバーカードを出すクリック透過の窓。
    card_window: Window,
    /// 群れ窓に重ねた、マウスの出入りとドラッグを拾うビュー。
    hover_view: Retained<HoverView>,

    flock: Flock,
    /// 顔レジストリ。組込み + ユーザ顔（`~/.config/ccsessions/faces/*.toml`）。
    faces: Registry,
    face: Arc<FaceSpec>,
    cfg: Config,
    packing: Packing,

    sessions: Arc<[Session]>,
    hovered: Option<Hovered>,
    card: Option<card::Card>,
    /// 群れ窓にいま適用してある矩形。`None` は「窓を隠してある」。
    ///
    /// 同じ値を投げ直さないための記憶なので、**環境が変わったら捨てて必ず貼り直す**
    /// （[`Overlay::rebuild_and_reposition`]）。ディスプレイ構成が変わったときは
    /// AppKit 側が窓を動かしている可能性があり、こちらの記憶が実際とずれる。
    placed: Option<Rect>,

    mtm: MainThreadMarker,
}

impl Overlay {
    fn new(
        event_loop: &tao::event_loop::EventLoop<AppEvent>,
        proxy: &EventLoopProxy<AppEvent>,
        mtm: MainThreadMarker,
    ) -> Self {
        let cfg = load_config();
        let faces = load_faces();
        let face = faces.resolve(&cfg.design);

        let creatures_window = WindowBuilder::new()
            .with_title("ccsessionsd")
            .with_inner_size(LogicalSize::new(1.0, 1.0))
            .with_decorations(false)
            .with_always_on_top(true)
            .with_transparent(true)
            .with_visible(false)
            // **キーウィンドウになれなくする**。tao の `TaoWindow` は
            // `canBecomeKeyWindow` / `canBecomeMainWindow` をこのフラグで上書きする。
            // ドラッグのためにクリックを受けるようになった以上、これが無いと
            // 「掴んだ瞬間に作業中のエディタからフォーカスを奪う」ことになりかねない。
            // **クリック自体はこのフラグを立てても届く**（`mouseDown:` は来る）ので、
            // ドラッグ機能とは両立する。
            .with_focusable(false)
            .build(event_loop)
            .expect("failed to build the creatures window");
        window::configure(&creatures_window);
        let hover_view = window::attach_hover(&creatures_window, proxy.clone(), mtm);
        hover_view.set_draggable(cfg.placement == Placement::Dock);

        let card_window = WindowBuilder::new()
            .with_title("ccsessionsd-card")
            .with_inner_size(LogicalSize::new(1.0, 1.0))
            .with_decorations(false)
            .with_always_on_top(true)
            .with_transparent(true)
            .with_visible(false)
            // カード窓はクリック透過なので本来キーにはならないが、群れ窓と揃えて
            // 明示的に落としておく（「この daemon の窓はどれもキーにならない」を
            // 窓ごとの偶然でなく宣言で保証する）。
            .with_focusable(false)
            .build(event_loop)
            .expect("failed to build the card window");
        window::configure(&card_window);
        card_window
            .set_ignore_cursor_events(true)
            .expect("the card window must be click-through");

        let packing = current_packing(&cfg, &face, screen::metrics(cfg.placement, mtm));
        let flock = Flock::new(
            Arc::clone(&face),
            size_of(&cfg),
            packing,
            screen::backing_scale(mtm),
        );
        if let Some(backing) = window::backing_layer(&creatures_window) {
            backing.addSublayer(flock.layer());
        }

        Overlay {
            creatures_window,
            card_window,
            hover_view,
            flock,
            faces,
            face,
            cfg,
            packing,
            sessions: Arc::from(Vec::new()),
            hovered: None,
            card: None,
            placed: None,
            mtm,
        }
    }

    fn handle(&mut self, event: AppEvent) {
        match event {
            AppEvent::Sessions(list) => self.on_sessions(list),
            AppEvent::ConfigChanged(cfg) => self.on_config_changed(*cfg),
            AppEvent::Hover(pos) => self.on_hover(pos),
            AppEvent::DockDragged { x, y } => self.on_dock_dragged(x, y),
            // 保存は**離した瞬間の 1 回だけ**。
            AppEvent::DockDragEnded => persist(&self.cfg),
            AppEvent::ScreenChanged => self.rebuild_and_reposition(),
            AppEvent::FacesChanged(faces) => {
                self.faces = *faces;
                self.faces.report_problems();
                self.rebuild_and_reposition();
            }
        }
    }

    fn on_sessions(&mut self, list: Arc<[Session]>) {
        // ホバー中の生き物が「別のセッション」に化けていないかを id で確かめる
        // （セッションが減って索引が範囲外になった場合もここで拾う）。
        let hovered_moved = self
            .hovered
            .as_ref()
            .is_some_and(|h| list.get(h.index).map(|s| &s.id) != Some(&h.session_id));

        self.sessions = list;
        // **詰め方をここで取り直す。** bar の「使える幅」はメニューエクストラの
        // 実測に依存するようになったが、エクストラの増減は `ScreenChanged` を
        // 起こさないので、`refit` を呼ぶ 3 つの入口（設定・画面・顔）をどれも
        // 通らない。取り直さないと縮小率が古いまま残り、**縮めれば右に収まるのに
        // 縮まずノッチ左へ逃げる**（アプリメニューと重なりうる方＝
        // `docs/adr/0012` と `0013` が避けたかった側）。
        //
        // `repack` は `needs_reconfigure`（`Packing` の比較）で守られているので、
        // 幅が変わっていないときはレイヤを作り直さない ＝ アニメ位相は保たれる。
        // 実測はここで 1 回だけ取り、下の `reposition_with` にも渡す。
        let metrics = screen::metrics(self.cfg.placement, self.mtm);
        let repacked = self.repack(metrics);

        let relaid = self.refresh_creatures() || repacked;
        if relaid || hovered_moved {
            self.hide_card();
        }
        // 群れが空になると `reposition` が窓ごと隠す。その先へは `mouseUp` も
        // `mouseExited` も届かないので、掴んでいる状態とホバー状態を
        // **AppKit のイベントに頼らずここで畳む**。放置すると、カーソルが
        // 握り拳で固まり、次にセッションが立ったときパネルが濃いまま作られる。
        if self.flock.is_empty() {
            self.hover_view.cancel_drag();
            self.flock.set_hovered(false);
        }
        self.reposition_with(metrics);
        if relaid {
            window::refresh_tracking(&self.hover_view);
        }
    }

    fn on_config_changed(&mut self, mut new_cfg: Config) {
        // **掴んでいる最中は位置だけ in-memory を勝たせる。**
        //
        // ドラッグ終了時の `persist` は config の mtime を動かすので、その ~500ms 後に
        // poller が**自分の書き込み**を `ConfigChanged` として返してくる。その間に
        // 次のドラッグを始めていると、ここでファイル側の（1 つ前の）位置に
        // 巻き戻り、パネルが目に見えて跳ね、そのまま離すと今のドラッグが
        // 丸ごと失われる（微調整で 2 回続けて掴むと普通に踏む）。
        //
        // 位置以外は素直に取り込む — 外部エディタでの設定変更を無視しないため。
        if self.hover_view.is_dragging() {
            new_cfg.dock_x = self.cfg.dock_x;
            new_cfg.dock_y = self.cfg.dock_y;
        }
        self.cfg = new_cfg;
        self.hover_view
            .set_draggable(self.cfg.placement == Placement::Dock);
        self.rebuild_and_reposition();
    }

    fn on_hover(&mut self, pos: Option<(f64, f64)>) {
        // 背景の濃さは「カーソルが帯の上にあるか」だけで決まり、どの生き物の
        // 上かは関係しない。**当たり判定による早期 return より前**に反映する
        // （同じ生き物の上で動いている間も、入った瞬間の 1 回で足りる）。
        self.flock.set_hovered(pos.is_some());
        let next = pos.and_then(|(x, y)| self.flock.hit_test(x, y));
        if next == self.hovered.as_ref().map(|h| h.index) {
            return;
        }

        // カードを組むあいだ `self` を可変に借りたいので、一覧の参照だけ先に取り出す
        // （`Arc` なので複製ではなく参照カウントが 1 増えるだけ）。
        let sessions = Arc::clone(&self.sessions);
        let target = next.and_then(|i| sessions.get(i).map(|s| (i, s)));
        log_hover(next, target.map(|(_, s)| s.name.as_str()));
        let Some((index, session)) = target else {
            self.hide_card();
            return;
        };

        let now = ccsessions_core::now_ms();
        let card =
            self.flock
                .build_card(session, now, self.cfg.done_ttl_ms(), self.cfg.reduce_motion);
        self.show_card(index, card);
        self.hovered = Some(Hovered {
            index,
            session_id: session.id.clone(),
        });
    }

    fn on_dock_dragged(&mut self, x: f64, y: f64) {
        // bar 配置では動かさない。ビュー側でも止めているが、配置切替と
        // マウスイベントが交差したときの保険（帯はメニューバー上の位置に
        // 意味があるので、絶対に動かしてはいけない）。
        if self.cfg.placement != Placement::Dock {
            return;
        }
        self.cfg.dock_x = Some(x);
        self.cfg.dock_y = Some(y);
        // 掴んで動かしている最中に詳細カードが出ていると邪魔なので畳む。
        // 出ていないなら何もしない（ドラッグ中はマウスが動くたびここへ来るので、
        // 隠し済みの窓へ毎回 `orderOut:` を投げない）。
        if self.card.is_some() {
            self.hide_card();
        }
        // 窓を動かすのはここだけ。クランプもこの中で掛かる。
        self.reposition();
    }

    /// 環境（設定・画面・顔）が変わったときの立て直し。
    ///
    /// 3 つの入口で手順がまったく同じなので 1 か所にまとめてある。分けて書くと、
    /// 手順を 1 つ足したときに片方だけ直して静かにずれる。
    fn rebuild_and_reposition(&mut self) {
        self.refit();
        self.refresh_creatures();
        self.hide_card();
        // 窓の位置は記憶に頼らず貼り直す（`placed` の doc 参照）。
        self.placed = None;
        self.reposition();
        window::refresh_tracking(&self.hover_view);
    }

    /// 顔・詰め方を今の設定と画面から解き直し、**変わったときだけ**群れを組み直す。
    ///
    /// 毎回無条件に `reconfigure` するとレイヤが作り直されてアニメの位相がリセットされ、
    /// 群れ全体が不自然に同期して揺れる（`creature.rs` の設計原則）。
    ///
    /// **何が変わったかをここで判定しない**のが要点。判定は `needs_reconfigure` の
    /// 1 か所に任せる — 呼ぶ側にも条件を持つと、設定を足したときに片方へ書き忘れて
    /// 静かに古いレイアウトのままになる（`bar_align` と `compact_flock` はどちらも
    /// 使える幅を変えるので、実際に取りこぼしていた）。
    fn refit(&mut self) {
        self.face = self.faces.resolve(&self.cfg.design);
        self.repack(screen::metrics(self.cfg.placement, self.mtm));
    }

    /// 詰め方だけを取り直す。**顔は解決し直さない。** 戻り値は詰め方が変わったか。
    ///
    /// **`Registry::resolve` をここに入れてはいけない。** 未知の顔 id に対して
    /// 毎回 stderr へ警告を出す（dedup していない）。ここは `on_sessions` から
    /// **最短 500ms ごと**に呼ばれるので、`design` に存在しない顔を書いた設定
    /// （書式さえ正しければ `config::load` は通す — それが意図）だと警告が
    /// 毎秒 2 行流れ、ローテーションの無い `ccsessionsd.err`（常駐なら
    /// `~/Library/Logs/ccsessions/`）を 1 日で
    /// 17 万行にする。顔が変わりうる経路（`ConfigChanged` / `FacesChanged`）は
    /// どちらも `refit` を通るので、ホットパスで解決し直す必要が無い。
    ///
    /// `metrics` を引数で受けるのは、同じ tick で `reposition` と二重に測らない
    /// ため（bar かつノッチありでは 1 回 ~600µs の FFI）。計測を配置で絞った
    /// 理由（`docs/adr/0012-notch-avoidance.md`）と同じ倹約をここでも通す。
    fn repack(&mut self, metrics: Option<ScreenMetrics>) -> bool {
        let before = self.packing;
        self.packing = current_packing(&self.cfg, &self.face, metrics);
        let scale = screen::backing_scale(self.mtm);
        let size = size_of(&self.cfg);
        if self
            .flock
            .needs_reconfigure(&self.face, size, self.packing, scale)
        {
            self.flock
                .reconfigure(Arc::clone(&self.face), size, self.packing, scale);
        }
        self.packing != before
    }

    /// 戻り値は「レイアウトが組み直された（＝窓の大きさが変わりうる）」かどうか。
    fn refresh_creatures(&mut self) -> bool {
        self.flock.update(
            &self.sessions,
            ccsessions_core::now_ms(),
            self.cfg.done_ttl_ms(),
            self.cfg.reduce_motion,
            self.cfg.show_glyphs,
        )
    }

    /// 群れ窓を現在のレイアウトと配置設定に合わせて置き直す。
    ///
    /// **矩形が変わったときだけ AppKit を叩く。** ここはセッションの更新ごと
    /// （変化が無くても 10 秒に 1 回）とドラッグ中のマウス移動ごとに呼ばれるが、
    /// 定常状態では毎回まったく同じ矩形になる。`setFrame_display:` と `orderFront:`
    /// はどちらもウィンドウサーバとの往復なので、同じ値の投げ直しはそのまま無駄。
    fn reposition(&mut self) {
        self.reposition_with(screen::metrics(self.cfg.placement, self.mtm));
    }

    /// 実測値を受け取る版。**同じ tick で `repack` と二重に測らない**ために分けてある
    /// （bar かつノッチありでは 1 回 ~600µs の FFI。詳細は `repack` の doc）。
    fn reposition_with(&mut self, metrics: Option<ScreenMetrics>) {
        let Some(metrics) = metrics else {
            return;
        };
        // 画面環境（特にノッチ機のジオメトリ）は機種ごとに違い、目視できない環境
        // （リモート・スクリーン録画権限なし）でも配置の妥当性を確かめられるように、
        // 実測値と決定した矩形をログに残す。
        log_geometry(&metrics, &self.cfg);
        // セッションが 0 のときは窓ごと隠す。透明でも矩形ぶんのクリックは奪うので、
        // 中身が無い窓を出しっぱなしにしない。
        if self.flock.is_empty() {
            if self.placed.is_some() {
                window::set_visible(&self.creatures_window, false);
                self.placed = None;
            }
            return;
        }
        let (cw, ch) = self.flock.window_size();
        log_squeeze(self.flock.squeeze(), cw);
        warn_if_centred_under_the_notch(&metrics, &self.cfg, cw);
        let r: Rect = match self.cfg.placement {
            Placement::Bar => {
                geometry::bar_rect(&metrics, cw, ch.max(theme::BAR_BAND_H), self.cfg.bar_align)
            }
            // ドラッグで決めた位置があればそれを使う。**窓を動かすのはここだけ**なので、
            // ドラッグ中も含めて位置の真実は設定側（`cfg.dock_x`/`dock_y`）に一本化される
            // （AppKit に窓を動かさせると、この関数が 500ms〜10 秒後に必ず上書きする）。
            Placement::Dock => geometry::dock_rect(
                &metrics,
                cw,
                ch,
                theme::DOCK_BOTTOM_MARGIN,
                self.cfg.dock_x,
                self.cfg.dock_y,
            ),
        };
        if self.placed == Some(r) {
            return;
        }
        log_window(r, &self.cfg, cw, ch);
        window::set_frame(&self.creatures_window, r);
        window::set_visible(&self.creatures_window, true);
        self.placed = Some(r);
    }

    fn show_card(&mut self, index: usize, c: card::Card) {
        let Some((lx, ly)) = self.flock.card_origin(index, c.width, c.height) else {
            return;
        };
        // 群れ窓のローカル座標をグローバルへ持ち上げる。
        let f = window::ns_window(&self.creatures_window).frame();
        let base = Rect::new(f.origin.x, f.origin.y, f.size.width, f.size.height);
        let want = Rect::new(base.x + lx, base.y + ly, c.width, c.height);
        // **画面内へ収める。** dock が下部中央に固定だったころは不要だったが、ドラッグで
        // どこへでも置けるようになった以上、パネルを上端や左右端へ寄せるとカードが
        // 画面外へ出て読めなくなる。
        let r = match screen::metrics(self.cfg.placement, self.mtm) {
            Some(m) => geometry::fit_card_on_screen(&m, want, base, theme::CARD_PANEL_GAP),
            None => want,
        };

        if let Some(prev) = self.card.take() {
            prev.layer.removeFromSuperlayer();
        }
        if let Some(backing) = window::backing_layer(&self.card_window) {
            // カードのレイヤは窓ローカルの (0,0) に置き、位置は窓側で持つ。
            c.layer.setFrame(ffi::rect(0.0, 0.0, c.width, c.height));
            backing.addSublayer(&c.layer);
        }
        // 画面収録権限が無い環境ではカードを目視できないので、実際の矩形を出す
        // （タイトル行のぶん高さが伸びているか・幅が上限で止まっているかを
        // ここの数値で確かめる）。
        eprintln!(
            "ccsessionsd: card #{index} size={:.0}x{:.0} at {:.0},{:.0}",
            c.width, c.height, r.x, r.y
        );
        window::set_frame(&self.card_window, r);
        window::set_visible(&self.card_window, true);
        self.card = Some(c);
    }

    fn hide_card(&mut self) {
        if let Some(c) = self.card.take() {
            c.layer.removeFromSuperlayer();
        }
        window::set_visible(&self.card_window, false);
        self.hovered = None;
    }
}

// ---------------------------------------------------------------------------
// 設定・画面から組み方を決める
// ---------------------------------------------------------------------------

/// 生き物を並べる順を**安定**にする。
///
/// `store::list_live` は「直近に動いたセッション」を選ぶために `updated` の降順で返す。
/// 選抜の基準としてはそれが正しいが、**表示順にそのまま使うと生き物が飛び回る** —
/// `updated` は hook が飛ぶたびに変わるので、動いたセッションが毎回先頭へ移動し、
/// 隣の生き物と場所を入れ替えてしまう。常時視界に入るものとして、これは落ち着かない。
///
/// そこで選抜（recency）と表示順（stability）を分け、ここではプロジェクト名 → id の
/// 辞書順に並べ直す。名前が同じプロジェクトが複数あっても id で決定的に並ぶので、
/// 同じセッション集合なら常に同じ配置になる。
fn sort_for_display(list: &mut [Session]) {
    list.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.id.cmp(&b.id)));
}

/// いまの画面から群れの組み方（縦の詰め方 ＋ 使える幅）を決める。
///
/// `screen::metrics` が読めないときは使える幅を `None` にする — そこから
/// 「幅が広いから縮めなくてよい」と決め打つより、`layout::squeeze` の匹数による
/// 判断へ落とす方が安全（`bar_fit` が `FALLBACK_MENU_BAR_H` へ倒すのと同じ考え方）。
fn current_packing(cfg: &Config, face: &FaceSpec, metrics: Option<ScreenMetrics>) -> Packing {
    let (fit, avail_w) = match cfg.placement {
        Placement::Bar => {
            let menu_bar_h = metrics.as_ref().map_or(0.0, |m| m.menu_bar_height());
            let (_, body_h) = face.body_size(Size::Bar);
            let fit = bar_fit(menu_bar_h, body_h);
            // 目視できない環境でも詰め方が追えるようにログへ出す（値が変わったときだけ）。
            log_bar_fit(menu_bar_h, body_h, fit);
            let w = metrics
                .as_ref()
                .map(|m| geometry::bar_available_width(m, cfg.bar_align));
            (fit, w)
        }
        // dock ではメニューバー高の制約が無い（`bar_fit` は bar 専用）。
        Placement::Dock => {
            let w = metrics
                .as_ref()
                .map(|m| geometry::dock_available_width(m, theme::DOCK_PAD_X));
            (BarFit::ROOMY, w)
        }
    };
    Packing {
        fit,
        avail_w,
        compact: cfg.compact_flock,
    }
}

/// 顔レジストリを読む。壊れた顔は stderr に列挙してその顔だけ無視する。
fn load_faces() -> Registry {
    let reg = Registry::load_in(&ccsessions_core::faces_dir());
    reg.report_problems();
    reg
}

fn size_of(cfg: &Config) -> Size {
    match cfg.placement {
        Placement::Bar => Size::Bar,
        Placement::Dock => Size::Dock,
    }
}

fn load_config() -> Config {
    match config::load(&ccsessions_core::config_path()) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("ccsessionsd: config.toml error — {e}; using built-in defaults");
            config::builtin_default()
        }
    }
}

/// 設定をファイルへ書き戻す。失敗しても致命的ではない（メモリ上の設定では動き続ける）。
fn persist(cfg: &Config) {
    let path = ccsessions_core::config_path();
    if let Err(e) = config::save(&path, cfg) {
        eprintln!("ccsessionsd: could not save config — {e}");
    }
}

// ---------------------------------------------------------------------------
// ログ
// ---------------------------------------------------------------------------
//
// 画面収録権限が無い環境では見た目を目視で確かめられないので、配置・詰め方・
// 当たり判定は stderr の数値で追う（`/tmp/ccsessionsd-dev.log`）。

/// **前回と違う行のときだけ** stderr に出す。
///
/// `reposition` はセッションの更新ごと（変化が無くても 10 秒に 1 回）に呼ばれ、
/// ドラッグ中はマウスが動くたびに呼ばれる。毎回出すと同じ行でログが埋まり、
/// 肝心の変化が読めなくなる。
///
/// 呼び出しごとに専用の記憶域を持たせたいのでマクロにしてある（関数だと
/// `static` を呼び出し側で宣言することになり、それが本体になってしまう）。
macro_rules! log_when_changed {
    ($($arg:tt)*) => {{
        static LAST: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);
        let line = format!($($arg)*);
        let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
        if last.as_deref() != Some(line.as_str()) {
            eprintln!("{line}");
            *last = Some(line);
        }
    }};
}

fn log_bar_fit(menu_bar_h: f64, body_h: f64, fit: BarFit) {
    log_when_changed!(
        "ccsessionsd: bar_fit menu_bar={menu_bar_h:.1} body_h={body_h:.1} \
         -> headroom={:.1} glyph_inside={} body_scale={:.3} anim_scale={:.3}",
        fit.headroom,
        fit.glyph_inside,
        fit.body_scale,
        fit.anim_scale
    );
}

fn log_window(r: Rect, cfg: &Config, cw: f64, ch: f64) {
    log_when_changed!(
        "ccsessionsd: window {:?} placement={} content={:.1}x{:.1}",
        r,
        cfg.placement.as_str(),
        cw,
        ch
    );
}

fn log_squeeze(sq: layout::Squeeze, content_w: f64) {
    log_when_changed!(
        "ccsessionsd: squeeze scale={:.3} compact={} content_w={content_w:.1}",
        sq.scale,
        sq.compact
    );
}

fn log_geometry(m: &geometry::ScreenMetrics, cfg: &Config) {
    // `menu_extra_left` と、それを基に群れが使える幅を出す。画面を目視できない環境
    // （リモート・スクリーン録画権限なし）では、この 2 つの数値が「実測が効いているか」
    // 「その結果いくら使えると判断したか」を確かめる唯一の手段になる。
    //
    // **使える幅は配置ごとに別の関数で決まる**ので、ここでも配置で分ける。bar 用の
    // 値を dock でも出すと、dock を追うときにログが嘘をつく（このログは目視の
    // 代わりなので、嘘は目視できない環境では訂正されない）。
    let avail_w = match cfg.placement {
        Placement::Bar => geometry::bar_available_width(m, cfg.bar_align),
        Placement::Dock => geometry::dock_available_width(m, theme::DOCK_PAD_X),
    };
    log_when_changed!(
        "ccsessionsd: screen frame={:?} visible={:?} safe_top={:.1} menu_bar={:.1} \
         aux_l={:?} aux_r={:?} notch={:?} menu_extra_left={:?} align={} avail_w={:.1}",
        m.frame,
        m.visible,
        m.safe_area_top,
        m.menu_bar_height(),
        m.aux_top_left,
        m.aux_top_right,
        m.notch_x_range(),
        m.menu_extra_left,
        cfg.bar_align.as_str(),
        avail_w,
    );
}

/// `bar_align = "center"` をノッチ機で選ぶと群れがノッチの下に隠れるので警告する。
///
/// **`Auto` は自動で退避するが、明示指定は素通しする**（ユーザの指定を勝手に
/// 覆さない、という既存の判断）。その結果「設定は効いているのに何も見えない」
/// 状態になり得るので、黙って隠れる代わりに理由を stderr に残す。
/// `ccsessions doctor` は objc2 に依存しないので測れず、設定だけを見た静的ヒントを
/// 出す。ここは実測（ノッチの有無と群れの幅）で判定できるので、そちらを補う。
///
/// 隠れなくなったときは何も言わないので `log_when_changed!` は使えない
/// （同じ警告文が消えたことを記憶する必要がある）。
fn warn_if_centred_under_the_notch(m: &geometry::ScreenMetrics, cfg: &Config, content_w: f64) {
    use std::sync::Mutex;
    static LAST: Mutex<Option<bool>> = Mutex::new(None);
    let hidden = cfg.placement == Placement::Bar
        && cfg.bar_align == BarAlign::Center
        && geometry::center_hits_notch(m, content_w);
    let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
    if *last == Some(hidden) {
        return;
    }
    *last = Some(hidden);
    if hidden {
        eprintln!(
            "ccsessionsd: 警告 bar_align=center で群れ（幅 {content_w:.1}）がノッチの下に隠れます。\
             bar_align=auto にすればノッチを避けます。"
        );
    }
}

/// ホバー対象が変わったことをログに出す。
///
/// マウス移動そのものは毎ピクセル飛んでくるが、ここは**対象が変わったときだけ**
/// 呼ばれるので出力は疎。画面を目視できない環境で当たり判定が効いているかを
/// 確かめる唯一の手段なので、常時出す。
fn log_hover(index: Option<usize>, name: Option<&str>) {
    match (index, name) {
        (Some(i), Some(n)) => eprintln!("ccsessionsd: hover -> #{i} ({n})"),
        (Some(i), None) => eprintln!("ccsessionsd: hover -> #{i}"),
        (None, _) => eprintln!("ccsessionsd: hover -> none"),
    }
}

// ---------------------------------------------------------------------------
// poller
// ---------------------------------------------------------------------------

/// セッションと設定を見張る背景スレッド。
///
/// 変化が無くても一定間隔でセッションを送る理由: 経過時間の表示（`1h15m`）は
/// 時間だけで変わるので、ファイルが動かなくても再描画の契機が要る。`creature::apply`
/// は表示内容が同じなら何もしないので、無駄打ちにはならない。
///
/// 並べ替えもここで済ませる。イベントループのスレッドでやる理由が無い。
fn poll_loop(proxy: EventLoopProxy<AppEvent>, mut cfg: Config, demo: bool) {
    let config_path = ccsessions_core::config_path();
    let faces_dir = ccsessions_core::faces_dir();
    let mut last_config_mtime = mtime(&config_path);
    // ユーザ顔の追加・編集・削除を拾う。ディレクトリの mtime はファイルの
    // 追加・削除では動くが**中身の編集では動かない**ので、中のファイルの
    // mtime も合わせて見る。
    let mut last_faces_stamp = faces_stamp(&faces_dir);
    let mut last: Option<Arc<[Session]>> = None;
    let mut ticks: u32 = 0;

    // 起動時に 1 度掃除する。daemon が止まっていたあいだに死んだセッションを
    // 引きずらないため（前回の終了が異常だった場合、その掃除もここで済む）。
    if !demo {
        sweep_dead_sessions(&cfg);
    }

    loop {
        std::thread::sleep(POLL_INTERVAL);

        // 設定ファイルの live reload。
        let m = mtime(&config_path);
        if m != last_config_mtime {
            last_config_mtime = m;
            match config::load(&config_path) {
                Ok(c) => {
                    cfg = c.clone();
                    let _ = proxy.send_event(AppEvent::ConfigChanged(Box::new(c)));
                }
                Err(e) => {
                    eprintln!("ccsessionsd: config.toml error — {e}; keeping previous config")
                }
            }
        }

        // 顔ディレクトリの live reload。
        let fs_stamp = faces_stamp(&faces_dir);
        if fs_stamp != last_faces_stamp {
            last_faces_stamp = fs_stamp;
            let reg = ccsessions_core::face::Registry::load_in(&faces_dir);
            let _ = proxy.send_event(AppEvent::FacesChanged(Box::new(reg)));
        }

        ticks += 1;
        if !demo && ticks.is_multiple_of(SWEEP_EVERY_TICKS) {
            sweep_dead_sessions(&cfg);
        }

        let now = ccsessions_core::now_ms();
        let mut list = if demo {
            flock::demo_sessions(now)
        } else {
            store::list_live(now, cfg.session_ttl_ms(), cfg.max_sessions)
        };
        sort_for_display(&mut list);
        let list: Arc<[Session]> = Arc::from(list);

        let changed = last.as_deref() != Some(&*list);
        if changed || ticks.is_multiple_of(RESEND_EVERY_TICKS) {
            last = Some(Arc::clone(&list));
            let _ = proxy.send_event(AppEvent::Sessions(list));
        }
    }
}

/// 死んだセッションのファイルを消し、**消したものを必ずログに残す**。
///
/// 生きているセッションを誤って消してしまったときに、ここのログだけが原因
/// （TTL 超過か、持ち主のプロセス消失か）を切り分ける手掛かりになる。
/// 掃除は稀にしか起きないので、1 件 1 行で出しても出力は疎のまま。
fn sweep_dead_sessions(cfg: &Config) {
    for (s, reason) in store::sweep(ccsessions_core::now_ms(), cfg.session_ttl_ms()) {
        let why = match reason {
            DeadReason::Expired => format!("{}秒 無更新", cfg.session_ttl_ms() / 1000),
            DeadReason::ProcessGone(pid) => format!("pid {pid} が居ない"),
        };
        eprintln!(
            "ccsessionsd: reaped session {} ({}) — {}",
            s.name, s.id, why
        );
    }
}

fn mtime(p: &std::path::Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(p).ok()?.modified().ok()
}

/// 顔ディレクトリの「変わったか」を表す指紋。
///
/// ディレクトリ自身の mtime だけだと**ファイルの中身を書き換えたときに動かない**
/// （追加・削除では動く）。投稿者は同じファイルを編集しながら見た目を追い込むので、
/// 中のファイルの mtime も畳み込む。エントリ数も入れて削除を確実に拾う。
fn faces_stamp(dir: &std::path::Path) -> Option<(usize, std::time::SystemTime)> {
    let entries = std::fs::read_dir(dir).ok()?;
    let mut count = 0usize;
    let mut newest: Option<std::time::SystemTime> = None;
    for e in entries.flatten() {
        let p = e.path();
        if p.extension().is_none_or(|x| x != "toml") {
            continue;
        }
        count += 1;
        if let Some(m) = mtime(&p) {
            newest = Some(newest.map_or(m, |n: std::time::SystemTime| n.max(m)));
        }
    }
    Some((count, newest.unwrap_or(std::time::UNIX_EPOCH)))
}
