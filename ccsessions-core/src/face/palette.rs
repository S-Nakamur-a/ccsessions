//! 状態パレット（アクセント色・面の塗り・目の色・減光率）。
//!
//! `ccsessions`（CLI）が `ccsessionsd` に依存せずに顔の SVG プレビューを描けるよう、
//! ここへ色を置く。色を 2 か所に持つと単一の真実が壊れるので、
//! **色の定義はここが唯一の場所**にする。顔ごとに色は変えられない（状態と色の
//! 対応が読み取りやすさの核なので）。`ccsessionsd/src/theme.rs` はここから
//! re-export するので、`theme.rs` を見れば色が分かる状態は保つ。

use crate::session::SessionState;

/// sRGB 成分（0.0–1.0）。alpha は使う側で足す。
pub type Rgb = (f64, f64, f64);

/// 状態のアクセント色（枠・グロー・グリフ・バッジ枠・名前）。
/// 元: working `oklch(0.82 0.15 195)` / wait_user `oklch(0.84 0.15 82)` /
/// wait_agent `oklch(0.72 0.15 285)` / idle `oklch(0.60 0.02 260)` /
/// done `oklch(0.80 0.16 152)` / error `oklch(0.64 0.16 25)`
pub fn accent(s: SessionState) -> Rgb {
    match s {
        SessionState::Working => (0.0000, 0.8836, 0.8863),
        SessionState::WaitUser => (0.9839, 0.7520, 0.2679),
        SessionState::WaitAgent => (0.6116, 0.5843, 0.9957),
        SessionState::Idle => (0.4758, 0.5046, 0.5510),
        SessionState::Done => (0.3768, 0.8571, 0.5371),
        SessionState::Error => (0.8642, 0.3679, 0.3474),
    }
}

/// 面の塗り。`color-mix(in oklch, accent 22%, #0b0912)` の解。
/// アクセントより遥かに暗く、状態ごとの色味だけがわずかに残る（枠とグローで状態を読ませ、
/// 面は「体の中身」として沈ませるのが元デザインの狙い）。
pub fn face_fill(s: SessionState) -> Rgb {
    match s {
        SessionState::Working => (0.1425, 0.1682, 0.2698),
        SessionState::WaitUser => (0.2292, 0.1447, 0.2292),
        SessionState::WaitAgent => (0.1562, 0.1354, 0.2385),
        SessionState::Idle => (0.1255, 0.1239, 0.1646),
        SessionState::Done => (0.1177, 0.1691, 0.2694),
        SessionState::Error => (0.1717, 0.1062, 0.2032),
    }
}

/// 生き物の地の暗色（バッジ背景・体の内側の基準）。元: `#0b0912`
pub const INK: Rgb = (0.0431, 0.0353, 0.0706);
/// 目の明色。元: `#eef2ff`
pub const EYE: Rgb = (0.9333, 0.9490, 1.0000);
/// 閉じた目（idle）の色。元: `#c6ccdb`
pub const EYE_CLOSED: Rgb = (0.7765, 0.8000, 0.8588);
/// エラー時の目。元: `oklch(0.82 0.13 25)`
pub const EYE_ERROR: Rgb = (1.0000, 0.6335, 0.6006);

/// idle / done の減光。元: `opacity:.55` / `opacity:.9`
pub fn face_opacity(s: SessionState) -> f32 {
    match s {
        SessionState::Idle => 0.55,
        SessionState::Done => 0.90,
        _ => 1.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// idle と done だけが減光され、他は等倍。
    #[test]
    fn only_idle_and_done_are_dimmed() {
        for s in SessionState::ORDER {
            let o = face_opacity(s);
            match s {
                SessionState::Idle => assert_eq!(o, 0.55),
                SessionState::Done => assert_eq!(o, 0.90),
                _ => assert_eq!(o, 1.0),
            }
        }
    }
}
