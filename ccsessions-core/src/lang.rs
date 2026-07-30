//! 表示言語と対訳。
//!
//! **対訳は [`L`] で持つ。** 片方の言語だけ書いた状態はコンパイルが通らないので、
//! 訳し忘れが実行時ではなくビルド時に落ちる。別ファイルの辞書にしないのは、
//! キーの二重管理（辞書には有るが誰も引かない・引いているが辞書に無い）を
//! 避けるため — 設定スキーマを `config::fields()` の 1 か所に集めているのと同じ理由。
//!
//! **ここに載るのは画面に出る文言だけ。** `ccsessions doctor` の診断と顔の検証
//! メッセージは英語に一本化してあるので `L` を通さない
//! （[ADR 0025](../../docs/adr/0025-ui-is-bilingual-diagnostics-are-english.md)）。

/// 解決済みの表示言語。
///
/// **`auto` はここには無い。** 設定の `auto` は [`crate::config::Language::resolve`]
/// が OS の言語タグを見て潰したあとの形で、描画側はもう迷わない。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    Ja,
    En,
}

impl Lang {
    /// `<html lang>` 属性や JSON に載せるときの表現。
    pub fn as_str(&self) -> &'static str {
        match self {
            Lang::Ja => "ja",
            Lang::En => "en",
        }
    }
}

/// 対訳のペア。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct L {
    pub ja: &'static str,
    pub en: &'static str,
}

impl L {
    pub const fn get(&self, lang: Lang) -> &'static str {
        match lang {
            Lang::Ja => self.ja,
            Lang::En => self.en,
        }
    }
}

/// 対訳を書くときの短縮形。`l("配置", "Placement")`。
///
/// `const fn` なので、既存の `&'static str` の const 表をそのまま `L` の表に
/// 差し替えられる（`config::fields()` や `parts::LINES` はどちらも const）。
pub const fn l(ja: &'static str, en: &'static str) -> L {
    L { ja, en }
}

/// OS / ブラウザが出す言語タグから表示言語を決める。
///
/// 受け取る文字列の形は層によって違うので、幅を持たせてある:
/// `"ja"`・`"ja-JP"`（NSLocale）・`"ja_JP.UTF-8"`（環境変数 `LANG`）・
/// `"ja,en-US;q=0.9"`（HTTP の `Accept-Language`）のどれでも `Ja` になる。
/// 複数並んでいるときは**先頭だけ**見る（q 値の比較はしない。`ja` か否かの
/// 2 択で、送り手は希望順に並べてくるため）。
///
/// **判断が付かなければ `En` に倒す。** 日本語だと確信できるときだけ日本語にする
/// — 英語話者の画面に日本語が出る事故の方が、その逆より困るので。
pub fn from_tag(tag: &str) -> Lang {
    let first = tag.split(',').next().unwrap_or("").trim();
    let primary = first.split(['-', '_', '.', ';']).next().unwrap_or("");
    if primary.eq_ignore_ascii_case("ja") {
        Lang::Ja
    } else {
        Lang::En
    }
}

/// 環境変数から言語タグを拾う。**CLI 用**。
///
/// POSIX の優先順（`LC_ALL` > `LC_MESSAGES` > `LANG`）に従う。空文字は「設定
/// されていない」として次を見る — シェルが `LANG=` と空で渡してくることがある。
///
/// **daemon はこれを使わない。** launchd から起動される常駐はロケール系の環境変数を
/// 継承しないので、`ccsessionsd` 側は `NSLocale` に訊く。
pub fn env_tag() -> Option<String> {
    ["LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .find_map(|k| std::env::var(k).ok().filter(|v| !v.trim().is_empty()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pair_answers_in_the_language_it_is_asked_for() {
        let p = l("配置", "Placement");
        assert_eq!(p.get(Lang::Ja), "配置");
        assert_eq!(p.get(Lang::En), "Placement");
    }

    #[test]
    fn japanese_is_recognised_in_every_shape_the_platform_hands_us() {
        for tag in [
            "ja",
            "ja-JP",
            "ja_JP.UTF-8",
            "ja,en-US;q=0.9",
            " ja-JP ",
            "JA",
        ] {
            assert_eq!(
                from_tag(tag),
                Lang::Ja,
                "{tag:?} should resolve to Japanese"
            );
        }
    }

    #[test]
    fn anything_we_cannot_read_as_japanese_falls_back_to_english() {
        for tag in [
            "en",
            "en-US",
            "en_US.UTF-8",
            "en-US,ja;q=0.9",
            "C",
            "POSIX",
            "fr-FR",
            "",
            "   ",
        ] {
            assert_eq!(from_tag(tag), Lang::En, "{tag:?} should resolve to English");
        }
    }

    #[test]
    fn only_the_first_tag_decides() {
        // ブラウザは希望順に並べる。2 番目に ja があっても英語のまま。
        assert_eq!(from_tag("en-GB,ja;q=0.8,fr;q=0.5"), Lang::En);
        assert_eq!(from_tag("ja-JP,en;q=0.8"), Lang::Ja);
    }
}
