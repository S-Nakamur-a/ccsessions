//! 状態パレット（アクセント色・面の塗り・目の色・減光率）。
//!
//! `ccsessions`（CLI）が `ccsessionsd` に依存せずに顔の SVG プレビューを描けるよう、
//! ここへ色を置く。色を 2 か所に持つと単一の真実が壊れるので、
//! **既定色の定義はここが唯一の場所**にする。`ccsessionsd/src/theme.rs` は
//! ここから re-export するので、`theme.rs` を見れば色が分かる状態は保つ。
//!
//! ここにあるのは**既定のパレット**。顔は `[colors.<状態>]` で状態ごとに
//! 上書きできる（`face::spec::StateColors`）ので、描く側は `palette` を直に
//! 引かず `FaceSpec::accent` / `FaceSpec::fill` / `FaceSpec::eye` を通すこと。

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

// ---------------------------------------------------------------------------
// 16 進表記との変換（顔の `[colors.*]` と SVG 出力が共有する）
// ---------------------------------------------------------------------------

/// `#rrggbb`（`#rgb` の短縮も可）を sRGB 成分にする。読めなければ `None`。
///
/// 前後の空白と大文字は許す（手で設定ファイルを書いたときに叱らないため）。
/// `#` の省略は許さない — 色だと分かる書き方を 1 つに保つ。
pub fn parse_hex(s: &str) -> Option<Rgb> {
    let h = s.trim().strip_prefix('#')?;
    if !h.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let byte = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).ok();
    let (r, g, b) = match h.len() {
        6 => (byte(0)?, byte(2)?, byte(4)?),
        // `#abc` は `#aabbcc`。CSS と同じ短縮。
        3 => {
            let d = |i: usize| u8::from_str_radix(&h[i..i + 1], 16).ok().map(|v| v * 17);
            (d(0)?, d(1)?, d(2)?)
        }
        _ => return None,
    };
    Some((r as f64 / 255.0, g as f64 / 255.0, b as f64 / 255.0))
}

/// sRGB 成分（0..1）を `#rrggbb` にする。範囲外はクランプする。
pub fn to_hex(c: Rgb) -> String {
    let b = |v: f64| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!("#{:02x}{:02x}{:02x}", b(c.0), b(c.1), b(c.2))
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

    #[test]
    fn a_hex_colour_round_trips() {
        for s in ["#000000", "#ffffff", "#7f3ac2", "#0b0912"] {
            assert_eq!(to_hex(parse_hex(s).unwrap()), s);
        }
    }

    /// 手で書いたときに叱らない範囲（大文字・前後の空白・3 桁）。
    #[test]
    fn upper_case_padding_and_the_short_form_are_accepted() {
        assert_eq!(parse_hex("#FF0000"), parse_hex("#ff0000"));
        assert_eq!(parse_hex("  #ff0000  "), parse_hex("#ff0000"));
        assert_eq!(parse_hex("#f00"), parse_hex("#ff0000"));
        assert_eq!(parse_hex("#abc"), parse_hex("#aabbcc"));
    }

    /// 読めない値は `None`。ここで弾くので、描画側に壊れた色は届かない。
    #[test]
    fn a_malformed_colour_is_rejected() {
        for s in ["", "#", "auto", "ff0000", "#gggggg", "#12345", "#1234567"] {
            assert_eq!(parse_hex(s), None, "{s:?} を色として受けてはいけない");
        }
    }

    /// 範囲外の成分でも `to_hex` は 2 桁 16 進を返す（描画が壊れない番人）。
    #[test]
    fn out_of_range_components_are_clamped() {
        assert_eq!(to_hex((-1.0, 2.0, f64::NAN)), "#00ff00");
    }
}
