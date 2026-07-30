//! AppKit / CoreAnimation を触るときの薄いヘルパ。
//!
//! FFI の定型（NSRect を組む・sRGB から CGColor を作る・&str から NSString を作る・
//! CATextLayer にフォントを差す）を 1 か所に集めて、`creature.rs` や `card.rs` の
//! 見通しを保つ。ここに**ロジックは置かない**。
//!
//! # SAFETY の共通前提
//! - AppKit / CoreAnimation オブジェクトはメインスレッドでしか触らない。呼び出し側
//!   （`main.rs` のイベントループ、`define_class!` の MainThreadOnly なビュー）が
//!   それを保証する。
//! - `unsafe` を要求する API はいずれも「型の合った引数を渡す」ことだけが要件で、
//!   各呼び出し箇所にその根拠をコメントで書く。

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::{NSColor, NSFont, NSFontWeightBold, NSFontWeightMedium};
use objc2_core_foundation::{CFRetained, CFType};
use objc2_core_graphics::{CGColor, CGMutablePath, CGPath};
use objc2_foundation::{NSLocale, NSPoint, NSRect, NSSize, NSString};
use objc2_quartz_core::{CALayer, CATextLayer};

use crate::theme::{Outline, Rgb, Seg};

// ---------------------------------------------------------------------------
// ロケール
// ---------------------------------------------------------------------------

/// OS の言語タグ（`"ja-JP"` 等）。取れなければ `None`。
///
/// **CLI と違い環境変数は見ない。** launchd から起動される常駐は `LANG` を
/// 継承しないので、`brew services` 経由では常に取れないことになる。
///
/// `NSBundle::preferredLocalizations` ではなく
/// `NSLocale::preferredLanguages` を使う: 前者はアプリが同梱している
/// `.lproj` との交差なので、ローカライズを 1 つも同梱していないこのバイナリでは
/// 常に開発言語だけを返す。欲しいのは**ユーザが「システム設定 > 言語と地域」で
/// 並べた希望**そのもの。
pub fn os_language_tag() -> Option<String> {
    NSLocale::preferredLanguages()
        .iter()
        .next()
        .map(|s| s.to_string())
}

// ---------------------------------------------------------------------------
// 幾何・色・文字列
// ---------------------------------------------------------------------------

pub fn rect(x: f64, y: f64, w: f64, h: f64) -> NSRect {
    NSRect {
        origin: NSPoint { x, y },
        size: NSSize {
            width: w,
            height: h,
        },
    }
}

/// sRGB + alpha から CGColor を作る。
///
/// デザインの色は oklch で与えられており、一部は sRGB 域外（`working` の accent は
/// 赤成分が負）。`theme.rs` の時点で 0..1 にクランプ済みなので、ここでは素通しでよい。
pub fn cgcolor(c: Rgb, a: f64) -> Retained<CGColor> {
    NSColor::colorWithSRGBRed_green_blue_alpha(c.0, c.1, c.2, a).CGColor()
}

pub fn ns(s: &str) -> Retained<NSString> {
    NSString::from_str(s)
}

// ---------------------------------------------------------------------------
// パス
// ---------------------------------------------------------------------------

/// `theme::outline` が返した輪郭を `CGPath` にする。
///
/// `origin` は輪郭を平行移動させる量（レイヤ内で内側にオフセットしたいときに使う。
/// CAShapeLayer のストロークはパス上に中心を置くので、枠線幅の半分だけ内側に
/// 縮めた輪郭を作って渡すと、枠が体からはみ出さない）。
pub fn path_from_outline(o: &Outline, origin: (f64, f64)) -> CFRetained<CGMutablePath> {
    let p = CGMutablePath::new();
    let (ox, oy) = origin;
    let mv = |(x, y): (f64, f64)| (x + ox, y + oy);

    let (sx, sy) = mv(o.start);
    // SAFETY: `m` に null を渡すのは「変換なし」の意味。座標は有限の f64。
    unsafe { CGMutablePath::move_to_point(Some(&p), std::ptr::null(), sx, sy) };
    for seg in &o.segs {
        match *seg {
            Seg::Line { to } => {
                let (x, y) = mv(to);
                // SAFETY: 同上。
                unsafe { CGMutablePath::add_line_to_point(Some(&p), std::ptr::null(), x, y) };
            }
            Seg::Cubic { c1, c2, to } => {
                let (c1x, c1y) = mv(c1);
                let (c2x, c2y) = mv(c2);
                let (tx, ty) = mv(to);
                // SAFETY: 同上。
                unsafe {
                    CGMutablePath::add_curve_to_point(
                        Some(&p),
                        std::ptr::null(),
                        c1x,
                        c1y,
                        c2x,
                        c2y,
                        tx,
                        ty,
                    )
                };
            }
        }
    }
    CGMutablePath::close_subpath(Some(&p));
    p
}

/// 点列を閉じた多角形の `CGPath` にする（塗り用。目のスリットに使う）。
pub fn path_from_polygon(pts: &[(f64, f64)]) -> CFRetained<CGMutablePath> {
    let p = CGMutablePath::new();
    for (i, &(x, y)) in pts.iter().enumerate() {
        // SAFETY: `m` に null を渡すのは「変換なし」の意味。座標は有限の f64。
        unsafe {
            if i == 0 {
                CGMutablePath::move_to_point(Some(&p), std::ptr::null(), x, y);
            } else {
                CGMutablePath::add_line_to_point(Some(&p), std::ptr::null(), x, y);
            }
        }
    }
    CGMutablePath::close_subpath(Some(&p));
    p
}

/// 折れ線の集まりを 1 本の `CGPath`（複数サブパス）にする。**閉じない** —
/// 顔のパネル線のような線画用。
pub fn path_from_strokes(lines: &[Vec<(f64, f64)>]) -> CFRetained<CGMutablePath> {
    let p = CGMutablePath::new();
    for line in lines {
        for (i, &(x, y)) in line.iter().enumerate() {
            // SAFETY: 同上。
            unsafe {
                if i == 0 {
                    CGMutablePath::move_to_point(Some(&p), std::ptr::null(), x, y);
                } else {
                    CGMutablePath::add_line_to_point(Some(&p), std::ptr::null(), x, y);
                }
            }
        }
    }
    p
}

/// `CGMutablePath` を `CGPath` として借りる。`CAShapeLayer::setPath` は `&CGPath` を取る。
pub fn as_path(p: &CFRetained<CGMutablePath>) -> &CGPath {
    p
}

// ---------------------------------------------------------------------------
// テキストレイヤ
// ---------------------------------------------------------------------------

/// 等幅フォントのテキストレイヤを作る。
///
/// デザインは `JetBrains Mono` を使っているが、システムに無い前提で
/// **システムの等幅フォント**（SF Mono 系）にフォールバックする。字幅・字面が近く、
/// メニューバーの他の文字と馴染むので、フォントを同梱するより素直だと判断した。
///
/// `bold` は見出し・グリフ・バッジ用（デザインの `font-weight:600/700` に対応）。
pub fn text_layer(
    s: &str,
    size: f64,
    color: Rgb,
    alpha: f64,
    bold: bool,
    scale: f64,
) -> Retained<CATextLayer> {
    let layer = CATextLayer::new();
    layer.setFontSize(size);
    layer.setForegroundColor(Some(&cgcolor(color, alpha)));
    // Retina で 1x にラスタライズされてボケるのを防ぐ。
    layer.setContentsScale(scale);
    layer.setWrapped(false);

    // SAFETY: NSFontWeight* は AppKit の extern static（CGFloat）。
    let weight = unsafe {
        if bold {
            NSFontWeightBold
        } else {
            NSFontWeightMedium
        }
    };
    let font = NSFont::monospacedSystemFontOfSize_weight(size, weight);
    // CATextLayer.font は macOS では CTFont / CGFont / NSFont / フォント名文字列を受理する。
    // objc2 の型は CFType なので、NSFont* を同一 ABI の CFType 参照として渡す。
    // SAFETY: &NSFont と &CFType はどちらも不透明オブジェクトへの thin ポインタで ABI 互換。
    let font_cf: &CFType = unsafe { &*(&*font as *const NSFont as *const CFType) };
    // SAFETY: 上のとおり CATextLayer は NSFont インスタンスを font として受理する。
    unsafe { layer.setFont(Some(font_cf)) };

    set_text(&layer, s);
    layer
}

/// テキストレイヤの文字列を差し替える（レイヤを作り直さずに済ませるため）。
pub fn set_text(layer: &CATextLayer, s: &str) {
    let str_ns = ns(s);
    // SAFETY: CATextLayer は文字列を内部でコピーする。NSString の参照を渡すだけ。
    unsafe { layer.setString(Some(str_ns.as_ref() as &AnyObject)) };
}

/// テキストレイヤの色を差し替える。
pub fn set_text_color(layer: &CATextLayer, color: Rgb, alpha: f64) {
    layer.setForegroundColor(Some(&cgcolor(color, alpha)));
}

/// 中央揃えにする。
pub fn center_text(layer: &CATextLayer) {
    // SAFETY: kCAAlignmentCenter は QuartzCore の extern static（配置名の文字列定数）。
    let mode = unsafe { objc2_quartz_core::kCAAlignmentCenter };
    layer.setAlignmentMode(mode);
}

/// 末尾省略にする（セッション名が長いとき）。
pub fn truncate_end(layer: &CATextLayer) {
    // SAFETY: kCATruncationEnd は QuartzCore の extern static（省略位置の文字列定数）。
    let mode = unsafe { objc2_quartz_core::kCATruncationEnd };
    layer.setTruncationMode(mode);
}

// ---------------------------------------------------------------------------
// レイヤ
// ---------------------------------------------------------------------------

/// 角丸の単色レイヤ（目・バッジ・吹き出しの土台に使う）。
pub fn solid_layer(color: Rgb, alpha: f64, radius: f64) -> Retained<CALayer> {
    let l = CALayer::new();
    l.setBackgroundColor(Some(&cgcolor(color, alpha)));
    l.setCornerRadius(radius);
    l
}

/// レイヤに外向きのグローを付ける（CSS の `box-shadow: 0 0 Npx` 相当）。
pub fn set_glow(layer: &CALayer, color: Rgb, radius: f64, opacity: f32) {
    layer.setShadowColor(Some(&cgcolor(color, 1.0)));
    layer.setShadowRadius(radius);
    layer.setShadowOpacity(opacity);
    layer.setShadowOffset(NSSize {
        width: 0.0,
        height: 0.0,
    });
}
