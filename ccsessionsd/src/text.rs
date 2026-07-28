//! テキストの幅の見積もりと省略。**純関数のみ**（FFI を含まない ＝ テストできる）。
//!
//! テキストレイヤの幅を厳密に測るには `NSAttributedString::size` を使うが、ここで
//! 欲しいのは**バッジやカードの箱の幅**という粗い見積もりなので、字幅の係数で足りる。
//! 足りなければ末尾省略（`ffi::truncate_end`）が効く。

/// 半角 1 文字あたりのおおよその字幅（font-size 比）。SF Mono の advance は概ね 0.6 倍。
pub const NARROW_ADVANCE: f64 = 0.6;

/// 全角 1 文字あたりのおおよその字幅（font-size 比）。
///
/// **CJK を半角と同じ 0.6 で見積もってはいけない。** 日本語は 1 文字がほぼ
/// font-size ぶんの幅で組まれるので、0.6 では 4 割方小さく出る。セッション
/// タイトルもエージェントの役割ラベルも日本語なので、箱がテキストより狭くなって
/// 端が切れる。
pub const WIDE_ADVANCE: f64 = 1.0;

/// 全角（East Asian Wide / Fullwidth）として数える文字か。
///
/// Unicode の EastAsianWidth 全体を持ち込まず、**この UI に実際に出る範囲**だけを
/// 並べた粗い判定にしてある（日本語・中韓・全角記号・絵文字）。外れても数 pt の
/// 見積もり差にしかならない。
fn is_wide(c: char) -> bool {
    matches!(c as u32,
        0x1100..=0x115F        // ハングル字母
        | 0x2E80..=0x303E      // CJK 部首・記号・句読点（「、」「。」を含む）
        | 0x3041..=0x33FF      // かな・注音・互換
        | 0x3400..=0x4DBF      // CJK 拡張 A
        | 0x4E00..=0x9FFF      // CJK 統合漢字
        | 0xA000..=0xA4CF      // イ文字
        | 0xAC00..=0xD7A3      // ハングル音節
        | 0xF900..=0xFAFF      // CJK 互換漢字
        | 0xFE30..=0xFE4F      // CJK 互換形（縦組み用）
        | 0xFF00..=0xFF60      // 全角英数・記号
        | 0xFFE0..=0xFFE6      // 全角通貨記号など
        | 0x1F300..=0x1FAFF    // 絵文字
        | 0x20000..=0x3FFFD    // CJK 拡張 B 以降
    )
}

/// `s` を `size` で描いたときのおおよその幅（pt）。
pub fn text_width(s: &str, size: f64) -> f64 {
    s.chars()
        .map(|c| {
            if is_wide(c) {
                WIDE_ADVANCE
            } else {
                NARROW_ADVANCE
            }
        })
        .sum::<f64>()
        * size
}

/// `max_w`（pt）に収まるところまで切り、切ったら末尾に `…` を付ける。
///
/// `CATextLayer` の `truncationMode` に任せない: **幅はこちらが先に決めている**
/// （カードの大きさをテキスト幅から積み上げているので、レイヤ側で切られても
/// カードの幅は縮まらず、右側に空白が残る）。
///
/// 文字数ではなく見積もり幅で切るのは、全角と半角で 1 文字の幅が倍近く違うため。
/// 探索は文字境界での二分探索（`text_width` の呼び出しは数回で済む）。
pub fn ellipsize(text: &str, font: f64, max_w: f64) -> String {
    if text_width(text, font) <= max_w {
        return text.to_string();
    }
    let chars: Vec<char> = text.chars().collect();
    // 「n 文字 + …」が収まる最大の n を探す。収まらなければ 0（… だけ）。
    let (mut lo, mut hi) = (0usize, chars.len());
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        let candidate: String = chars[..mid].iter().collect::<String>() + "…";
        if text_width(&candidate, font) <= max_w {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    chars[..lo].iter().collect::<String>() + "…"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_is_measured_at_the_monospace_advance() {
        assert_eq!(text_width("abcd", 10.0), 4.0 * 6.0);
    }

    #[test]
    fn japanese_is_measured_at_full_width() {
        // 半角と同じ係数で見積もると箱がテキストより狭くなる。
        assert_eq!(text_width("設計待ち", 10.0), 4.0 * 10.0);
        assert!(text_width("設計待ち", 10.0) > text_width("abcd", 10.0));
    }

    #[test]
    fn mixed_text_adds_up_per_character() {
        // 8 半角 + 2 全角。**サンプルに製品名を使わない** — 名前が変わるたびに
        // 文字数がずれてこのテストが落ちる（実際に改名で落ちた）。
        assert_eq!(text_width("abcdefghの顔", 10.0), 8.0 * 6.0 + 2.0 * 10.0);
    }

    #[test]
    fn japanese_punctuation_counts_as_wide() {
        assert_eq!(text_width("、。", 10.0), 2.0 * 10.0);
    }

    #[test]
    fn text_that_fits_is_returned_unchanged() {
        assert_eq!(ellipsize("short", 10.0, 100.0), "short");
    }

    #[test]
    fn an_overlong_title_is_cut_and_marked_with_an_ellipsis() {
        // 全角 10 文字 = 100pt を 55pt に収める。「…」自身も幅を食う。
        let cut = ellipsize("あいうえおかきくけこ", 10.0, 55.0);
        assert!(cut.ends_with('…'), "{cut}");
        assert!(
            text_width(&cut, 10.0) <= 55.0,
            "{cut} must fit in the budget"
        );
        // 収まる範囲では欲張ること（1 文字ぶんの余りを残さない）。
        let one_more: String = "あいうえおかきくけこ"
            .chars()
            .take(cut.chars().count())
            .collect::<String>()
            + "…";
        assert!(text_width(&one_more, 10.0) > 55.0, "{cut} is too timid");
    }

    #[test]
    fn a_hopeless_budget_leaves_just_the_ellipsis() {
        assert_eq!(ellipsize("あいう", 10.0, 1.0), "…");
    }
}
