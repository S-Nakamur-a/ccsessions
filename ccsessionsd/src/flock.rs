//! 群れ — 複数の生き物とホバーカードをまとめて面倒を見る層。
//!
//! 責務:
//! - セッション数が変わったらレイヤツリーを組み直し、変わらなければ `apply` だけする
//! - ウィンドウ座標 → 何匹目か の当たり判定
//! - ホバーカードを置くべき位置を返す（カード自体は別ウィンドウなので `main` が置く）
//!
//! **なぜ「数が変わったときだけ作り直す」か**: 生き物のスロット位置はレイアウト計算に
//! 依存するため、数が変わると全員の位置がずれる。逆に数が同じなら位置は不変なので、
//! 色と文字とアニメだけ差し替えれば足りる（アニメの位相が保たれ、群れが自然に見える）。
//!
//! **なぜカードを別ウィンドウにするか**: bar 配置ではカードがメニューバーより下へ
//! はみ出す。カードぶんまで群れのウィンドウを広げると、その矩形がメニューバー下の
//! クリックを奪ってしまう。カードはクリック透過の別窓に出して、群れの窓は帯の
//! 大きさぴったりに保つ。

use objc2::rc::Retained;
use objc2_quartz_core::{CALayer, CAShapeLayer};

use std::sync::Arc;

use ccsessions_core::face::FaceSpec;
use ccsessions_core::lang::Lang;
use ccsessions_core::session::{Session, SessionState};

use crate::card::{self, AgentRow, CardView};
use crate::creature::{Creature, View};
use crate::ffi::rect;
use crate::layout::{self, Layout, Packing, Slot, Squeeze};
use crate::theme::{self, Size};

pub struct Flock {
    /// ウィンドウの contentView のバッキングレイヤに差す親。
    root: Retained<CALayer>,
    /// dock 配置の背景パネル（bar では使わない）。
    ///
    /// 濃さを差し替えるので `CAShapeLayer` のまま持つ（`CALayer` へ潰すと
    /// `setFillColor` が呼べず、毎回 downcast する羽目になる）。
    panel: Option<Retained<CAShapeLayer>>,
    /// カーソルが dock の上にあるか。**背景の濃さだけを決める**状態で、
    /// どの生き物の上かは見ない（`hit_test` とは別物）。
    hovered: bool,
    /// 生き物を載せるコンテナ（dock ではパネルの内側にオフセットされる）。
    stage: Retained<CALayer>,
    creatures: Vec<Creature>,
    layout: Layout,

    /// 群れ全員が使う顔。
    face: Arc<FaceSpec>,
    size: Size,
    /// 組み方（メニューバー高への適応 ＋ 使える幅とコンパクト表示の方針）。
    ///
    /// **縮小率そのものではなく方針を持つ**のが要点。縮小率は匹数に依存するので、
    /// `rebuild` のたびに解き直す（`BarFit` のように 1 つの値では持てない）。
    packing: Packing,
    scale: f64,
}

impl Flock {
    /// 空の群れを作る。`root` を呼び出し側が contentView のレイヤに差す。
    pub fn new(face: Arc<FaceSpec>, size: Size, packing: Packing, scale: f64) -> Self {
        let root = CALayer::new();
        let stage = CALayer::new();
        root.addSublayer(&stage);
        let layout = layout::lay_out(0, &face, size, packing.fit, Squeeze::NONE);
        Flock {
            root,
            panel: None,
            hovered: false,
            stage,
            creatures: Vec::new(),
            layout,
            face,
            size,
            packing,
            scale,
        }
    }

    pub fn layer(&self) -> &CALayer {
        &self.root
    }

    /// 表示するセッションが 1 つも無いか。窓を隠すかどうかの判断に使う。
    pub fn is_empty(&self) -> bool {
        self.creatures.is_empty()
    }

    /// いま適用されている縮小。目視できない環境でログに残すために公開する。
    pub fn squeeze(&self) -> Squeeze {
        self.layout.squeeze
    }

    /// いまの状態に対応する背景の不透明度。
    fn panel_alpha(&self) -> f64 {
        if self.hovered {
            theme::DOCK_PANEL_ALPHA_HOVER
        } else {
            theme::DOCK_PANEL_ALPHA_IDLE
        }
    }

    /// カーソルが dock の上にあるかを反映する。**変わったときだけ**フェードする。
    ///
    /// 毎回のマウス移動で呼ばれるので、同じ値なら何もしないことが要点
    /// （さもないとフェードが毎フレーム貼り直されて、いつまでも収束しない）。
    /// bar 配置ではパネルが無いので状態だけ持って何もしない。
    pub fn set_hovered(&mut self, hovered: bool) {
        if self.hovered == hovered {
            return;
        }
        self.hovered = hovered;
        if let Some(p) = &self.panel {
            card::set_dock_panel_alpha(p, self.panel_alpha(), theme::DOCK_PANEL_FADE_SECS);
        }
    }

    /// 現在のレイアウトが要求するウィンドウの内容サイズ。
    /// dock では背景パネルのぶんだけ大きくなる。
    pub fn window_size(&self) -> (f64, f64) {
        if self.size.is_bar() {
            (self.layout.width, self.layout.height)
        } else {
            layout::dock_panel(&self.layout)
        }
    }

    /// 表示するセッションを反映する。数が変わればレイヤを組み直す。
    /// 戻り値は「レイアウトが組み直された（＝窓の大きさが変わりうる）」かどうか。
    pub fn update(
        &mut self,
        sessions: &[Session],
        now: u64,
        done_ttl_ms: u64,
        reduce_motion: bool,
        show_glyphs: bool,
    ) -> bool {
        let relaid = sessions.len() != self.creatures.len();
        if relaid {
            self.rebuild(sessions.len());
        }
        for (c, s) in self.creatures.iter().zip(sessions) {
            c.apply(&view_of(s, now, done_ttl_ms, show_glyphs), reduce_motion);
        }
        relaid
    }

    /// 生き物のレイヤツリーを人数ぶん組み直す。
    ///
    /// **縮小率はここで解く**。使える幅に収まるかどうかは匹数で変わるので、
    /// 匹数が変わるこの瞬間が唯一の解き直しどころになる。
    fn rebuild(&mut self, count: usize) {
        for c in self.creatures.drain(..) {
            c.layer().removeFromSuperlayer();
        }
        let sq = self.packing.squeeze_for(count, &self.face, self.size);
        self.layout = layout::lay_out(count, &self.face, self.size, self.packing.fit, sq);

        // dock は背景パネルを敷き、その内側に群れを置く。
        if let Some(p) = self.panel.take() {
            p.removeFromSuperlayer();
        }
        let (ox, oy) = if self.size.is_bar() {
            (0.0, 0.0)
        } else {
            let (pw, ph) = layout::dock_panel(&self.layout);
            // **いまの濃さで作る**。既定値で作ると、匹数が変わってレイヤを組み直す
            // たびに背景が既定へ戻ってしまう（カーソルを乗せたままセッションが
            // 増減すると点滅して見える）。
            let panel = card::dock_panel(pw, ph, self.panel_alpha());
            self.root.insertSublayer_atIndex(&panel, 0);
            self.panel = Some(panel);
            (theme::DOCK_PAD_X, theme::DOCK_PAD_BOTTOM)
        };
        self.stage
            .setFrame(rect(ox, oy, self.layout.width, self.layout.height));

        for slot in &self.layout.slots {
            let c = Creature::build(
                *slot,
                Arc::clone(&self.face),
                self.size,
                self.packing.fit,
                self.scale,
                sq,
            );
            self.stage.addSublayer(c.layer());
            self.creatures.push(c);
        }
    }

    /// 配置・顔・メニューバー高が変わったので全部組み直す。
    ///
    /// **変化が無いときは呼ばないこと**。レイヤを作り直すとアニメの位相がリセットされ、
    /// 群れ全体が不自然に同期して揺れる（`creature.rs` の設計原則）。
    /// 変化の有無は `needs_reconfigure` で判定する。
    pub fn reconfigure(&mut self, face: Arc<FaceSpec>, size: Size, packing: Packing, scale: f64) {
        self.face = face;
        self.size = size;
        self.packing = packing;
        self.scale = scale;
        let n = self.creatures.len();
        self.rebuild(n);
    }

    /// 組み直しが要るか（顔・配置・詰め方・使える幅・Retina 倍率のどれかが変わったか）。
    pub fn needs_reconfigure(
        &self,
        face: &FaceSpec,
        size: Size,
        packing: Packing,
        scale: f64,
    ) -> bool {
        self.size != size
            || self.packing != packing
            || self.scale != scale
            // 顔は id と色で見る。**色を id と別に見るのが要点** — ビルダーで色だけ
            // 直して保存し直すと id は同じままなので、id しか見ないと変更が画面に出ない
            // （形を直したときは寸法が変わって別経路で組み直されるが、色は何も動かさない）。
            || self.face.id != face.id
            || self.face.colors != face.colors
    }

    /// ウィンドウ座標（左下原点）から何匹目かを引く。
    ///
    /// `stage` のオフセットを引いてからスロットの当たり矩形と比べる。
    pub fn hit_test(&self, x: f64, y: f64) -> Option<usize> {
        let f = self.stage.frame();
        let (lx, ly) = (x - f.origin.x, y - f.origin.y);
        self.layout.slots.iter().position(|s| {
            lx >= s.hit_x && lx < s.hit_x + s.hit_w && ly >= s.hit_y && ly < s.hit_y + s.hit_h
        })
    }

    /// `i` 匹目のスロットを返す（カードの位置決めに使う）。
    pub fn slot(&self, i: usize) -> Option<Slot> {
        self.layout.slots.get(i).copied()
    }

    /// 生き物を載せているコンテナの、ウィンドウ内でのオフセット。
    pub fn stage_origin(&self) -> (f64, f64) {
        let f = self.stage.frame();
        (f.origin.x, f.origin.y)
    }

    /// ホバーカードのレイヤを組む。置き場所（別ウィンドウ）は `main` が決める。
    pub fn build_card(
        &self,
        session: &Session,
        now: u64,
        done_ttl_ms: u64,
        reduce_motion: bool,
        lang: Lang,
    ) -> card::Card {
        card::build(
            &card_view_of(session, now, done_ttl_ms, lang),
            self.size,
            self.scale,
            reduce_motion,
            &self.face,
        )
    }

    /// `i` 匹目のカードを置くべき位置を、**群れウィンドウのローカル座標**で返す。
    /// カードは別ウィンドウなので、`main` がこれをグローバル座標へ足して使う。
    pub fn card_origin(&self, i: usize, card_w: f64, card_h: f64) -> Option<(f64, f64)> {
        let slot = self.slot(i)?;
        let (ox, oy) = self.stage_origin();
        // 生き物の水平中心に揃える。
        let x = ox + slot.hit_x + slot.hit_w / 2.0 - card_w / 2.0;
        let y = if self.size.is_bar() {
            // bar: 帯のすぐ下。帯自体がメニューバーの中なので、カードはバーの下に垂れる。
            oy + slot.body_y - theme::card_offset(self.size) - card_h
        } else {
            // dock: 背景パネルの**上端より上**へ完全に逃がす。
            // 体からの相対位置で置くと、パネルの上余白（DOCK_PAD_TOP ＋ グリフ用の
            // 余白）にカードの下端が食い込み、角丸パネルの縁に被って見える（実測で
            // 12pt 重なった）。パネル高を基準にすれば、群れの構成が変わっても常に浮く。
            self.window_size().1 + theme::CARD_PANEL_GAP
        };
        Some((x, y))
    }
}

/// `Session` から生き物の表示状態を作る。
fn view_of(s: &Session, now: u64, done_ttl_ms: u64, show_glyphs: bool) -> View {
    let state = s.display_state(now, done_ttl_ms);
    View {
        state,
        agents: s.agents.len(),
        short: s.short_name().to_string(),
        dur: Session::fmt_dur(now.saturating_sub(s.since)),
        show_glyph: show_glyphs,
    }
}

/// `Session` からホバーカードの内容を作る。
fn card_view_of(s: &Session, now: u64, done_ttl_ms: u64, lang: Lang) -> CardView {
    let state = s.display_state(now, done_ttl_ms);
    CardView {
        name: s.name.clone(),
        title: s.title.clone(),
        state,
        state_label: state.label(lang),
        dur: Session::fmt_dur(now.saturating_sub(s.since)),
        agents: s
            .agents
            .iter()
            .map(|a| AgentRow {
                name: a.name.clone(),
                role: a.role.clone(),
                state: a.state,
            })
            .collect(),
    }
}

/// デモ用のダミーセッション。`ccsessionsd --demo` が使う。
///
/// 実際に Claude Code を 6 セッション走らせずに 6 状態すべての見た目を確認できると、
/// theme.rs のイテレーションが桁違いに速くなる。
pub fn demo_sessions(now: u64) -> Vec<Session> {
    use ccsessions_core::session::Agent;
    let mk = |id: &str,
              name: &str,
              title: Option<&str>,
              state: SessionState,
              mins: u64,
              agents: Vec<(&str, &str, SessionState)>| Session {
        id: id.to_string(),
        name: name.to_string(),
        // タイトルは「あるセッション」と「まだ無いセッション」の両方を混ぜてある
        // （カードが 2 行と 1 行の両方になるのを一度に見るため）。
        title: title.map(str::to_string),
        cwd: format!("/demo/{name}"),
        state,
        since: now.saturating_sub(mins * 60_000),
        updated: now,
        main_stopped: false,
        error_kind: None,
        // デモのセッションに持ち主は居ない。pid 無し ＝ 生存確認の対象外。
        pid: None,
        agents: agents
            .into_iter()
            .map(|(n, r, st)| Agent {
                name: n.to_string(),
                role: r.to_string(),
                state: st,
                id: String::new(),
            })
            .collect(),
    };
    vec![
        mk(
            "ccsessions",
            "ccsessions",
            Some("ホバーカードにセッションタイトルを出す"),
            SessionState::Working,
            75,
            vec![
                ("ada", "設計", SessionState::Working),
                ("eden", "実装", SessionState::Working),
            ],
        ),
        mk(
            "eden",
            "eden-overlay",
            Some("メニューバーの当たり判定がずれる件の調査と、帯の高さの再計算"),
            SessionState::WaitUser,
            8,
            vec![("lead", "リード", SessionState::WaitUser)],
        ),
        mk(
            "notion",
            "notion-sync",
            None,
            SessionState::WaitAgent,
            5,
            vec![
                ("beatrice", "境界", SessionState::Working),
                ("ada", "設計", SessionState::WaitAgent),
            ],
        ),
        mk(
            "dots",
            "dotfiles",
            Some("zsh の起動時間を詰める"),
            SessionState::Done,
            41,
            vec![("eden", "実装", SessionState::Done)],
        ),
        mk(
            "paper",
            "paper-review",
            None,
            SessionState::Idle,
            92,
            vec![("scholar", "読解", SessionState::Idle)],
        ),
        mk(
            "scr",
            "web-scraper",
            Some("robots.txt の解釈が甘い"),
            SessionState::Error,
            6,
            vec![("crawler", "収集", SessionState::Error)],
        ),
    ]
}
