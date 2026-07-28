//! 顔の描画で **CALayer と SVG の両方が要る**寸法定数。
//!
//! 色（`palette`）と同じ理由でここに置いてある: `ccsessions` CLI（`ccsessionsd` に
//! 依存しない）が SVG プレビューを描くので、`ccsessionsd/src/theme.rs` にあると
//! 手が届かない。値を 2 か所に持つと SVG が「CALayer の忠実なプレビュー」でなくなる。
//!
//! **顔ごとには変えられない**。`theme.rs` から re-export しているので、
//! デザインのイテレーションで探す場所は今までどおり `theme.rs` でよい。

use crate::face::Size;

/// 体の枠線幅（pt）。元デザイン: `1.5px`
pub const BORDER_W: f64 = 1.5;

/// パネル線の太さ（pt）。体の枠（`BORDER_W`）より細くして輪郭を主役に保つ。
pub fn detail_line_w(size: Size) -> f64 {
    if size.is_bar() {
        0.8
    } else {
        1.0
    }
}

/// パネル線の濃さ。輪郭と同じアクセント色を薄く使う。
pub const DETAIL_ALPHA: f64 = 0.62;
