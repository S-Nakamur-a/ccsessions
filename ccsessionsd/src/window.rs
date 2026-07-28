//! オーバーレイ窓の設定と、ホバーを拾うためのカスタム NSView。
//!
//! # 設計の要点
//! - **ウィンドウレベルは `NSStatusWindowLevel`(25)**。メニューバー(`NSMainMenuWindowLevel`=24)
//!   より 1 だけ手前なので、生き物がバーの上に乗る。101（ポップアップメニュー）まで上げると
//!   メニューを開いたときにその上に被さってしまうので上げすぎない。
//! - **窓は帯のサイズぴったりに作る**。マウスイベントを受ける窓は自分の矩形ぶんだけ
//!   下のメニューバーのクリックを奪う。群れの幅しか占有しなければ、メニューバー左右の
//!   通常操作は無傷で残る。
//! - **全 Space に出す**（`CanJoinAllSpaces` + `Stationary`）。Space を切り替えても
//!   セッションの様子が見えていてほしい。フルスクリーンアプリの上にも出す
//!   （`FullScreenAuxiliary`）。
//! - 透明窓の影は「可視ピクセルの形」に沿って描かれ、生き物の周りに黒いハローが出る。
//!   `setHasShadow(false)` で切り、影はレイヤ側（カードなど）で個別に付ける。

use std::cell::Cell;

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{define_class, msg_send, AnyThread, DefinedClass, MainThreadOnly};
use objc2_app_kit::{
    NSCursor, NSEvent, NSStatusWindowLevel, NSTrackingArea, NSTrackingAreaOptions, NSView,
    NSWindow, NSWindowCollectionBehavior,
};
use objc2_foundation::{MainThreadMarker, NSObjectProtocol, NSRect};
use tao::event_loop::EventLoopProxy;
use tao::platform::macos::WindowExtMacOS;
use tao::window::Window;

use crate::geometry::Rect;
use crate::AppEvent;

/// tao の窓から `NSWindow` を借りる。
///
/// # SAFETY
/// メインスレッド文脈で、tao が生存させている窓の生ポインタから一時参照を借りるだけ。
pub fn ns_window(window: &Window) -> &NSWindow {
    unsafe { &*(window.ns_window() as *mut NSWindow) }
}

/// オーバーレイとしての基本設定を入れる。起動時に 1 回だけ呼ぶ。
pub fn configure(window: &Window) {
    let w = ns_window(window);
    w.setLevel(NSStatusWindowLevel);
    w.setCollectionBehavior(
        NSWindowCollectionBehavior::CanJoinAllSpaces
            | NSWindowCollectionBehavior::Stationary
            | NSWindowCollectionBehavior::IgnoresCycle
            | NSWindowCollectionBehavior::FullScreenAuxiliary,
    );
    w.setHasShadow(false);
    w.setOpaque(false);
    w.setBackgroundColor(Some(&objc2_app_kit::NSColor::clearColor()));
    // ホバーを取るためにマウス移動イベントを受け取る。
    w.setAcceptsMouseMovedEvents(true);
    w.setIgnoresMouseEvents(false);

    if let Some(content) = w.contentView() {
        content.setWantsLayer(true);
    }
}

/// 窓の矩形を AppKit のグローバル座標（左下原点）で直接設定する。
///
/// tao の `set_outer_position` は左上原点の論理座標なので、`geometry.rs` が返す
/// 左下原点の矩形とは相性が悪い。`NSWindow::setFrame_display` を直に使って
/// 座標系の変換を 1 か所に閉じ込める。
pub fn set_frame(window: &Window, r: Rect) {
    let w = ns_window(window);
    w.setFrame_display(
        NSRect {
            origin: objc2_foundation::NSPoint { x: r.x, y: r.y },
            size: objc2_foundation::NSSize {
                width: r.w,
                height: r.h,
            },
        },
        true,
    );
}

/// 窓の表示・非表示。
///
/// **tao の `Window::set_visible` を使ってはいけない。** 中身は
/// `makeKeyAndOrderFront`（`tao-0.35.3/src/platform_impl/macos/window.rs`）で、
/// **表示のたびにキーウィンドウを奪い、アプリをアクティブ化しうる**。この daemon は
/// セッションが変わるたびに窓を出し直し、ホバーのたびにカード窓を出すので、
/// そのままだと入力中のエディタからフォーカスを奪い続けることになる。
///
/// `orderFront:` は「前面に出すがキーにはしない」。常駐オーバーレイが欲しいのは
/// こちらだけ。
///
/// 併せて、窓は必ず `WindowBuilder::with_visible(false)` で作ること。tao は
/// `applicationDidFinishLaunching` で「その時点で可視な窓」に `makeKeyAndOrderFront`
/// を掛け直す（`app_state.rs` の `window_activation_hack`）ので、可視で作ると
/// 起動時に一度フォーカスを奪われる。不可視で作って後から `orderFront` すれば
/// 一度もアクティブにならない。
pub fn set_visible(window: &Window, visible: bool) {
    let w = ns_window(window);
    if visible {
        w.orderFront(None);
    } else {
        w.orderOut(None);
    }
}

/// contentView のバッキングレイヤ。群れの root レイヤをここに差す。
pub fn backing_layer(window: &Window) -> Option<Retained<objc2_quartz_core::CALayer>> {
    ns_window(window).contentView()?.layer()
}

// ---------------------------------------------------------------------------
// HoverView — マウスの出入り・移動・ドラッグをイベントループへ流すビュー
// ---------------------------------------------------------------------------

/// ここを超えて動いたら「クリック」ではなく「ドラッグ」と見なす距離（pt）。
///
/// 0 にすると、ホバーしようとして触れただけの微動で群れが動いてしまう。逆に大きいと
/// 動かし始めが重く感じる。3pt は「押して離すだけの操作では絶対に超えない」かつ
/// 「動かす意思があれば一瞬で超える」あたり。
const DRAG_THRESHOLD: f64 = 3.0;

/// ボタンを押した瞬間に固定される、ドラッグの基準。
///
/// **移動量は「押した瞬間からの総和」で計算する**（前回のイベントからの差分を
/// 足し込まない）。窓は画面の縁でクランプされて提案どおりには動かないので、差分を
/// 積むと縁に当てたぶんだけ実位置と提案位置がずれていき、引き返しても戻らなくなる。
#[derive(Clone, Copy)]
pub struct Grab {
    /// 押した瞬間のカーソル位置（画面座標）。
    cursor: (f64, f64),
    /// 押した瞬間のパネル中心 x / 下端 y（画面座標）。
    panel: (f64, f64),
}

pub struct HoverViewIvars {
    proxy: EventLoopProxy<AppEvent>,
    /// ボタンを押している間だけ `Some`。
    grab: Cell<Option<Grab>>,
    /// しきい値を超えて実際にドラッグへ移行したか。
    dragging: Cell<bool>,
    /// いま dock 配置か。**bar ではドラッグもカーソル変更もしない**
    /// （メニューバー上の帯は位置に意味があるので動かさない）。
    draggable: Cell<bool>,
}

define_class!(
    // SAFETY:
    // - スーパークラス NSView はサブクラス要件を持たない。
    // - `HoverView` は `Drop` を実装しない（ivars ドロップは objc2 が行う）。
    // - MainThreadOnly: マウスイベントはメインスレッドで配送される。
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[ivars = HoverViewIvars]
    pub struct HoverView;

    // SAFETY: `NSObjectProtocol` に安全要件なし。
    unsafe impl NSObjectProtocol for HoverView {}

    impl HoverView {
        /// カーソルが帯に入った。
        // SAFETY: セレクタ `mouseEntered:` のシグネチャは正しい（NSEvent* 引数 1 つ）。
        #[unsafe(method(mouseEntered:))]
        fn mouse_entered(&self, event: &NSEvent) {
            self.apply_cursor();
            self.send_move(event);
        }

        /// 帯の中でカーソルが動いた。どの生き物の上かはイベントループ側で判定する。
        // SAFETY: セレクタ `mouseMoved:` のシグネチャは正しい。
        #[unsafe(method(mouseMoved:))]
        fn mouse_moved(&self, event: &NSEvent) {
            // `cursorUpdate:` の保険。**単独では効かない**（AppKit がマウス移動のたびに
            // カーソルを決め直して上書きする）が、届く条件が増えるぶんには害がない。
            self.apply_cursor();
            self.send_move(event);
        }

        /// AppKit が「この領域のカーソルを決めろ」と聞いてくる正規の入口。
        ///
        /// **`mouseMoved:` で `NSCursor::set()` しても効かない** — AppKit はマウスが
        /// 動くたびにカーソル矩形からカーソルを決め直すので、こちらで設定した直後に
        /// 上書きされる（実測: 矢印のまま変わらなかった）。`NSTrackingArea` に
        /// `CursorUpdate` を足してこのメソッドで応じるのが、AppKit が用意している経路。
        ///
        /// **ただしこれでもホバーだけでは届かない**（`apply_cursor` の doc 参照）。
        /// 押している間だけ効く。`ActiveAlways` を付けても、カーソルの所有権は
        /// アクティブなアプリ側にあるため。
        // SAFETY: セレクタ `cursorUpdate:` のシグネチャは正しい（NSEvent* 引数 1 つ）。
        #[unsafe(method(cursorUpdate:))]
        fn cursor_update(&self, _event: &NSEvent) {
            self.apply_cursor();
        }

        /// カーソルが帯から出た。カードを畳む。
        // SAFETY: セレクタ `mouseExited:` のシグネチャは正しい。
        #[unsafe(method(mouseExited:))]
        fn mouse_exited(&self, _event: &NSEvent) {
            // カーソルを戻すのは**自分で変えた場合だけ**。bar 配置では一度も
            // 触っていないので、ここで矢印にすると出た先（テキスト欄の I ビーム等）の
            // カーソルを一瞬奪ってしまう。
            //
            // ドラッグ中も戻さない。画面の縁でパネルがクランプされて指だけ先に行くと
            // 帯の外へ出るが、掴んだままなので握り拳を保つのが正しい。
            if self.ivars().draggable.get() && !self.ivars().dragging.get() {
                NSCursor::arrowCursor().set();
            }
            let _ = self.ivars().proxy.send_event(AppEvent::Hover(None));
        }

        /// パネルを掴んだ。この時点ではまだ動かさない（クリックかもしれない）。
        // SAFETY: セレクタ `mouseDown:` のシグネチャは正しい。
        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            if !self.ivars().draggable.get() {
                return;
            }
            let Some(cursor) = self.screen_point(event) else {
                return;
            };
            let Some(frame) = self.window().map(|w| w.frame()) else {
                return;
            };
            self.ivars().grab.set(Some(Grab {
                cursor,
                panel: (
                    frame.origin.x + frame.size.width / 2.0,
                    frame.origin.y,
                ),
            }));
            self.ivars().dragging.set(false);
        }

        /// 掴んだまま動かしている。しきい値を超えて初めてドラッグになる。
        // SAFETY: セレクタ `mouseDragged:` のシグネチャは正しい。
        #[unsafe(method(mouseDragged:))]
        fn mouse_dragged(&self, event: &NSEvent) {
            let Some(grab) = self.ivars().grab.get() else {
                return;
            };
            let Some((cx, cy)) = self.screen_point(event) else {
                return;
            };
            let (dx, dy) = (cx - grab.cursor.0, cy - grab.cursor.1);
            if !self.ivars().dragging.get() {
                if dx.hypot(dy) < DRAG_THRESHOLD {
                    return;
                }
                self.ivars().dragging.set(true);
                self.apply_cursor();
            }
            // **窓は動かさない**。位置の真実は設定側にあり、窓を動かすのは
            // `main::reposition` だけ（`geometry::dock_rect` の doc 参照）。
            let _ = self.ivars().proxy.send_event(AppEvent::DockDragged {
                x: grab.panel.0 + dx,
                y: grab.panel.1 + dy,
            });
        }

        /// 離した。実際に動かしていたときだけ保存を促す。
        // SAFETY: セレクタ `mouseUp:` のシグネチャは正しい。
        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, _event: &NSEvent) {
            self.ivars().grab.set(None);
            if self.ivars().dragging.replace(false) {
                self.apply_cursor();
                let _ = self.ivars().proxy.send_event(AppEvent::DockDragEnded);
            }
        }
    }
);

impl HoverView {
    fn new(proxy: EventLoopProxy<AppEvent>, mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(HoverViewIvars {
            proxy,
            grab: Cell::new(None),
            dragging: Cell::new(false),
            draggable: Cell::new(false),
        });
        // SAFETY: NSView の `init` シグネチャは正しい。
        unsafe { msg_send![super(this), init] }
    }

    /// この帯を掴んで動かせるか（dock 配置のときだけ true）を切り替える。
    ///
    /// 配置が変わったら `main` が呼ぶ。ドラッグ中に bar へ切り替わっても、
    /// 掴んでいる状態は次の `mouseUp` で必ず解けるので取り残されない。
    pub fn set_draggable(&self, on: bool) {
        self.ivars().draggable.set(on);
        if !on {
            self.cancel_drag();
        }
    }

    /// いま掴んで動かしている最中か。
    ///
    /// `main` が「設定ファイルの変更通知で位置を上書きしてよいか」を判断するのに使う
    /// （ドラッグ中は in-memory の位置が正で、ファイル側は古い）。
    pub fn is_dragging(&self) -> bool {
        self.ivars().dragging.get()
    }

    /// 掴んでいる状態を強制的に解除する。
    ///
    /// **`mouseUp:` が来ない経路があるため要る。** 群れが空になると `reposition` が窓ごと
    /// `orderOut` するので、その先の `mouseUp` はこのビューに届かない。放置すると
    /// `dragging` が真のまま残り、カーソルが握り拳で固まる。
    pub fn cancel_drag(&self) {
        self.ivars().grab.set(None);
        self.ivars().dragging.set(false);
    }

    /// いまの状態に合ったカーソルを当てる。掴めるなら開いた手、掴んでいれば握り拳。
    ///
    /// # 既知の限界（実測）
    /// **ホバーしただけでは変わらない。押している間だけ効く。** macOS はカーソルの
    /// 管理を**アクティブなアプリ**に握らせるので、非アクティブかつキーになれない
    /// この窓では、ただカーソルが乗っているだけの状態で `cursorUpdate:` が届かない
    /// （届いても直後に前面アプリのカーソル矩形で上書きされる）。ボタンを押している
    /// 間はマウスがこの窓に捕まるため、そこでは狙いどおり握り拳になる。
    ///
    /// ホバーだけで変えるにはアプリをアクティブ化するしかなく、それは
    /// 「フォーカスを奪わない」という最重要の不変条件と引き換えになるので**採らない**。
    /// 掴んだ瞬間に手の形になれば「動かせる」ことは伝わる、と割り切っている。
    fn apply_cursor(&self) {
        if !self.ivars().draggable.get() {
            return;
        }
        if self.ivars().dragging.get() {
            NSCursor::closedHandCursor().set();
        } else {
            NSCursor::openHandCursor().set();
        }
    }

    /// イベントの位置を**画面座標**へ直す。
    ///
    /// `locationInWindow` は窓が動けば一緒にずれるので、ドラッグ中の基準には使えない。
    /// そのつどの窓の frame を足して絶対座標に直せば、窓がクランプされて提案どおりに
    /// 動かなかった場合でもカーソルの実位置を取り違えない。
    fn screen_point(&self, event: &NSEvent) -> Option<(f64, f64)> {
        let f = self.window()?.frame();
        let p = event.locationInWindow();
        Some((f.origin.x + p.x, f.origin.y + p.y))
    }

    /// マウス位置をウィンドウ座標で流す。
    ///
    /// 枠なし窓では contentView の原点がウィンドウ原点と一致するので、
    /// `locationInWindow` をそのままビュー座標として使える。
    fn send_move(&self, event: &NSEvent) {
        let p = event.locationInWindow();
        let _ = self
            .ivars()
            .proxy
            .send_event(AppEvent::Hover(Some((p.x, p.y))));
    }
}

/// ホバーとドラッグを拾うビューを contentView に重ねる。
///
/// 返り値は呼び出し側が保持すること（トラッキング領域の貼り直しと、配置切替に
/// 伴う `set_draggable` に使う）。
pub fn attach_hover(
    window: &Window,
    proxy: EventLoopProxy<AppEvent>,
    mtm: MainThreadMarker,
) -> Retained<HoverView> {
    let view = HoverView::new(proxy, mtm);
    let w = ns_window(window);
    if let Some(content) = w.contentView() {
        let b = content.bounds();
        view.setFrame(b);
        // 窓のリサイズにビューを追従させる（配置切替で窓の大きさが変わる）。
        view.setAutoresizingMask(
            objc2_app_kit::NSAutoresizingMaskOptions::ViewWidthSizable
                | objc2_app_kit::NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        content.addSubview(&view);
    }
    refresh_tracking(&view);
    view
}

/// トラッキング領域を貼り直す。窓の大きさが変わったら呼ぶ。
///
/// `InVisibleRect` を付けると領域がビューの可視矩形に自動追従するので、
/// 本来は貼り直し不要だが、`AssumeInside` を使わない代わりに配置切替時は
/// 明示的に貼り直して取りこぼしを避ける。
pub fn refresh_tracking(view: &NSView) {
    // 既存のトラッキング領域を外す。
    // SAFETY: `trackingAreas` は NSArray<NSTrackingArea> を返す。
    let existing: Retained<objc2_foundation::NSArray<NSTrackingArea>> =
        unsafe { msg_send![view, trackingAreas] };
    for i in 0..existing.len() {
        view.removeTrackingArea(&existing.objectAtIndex(i));
    }

    let opts = NSTrackingAreaOptions::MouseEnteredAndExited
        | NSTrackingAreaOptions::MouseMoved
        // CursorUpdate: `cursorUpdate:` を呼んでもらう。これが無いと
        // 掴めることを示すカーソル（開いた手／握り拳）が一切出ない。
        | NSTrackingAreaOptions::CursorUpdate
        // ActiveAlways: アプリが非アクティブでも追跡する。ccsessionsd は accessory で
        // 決してアクティブにならないので、これが無いとホバーが一切飛ばない。
        | NSTrackingAreaOptions::ActiveAlways
        | NSTrackingAreaOptions::InVisibleRect;

    let area = NSTrackingArea::alloc();
    // SAFETY: rect は InVisibleRect 指定時は無視される。owner はこのビュー（
    // mouseEntered:/mouseMoved:/mouseExited: を実装している）。userInfo は不要。
    let area = unsafe {
        NSTrackingArea::initWithRect_options_owner_userInfo(
            area,
            view.bounds(),
            opts,
            Some(&*(view as *const NSView as *const AnyObject)),
            None,
        )
    };
    view.addTrackingArea(&area);
}
