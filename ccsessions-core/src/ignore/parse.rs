//! 原文 → 照合器。書き方と、なぜその書き方に絞ったかはモジュール doc。

use std::path::Path;

use super::glob::{split_path, Matcher, Seg};

/// 1 条件の長さの上限。照合は最悪 O(パターン長 × 対象長) で、poller が毎 tick
/// 踏む経路に置く以上は上限を切っておく。
const MAX_PATTERN_LEN: usize = 512;

/// 原文を検証して、前後の空白を落とした原文と照合器を返す。
pub(super) fn compile(raw: &str, home: &Path) -> Result<(String, Matcher), String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("ignore: an empty rule is not allowed".to_string());
    }
    // 制御文字が条件に紛れ込むのは常に事故（色付き出力からのコピペで ESC が
    // 混ざる等）で、通しても当たらない。書き出しの側でも `config::toml_escape`
    // が逃がすが、入口でも弾いて二重に閉じておく。
    if let Some(c) = trimmed.chars().find(|c| c.is_control()) {
        return Err(format!(
            "ignore: the rule contains a control character (U+{:04X}): {trimmed:?}",
            c as u32
        ));
    }
    if trimmed.chars().count() > MAX_PATTERN_LEN {
        return Err(format!(
            "ignore: the rule is too long (at most {MAX_PATTERN_LEN} characters): {:?}…",
            trimmed.chars().take(32).collect::<String>()
        ));
    }
    Ok((trimmed.to_string(), path_matcher(trimmed, home)?))
}

/// 条件の形を決める。
fn path_matcher(body: &str, home: &Path) -> Result<Matcher, String> {
    let expanded = expand_home(body, home)?;
    if expanded.is_empty() {
        return Err("ignore: an empty rule is not allowed".to_string());
    }
    let segs = split_path(&expanded);
    // 実在の cwd に `.` / `..` セグメントは現れないので、通しても永久に不一致。
    if let Some(dot) = segs.iter().find(|s| **s == "." || **s == "..") {
        return Err(format!(
            "ignore: cannot resolve {dot:?} in {body:?} (write an absolute path, or one \
             starting with `~/`)"
        ));
    }
    // `**` 始まりは「どの深さでも」を明示して書いた形なので、根から書かれて
    // いなくても通す。
    if !expanded.starts_with('/') && segs.first() != Some(&"**") {
        return Err(format!(
            "ignore: {body:?} is not anchored at the root (start it with `/` or `~/`, or \
             prefix it with `**/` as in `**/cron-jobs/**` to match at any depth)"
        ));
    }

    if !expanded.contains('*') && !expanded.contains('?') {
        return Ok(Matcher::Prefix(
            segs.into_iter().map(str::to_string).collect(),
        ));
    }
    Ok(Matcher::Glob(
        segs.into_iter()
            .map(|s| {
                if s == "**" {
                    Seg::DoubleStar
                } else {
                    Seg::Pat(s.chars().collect())
                }
            })
            .collect(),
    ))
}

/// 先頭の `~` を展開する。`~user` は展開しない（誰の home か決められない）。
fn expand_home(body: &str, home: &Path) -> Result<String, String> {
    if body == "~" || body.starts_with("~/") {
        let home = home.to_string_lossy();
        let rest = body.strip_prefix('~').unwrap_or("");
        let expanded = format!("{}{}", home.trim_end_matches('/'), rest);
        // ホーム自体が `/` のとき、条件 `~` は「すべて」を意味する。空文字に
        // なると「空の条件」として弾かれてしまうので、根に戻しておく。
        return Ok(if expanded.is_empty() {
            "/".to_string()
        } else {
            expanded
        });
    }
    if body.starts_with('~') {
        return Err(format!(
            "ignore: cannot expand `~` in {body:?} (start it with `~/`, or write an absolute path)"
        ));
    }
    Ok(body.to_string())
}

#[cfg(test)]
mod tests {
    use super::super::testing::{hits, home, session};
    use super::super::IgnorePattern;
    use super::*;
    use std::path::PathBuf;

    // ---- `~` の展開 -----------------------------------------------------------

    #[test]
    fn a_tilde_expands_to_the_home_directory() {
        assert!(hits("~/work/tmp", "/Users/tester/work/tmp/x"));
        assert!(!hits("~/work/tmp", "/other/work/tmp"));
    }

    #[test]
    fn a_bare_tilde_matches_everything_under_home() {
        assert!(hits("~", "/Users/tester/anything"));
        assert!(!hits("~", "/opt/elsewhere"));
    }

    /// `~user` は誰の home か決められないので受け付けない（黙って別物に
    /// 当てるより、読めないと言う方が安全）。
    #[test]
    fn a_named_tilde_is_refused() {
        let err = IgnorePattern::parse_in("~alice/work", &home()).unwrap_err();
        assert!(err.contains('~'), "理由が分からない: {err}");
    }

    /// ホームが `/` で終わっていても区切りが二重にならない。
    #[test]
    fn a_home_with_a_trailing_slash_does_not_double_the_separator() {
        let p = IgnorePattern::parse_in("~/work", &PathBuf::from("/Users/tester/")).unwrap();
        assert!(p.matches(&session("/Users/tester/work/x")));
    }

    /// ホームが `/` のとき、条件 `~` は「すべて」。空文字になって
    /// 「空の条件」に化けてはいけない。
    #[test]
    fn a_tilde_still_works_when_home_is_the_root() {
        let p = IgnorePattern::parse_in("~", &PathBuf::from("/")).unwrap();
        assert!(p.matches(&session("/anywhere/at/all")));
    }

    // ---- 不正な条件 -----------------------------------------------------------

    #[test]
    fn an_empty_pattern_is_refused() {
        for src in ["", "   ", "\t"] {
            assert!(
                IgnorePattern::parse_in(src, &home()).is_err(),
                "{src:?} が通ってしまった"
            );
        }
    }

    /// 弾く理由はモジュール doc。ここが固定するのは、エラーが書き直し方
    /// （`**/`）まで言うこと — 言わないと「なぜ通らないのか」で詰む。
    #[test]
    fn a_relative_pattern_is_refused_and_points_at_the_double_star_form() {
        for src in ["cron-jobs", "work/tmp", "cron-*", "*foo*"] {
            let err = IgnorePattern::parse_in(src, &home())
                .expect_err(&format!("{src:?} が通ってしまった"));
            assert!(err.contains("**/"), "書き直し方が分からない: {err}");
        }
        // `**/` を付ければ通る。
        assert!(IgnorePattern::parse_in("**/cron-jobs/**", &home()).is_ok());
        assert!(IgnorePattern::parse_in("**/*foo*/**", &home()).is_ok());
    }

    /// TOML の基本文字列が生では許さない範囲まで含めて弾くこと
    /// （書き出し側の `config::toml_escape` と二重に閉じている）。
    #[test]
    fn a_pattern_containing_a_control_character_is_refused() {
        for c in ['\u{1B}', '\u{00}', '\u{0B}', '\u{7F}', '\n'] {
            let src = format!("/tmp/a{c}b");
            let err = IgnorePattern::parse_in(&src, &home())
                .expect_err(&format!("U+{:04X} が通ってしまった", c as u32));
            assert!(err.contains("control character"), "理由が分からない: {err}");
        }
    }

    /// 通すと永久に不一致 ＝ 「書いたのに何も隠れず、警告も出ない」になる。
    #[test]
    fn a_dot_segment_is_refused_instead_of_silently_never_matching() {
        for src in ["./cron-jobs", "../x", "/a/./b", "~/x/../y"] {
            assert!(
                IgnorePattern::parse_in(src, &home()).is_err(),
                "{src:?} が通ってしまった（黙って何にも当たらなくなる）"
            );
        }
    }

    #[test]
    fn an_overlong_pattern_is_refused() {
        let long = format!("/{}", "a".repeat(MAX_PATTERN_LEN));
        assert!(IgnorePattern::parse_in(&long, &home()).is_err());
        let ok = format!("/{}", "a".repeat(MAX_PATTERN_LEN - 1));
        assert!(IgnorePattern::parse_in(&ok, &home()).is_ok());
    }

    /// 前後の空白は落とす（設定ファイルの整形で条件が変わらない）。
    #[test]
    fn surrounding_whitespace_is_trimmed() {
        let p = IgnorePattern::parse_in("  /a/foo  ", &home()).unwrap();
        assert_eq!(p.as_str(), "/a/foo");
        assert!(p.matches(&session("/a/foo/b")));
    }
}
