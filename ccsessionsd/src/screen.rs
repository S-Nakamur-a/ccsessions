//! 画面の実測値の取得と、ディスプレイ構成変化への追従。
//!
//! `geometry.rs` が純関数で配置を決められるよう、AppKit から読んだ値を
//! `ScreenMetrics` に詰めて渡すのがここの仕事。
//!
//! 構成変化を自分で購読しているのは次の理由:
//! tao 0.35 は macOS の画面構成変化に対してイベントを出さないため、外部モニタを
//! 繋ぐと窓が古いジオメトリのまま取り残される。`NSApplicationDidChangeScreen
//! ParametersNotification` を直接購読して、メインループへ再配置を促す。

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{define_class, msg_send, sel, DefinedClass, MainThreadOnly};
use objc2_app_kit::{NSApplicationDidChangeScreenParametersNotification, NSScreen};
use objc2_core_foundation::{CFArray, CFDictionary, CFNumber, CFString, CFType, CGRect};
use objc2_core_graphics::{
    kCGWindowBounds, kCGWindowLayer, kCGWindowOwnerPID, CGRectMakeWithDictionaryRepresentation,
    CGWindowListCopyWindowInfo, CGWindowListOption,
};
use objc2_foundation::{
    MainThreadMarker, NSNotification, NSNotificationCenter, NSObject, NSObjectProtocol, NSRect,
};
use tao::event_loop::EventLoopProxy;

use ccsessions_core::config::Placement;

use crate::geometry::{Rect, ScreenMetrics};
use crate::AppEvent;

fn to_rect(r: NSRect) -> Rect {
    Rect::new(r.origin.x, r.origin.y, r.size.width, r.size.height)
}

/// primary スクリーンの実測値を読む。スクリーンが取れない場合（起動直後の稀な状況）は
/// `None` を返し、呼び出し側は次のポーリングまで待つ。
///
/// メニューエクストラの実測（`menu_extra_left`）は **`placement == Bar` かつノッチが
/// あるときだけ**行う。正味の増分は中央値で約 +500〜640µs（窓数 25）で、`metrics()`
/// は `reposition` と `current_packing` から呼ばれ、**dock 配置のドラッグ中はマウス
/// 移動ごと（〜60Hz）**に走る。dock ではこの値を一切使わないので、条件なしに毎回
/// 測ると無駄な FFI 呼び出しを払い続けることになる（計測値と対照実験は
/// `docs/adr/0012-notch-avoidance.md` 参照）。
pub fn metrics(placement: Placement, mtm: MainThreadMarker) -> Option<ScreenMetrics> {
    let s = NSScreen::mainScreen(mtm)?;
    let frame = to_rect(s.frame());

    // `auxiliaryTopLeftArea` / `auxiliaryTopRightArea` はノッチ機だけが意味のある値を返す。
    // ノッチが無い機種では画面全幅と等しい矩形（＝隙間なし）になるため、
    // 「左の右端 < 右の左端」で隙間があるときだけノッチとして扱う（geometry 側で判定）。
    // ゼロ矩形が返る可能性もあるので、面積 0 は None に落とす。
    let aux = |r: NSRect| {
        let r = to_rect(r);
        (r.w > 0.0 && r.h > 0.0).then_some(r)
    };

    let mut m = ScreenMetrics {
        frame,
        visible: to_rect(s.visibleFrame()),
        safe_area_top: s.safeAreaInsets().top,
        aux_top_left: aux(s.auxiliaryTopLeftArea()),
        aux_top_right: aux(s.auxiliaryTopRightArea()),
        menu_extra_left: None,
    };
    if placement == Placement::Bar && m.notch_x_range().is_some() {
        m.menu_extra_left = menu_extra_left(mtm, &m);
    }
    Some(m)
}

/// メニューエクストラが置かれる CoreGraphics の window layer。
const MENU_EXTRA_LAYER: i64 = 25;

/// メニューエクストラ（時計・コントロールセンター等）の左端 x を実測する。
///
/// `CGWindowListCopyWindowInfo` はメニューエクストラを **layer 25** の窓として返す。
/// その最小 x が「ノッチ右の空き帯」の右端になる。**画面収録権限は要らない**
/// （権限が無いと伏せられるのは `kCGWindowName` だけで、layer / bounds / ownerPID
/// は取れる）。
///
/// 返すのは **AppKit グローバル座標**（左下原点）の x。エクストラが 1 つも見つから
/// なければ `None`（呼び出し側は既定の見積もりへ落とす）。
///
/// 除外するもの:
/// - **自分の窓**。ccsessionsd 自身も level 25 に窓を置くので、除かないと自分の左端を
///   返してしまう（作者機の実測: 自分が x=856、エクストラが x=1073）。
/// - メニューバー帯の外に居る layer 25 の窓（他アプリのフローティング窓など）。
///   窓の**縦中心**が帯に入っているかで判定する。帯高 33 に対しエクストラは高さ 33、
///   ccsessionsd の帯は 32 と 1pt ずれるので、端の一致には頼らない。
/// - **メインスクリーン以外のディスプレイに居る窓**。CG の窓一覧は全ディスプレイを
///   束ねて返すので、帯の在る画面の水平範囲で絞る。
fn menu_extra_left(mtm: MainThreadMarker, m: &ScreenMetrics) -> Option<f64> {
    // AppKit のグローバル座標の原点は primary スクリーン（＝ `screens()[0]`）の左下。
    // CG のグローバル座標は同じ点を原点とする左上原点系なので、y だけを
    // `primary.max_y() - (cg_y + cg_h)` で折り返せば AppKit 座標になる（x は同一）。
    let flip_base = to_rect(NSScreen::screens(mtm).firstObject()?.frame()).max_y();

    let band_top = m.frame.max_y();
    let band_bottom = band_top - m.menu_bar_height();

    let list = CGWindowListCopyWindowInfo(
        CGWindowListOption::OptionOnScreenOnly | CGWindowListOption::ExcludeDesktopElements,
        0, // kCGNullWindowID
    )?;
    // SAFETY: `CGWindowListCopyWindowInfo` は「`kCGWindow*` を鍵とする `CFDictionary`」
    // の `CFArray` を返すと文書化されている。値の型は `CFType` として受け、実際の型は
    // `downcast` で確かめるので、取り違えても panic せず `None` に落ちる。
    let list: &CFArray<CFDictionary<CFString, CFType>> = unsafe { list.cast_unchecked() };

    let me = std::process::id() as i64;
    let mut left: Option<f64> = None;

    for w in list.iter() {
        // SAFETY: `kCGWindow*` は CoreGraphics が公開する定数文字列（extern static）。
        let (key_layer, key_pid, key_bounds) =
            unsafe { (kCGWindowLayer, kCGWindowOwnerPID, kCGWindowBounds) };

        let layer = w
            .get(key_layer)
            .and_then(|v| v.downcast::<CFNumber>().ok()?.as_i64());
        if layer != Some(MENU_EXTRA_LAYER) {
            continue;
        }
        let pid = w
            .get(key_pid)
            .and_then(|v| v.downcast::<CFNumber>().ok()?.as_i64());
        // **pid が読めない窓も飛ばす（fail-close）。** `pid != Some(me)` だけで
        // 判定すると、pid が読めなかった窓（`None`）は「自分ではない」側に倒れて
        // 候補に残る。自分の窓を取り込むと空き幅が 225 → 8 に化け、
        // `docs/adr/0012-notch-avoidance.md` に書いた 10 秒周期の左右振動になる。
        // bounds が読めないときに `continue` するのと向きを揃える。
        if pid.is_none() || pid == Some(me) {
            continue;
        }
        let Some(r) = w
            .get(key_bounds)
            .and_then(|v| v.downcast::<CFDictionary>().ok())
            .and_then(|d| {
                let mut r = CGRect::default();
                // SAFETY: `d` は CG 自身が `kCGWindowBounds` に入れた
                // `{X,Y,Width,Height}` 表現の辞書であり、この関数が期待する形。
                unsafe { CGRectMakeWithDictionaryRepresentation(Some(&d), &mut r) }.then_some(r)
            })
        else {
            continue;
        };

        let centre_y = flip_base - (r.origin.y + r.size.height / 2.0);
        if centre_y < band_bottom || centre_y > band_top {
            continue;
        }
        // メインスクリーン以外のディスプレイに居る窓を除く。CG の窓一覧は全ディスプレイを
        // 束ねて返すので、帯の在る画面の水平範囲で絞らないと副ディスプレイの
        // メニューエクストラを拾いうる。
        if !m.frame.contains_x(r.origin.x) {
            continue;
        }
        left = Some(left.map_or(r.origin.x, |v: f64| v.min(r.origin.x)));
    }
    left
}

/// Retina 倍率。テキストレイヤを鮮明にするために使う。
pub fn backing_scale(mtm: MainThreadMarker) -> f64 {
    NSScreen::mainScreen(mtm)
        .map(|s| s.backingScaleFactor())
        .unwrap_or(2.0)
}

// ---------------------------------------------------------------------------
// 画面構成変化の購読
// ---------------------------------------------------------------------------

struct ScreenObserverIvars {
    proxy: EventLoopProxy<AppEvent>,
}

define_class!(
    // SAFETY:
    // - スーパークラス NSObject はサブクラス要件を持たない。
    // - `ScreenObserver` は `Drop` を実装しない（ivars ドロップは objc2 が行う）。
    // - MainThreadOnly: 画面構成通知はメインスレッドで配送される。
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = ScreenObserverIvars]
    struct ScreenObserver;

    // SAFETY: `NSObjectProtocol` に安全要件なし。
    unsafe impl NSObjectProtocol for ScreenObserver {}

    impl ScreenObserver {
        // SAFETY: セレクタ `screenParametersChanged:` のシグネチャは正しい（NSNotification* 1 つ）。
        #[unsafe(method(screenParametersChanged:))]
        fn screen_parameters_changed(&self, _notification: &NSNotification) {
            // 受け手が落ちているのはプロセス終了時のみ。失敗は無視でよい。
            let _ = self.ivars().proxy.send_event(AppEvent::ScreenChanged);
        }
    }
);

impl ScreenObserver {
    fn new(proxy: EventLoopProxy<AppEvent>, mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(ScreenObserverIvars { proxy });
        // SAFETY: NSObject の `init` シグネチャは正しい。
        unsafe { msg_send![super(this), init] }
    }
}

/// 画面構成変化の observer を作って返す。
///
/// `NSNotificationCenter` は observer を retain しないので、返り値は呼び出し側が
/// 生かし続けること（main は `run()` で発散するので、そこに束縛すれば足りる）。
pub fn observe(proxy: EventLoopProxy<AppEvent>, mtm: MainThreadMarker) -> Retained<NSObject> {
    let observer = ScreenObserver::new(proxy, mtm);
    let center = NSNotificationCenter::defaultCenter();
    // SAFETY: observer は `screenParametersChanged:` を実装している。name は AppKit の
    // extern static（通知名）。object=None で送信元を問わず購読する。
    unsafe {
        center.addObserver_selector_name_object(
            &*(Retained::as_ptr(&observer) as *const AnyObject),
            sel!(screenParametersChanged:),
            Some(NSApplicationDidChangeScreenParametersNotification),
            None,
        );
    }
    // 具体型を外へ漏らさず、生存維持だけを担わせるため NSObject へ erase。
    Retained::into_super(observer)
}
