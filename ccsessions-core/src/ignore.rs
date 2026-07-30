//! セッションを一覧表示から外す条件（`config.toml` の `ignore`）。
//!
//! **これは表示のフィルタであって、セッションの生死ではない。** ここで弾いた
//! セッションもファイルはそのまま残り、`store::sweep` はこの判定を一切見ない
//! （消えるのは今までどおり pid か TTL で死んだときだけ）。`ccsessions list --all`
//! で全件見えるのも、データを触っていないことの裏返し。
//!
//! I/O を持たない。`~` の展開に使うホームディレクトリすら引数で受ける
//! （`parse_in`）ので、テストは本物の `$HOME` にも設定ファイルにも依存しない
//! — `store` / `config` の `*_in` と同じ流儀。
//!
//! # 条件の書き方
//!
//! **当てる相手は cwd だけ**で、書き方は 2 つしかない。
//!
//! | 書き方 | 意味 |
//! |---|---|
//! | `/Users/x/work/tmp` · `~/work/tmp` | ワイルドカードを含まない ＝ そのパスと**配下すべて**。**ディレクトリ境界で切る**ので `/a/foo` は `/a/foobar` に当たらない |
//! | `~/work/tmp/**` · `**/cron-jobs/**` | glob。`*` `?` は `/` をまたがず、`**` はまたぐ |
//!
//! 方言は glob の慣習どおり（gitignore / ripgrep と同じ）。**glob を書いたら
//! そのとおりに照合する** — 「配下も含む」暗黙の拡張はワイルドカードを 1 つも
//! 書かなかった条件にだけ与える。`~/work/tmp/*` は直下の 1 段だけで、配下まで
//! 含めたければ `~/work/tmp/**` と書く。そうしないと `*` と `**` の区別が無くなり、
//! glob を知っている人ほど裏切られる。
//!
//! **根から書かれていない条件は受け付けない。** 照合の相手は必ず絶対パスなので、
//! `cron-jobs` と書いても当たらない。当たらないものを黙って受けるのが一番たちが
//! 悪く（書いたのに何も隠れず、警告も出ない）、かといって gitignore のように
//! 「どの深さでも」と解釈すると、1 語書いただけで意図しない深さまで広く消える。
//! `**/cron-jobs/**` と明示させる。

use std::path::Path;

use crate::session::Session;

/// 1 条件の長さの上限。**glob の照合は最悪 O(パターン長 × 対象長)** なので、
/// poller が毎 tick 踏む経路に置く以上は上限を切っておく。
const MAX_PATTERN_LEN: usize = 512;

// ---------------------------------------------------------------------------
// パターン
// ---------------------------------------------------------------------------

/// glob の 1 セグメント（`/` で区切った 1 段）。
#[derive(Debug, Clone, PartialEq, Eq)]
enum Seg {
    /// `**` — 0 段以上のセグメントに当たる。
    DoubleStar,
    /// `*` / `?` を含みうる 1 段ぶんのパターン。文字単位で持つのは、
    /// マルチバイトなディレクトリ名でも `?` が 1 文字として振る舞うため。
    Pat(Vec<char>),
}

/// 条件の実体。
#[derive(Debug, Clone, PartialEq, Eq)]
enum Matcher {
    /// ワイルドカードを含まない条件。前方一致（セグメント単位なので自然に
    /// ディレクトリ境界で切れる）で、配下も含む。空なら「すべて」（条件 `/`）。
    Prefix(Vec<String>),
    /// glob。書いたとおりに照合する。
    Glob(Vec<Seg>),
}

/// 一覧から外す条件 1 つ。
///
/// 書かれた原文（`raw`）を持ち続けるのは、設定画面が「読んだ値を書き戻すと元に
/// 戻る」を保証しているため（`config::field_value` → `config::set_field`）。
/// 展開後のパスを書き戻すと、`~/work` と書いた設定が保存のたびに
/// `/Users/x/work` へ化ける。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IgnorePattern {
    raw: String,
    matcher: Matcher,
}

impl IgnorePattern {
    /// 書かれたとおりの原文。
    pub fn as_str(&self) -> &str {
        &self.raw
    }

    /// 条件をパースする。`~` の展開には本物のホームディレクトリを使う。
    pub fn parse(raw: &str) -> Result<Self, String> {
        Self::parse_in(raw, &crate::home_dir())
    }

    /// `home` を明示して受け取る版。テストはこちらを叩く。
    pub fn parse_in(raw: &str, home: &Path) -> Result<Self, String> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err("ignore: an empty rule is not allowed".to_string());
        }
        // **制御文字は入口で弾く。** パスの条件に紛れ込むのは常に事故（色付き
        // 出力からのコピペで ESC が混ざる等）で、通しても当たらない。加えて、
        // 通すと設定ファイルの書き出しが TOML の基本文字列の禁止文字に触れる
        // ため、`config::toml_escape` と二重に閉じておく。
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

        Ok(IgnorePattern {
            raw: trimmed.to_string(),
            matcher: path_matcher(trimmed, home)?,
        })
    }

    /// このセッションを一覧から外すか。
    pub fn matches(&self, s: &Session) -> bool {
        match &self.matcher {
            Matcher::Prefix(p) => path_prefix_matches(p, &s.cwd),
            // 文字列→文字列への変換はここで 1 回だけ行う。`match_segments` の中で
            // やるとバックトラックのたびに同じセグメントを変換し直す。
            Matcher::Glob(segs) => {
                let cwd: Vec<Vec<char>> = split_path(&s.cwd)
                    .into_iter()
                    .map(|seg| seg.chars().collect())
                    .collect();
                match_segments(segs, &cwd)
            }
        }
    }
}

/// 条件の形を決める。
fn path_matcher(body: &str, home: &Path) -> Result<Matcher, String> {
    let expanded = expand_home(body, home)?;
    if expanded.is_empty() {
        return Err("ignore: an empty rule is not allowed".to_string());
    }
    let segs = split_path(&expanded);
    // **`.` / `..` は弾く。** 通すとセグメント比較で永久に不一致になり、
    // 「書いたのに何も隠れないが、警告も出ない」という一番たちの悪い形になる
    // （実在の cwd に `.` セグメントは現れない）。
    if let Some(dot) = segs.iter().find(|s| **s == "." || **s == "..") {
        return Err(format!(
            "ignore: cannot resolve {dot:?} in {body:?} (write an absolute path, or one \
             starting with `~/`)"
        ));
    }
    // **根から書かれていない条件も弾く**（モジュール doc 参照）。`**` 始まりは
    // 「どの深さでも」を明示して書いた形なので通す。
    if !expanded.starts_with('/') && segs.first() != Some(&"**") {
        return Err(format!(
            "ignore: {body:?} is not anchored at the root (start it with `/` or `~/`, or \
             prefix it with `**/` as in `**/cron-jobs/**` to match at any depth)"
        ));
    }

    // ワイルドカードが無ければパスとして扱う。「配下も含む前方一致」は、glob を
    // 書いていない条件にだけ与える暗黙の拡張（モジュール doc 参照）。
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

// ---------------------------------------------------------------------------
// 条件の束
// ---------------------------------------------------------------------------

/// 一覧から外す条件の束。**いずれか 1 つに当たれば外す。**
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IgnoreRules {
    patterns: Vec<IgnorePattern>,
}

impl IgnoreRules {
    /// 1 条件ずつパースし、**壊れた条件はその行だけ捨てて理由を返す**。
    ///
    /// 全体を `Err` にしないのは、打ち間違い 1 つで設定ごと既定へ落ちるのを
    /// 避けるため（daemon は `config::load` が `Err` を返すと last-good か
    /// 組込みデフォルトへ落ちるので、他の設定変更まで巻き添えになる）。
    /// 呼び出し側（`config::load`）が理由をログに出す。
    pub fn parse_lines_in<S: AsRef<str>>(lines: &[S], home: &Path) -> (Self, Vec<String>) {
        let mut patterns = Vec::new();
        let mut errors = Vec::new();
        for line in lines {
            match IgnorePattern::parse_in(line.as_ref(), home) {
                Ok(p) => patterns.push(p),
                Err(e) => errors.push(e),
            }
        }
        (IgnoreRules { patterns }, errors)
    }

    /// 本物のホームディレクトリを使う版。
    ///
    /// **1 件も無ければホームディレクトリを見に行かない。** `crate::home_dir()` は
    /// 解決できなければ panic するので、`ignore` を書いていない設定を読むだけの
    /// 経路（hook を含む）に panic 面を増やさないため。hook は何があっても
    /// exit 0 で終わらなければならない。
    pub fn parse_lines<S: AsRef<str>>(lines: &[S]) -> (Self, Vec<String>) {
        if lines.is_empty() {
            return (Self::default(), Vec::new());
        }
        Self::parse_lines_in(lines, &crate::home_dir())
    }

    pub fn is_empty(&self) -> bool {
        self.patterns.is_empty()
    }

    pub fn len(&self) -> usize {
        self.patterns.len()
    }

    /// 書かれたとおりの条件を順に返す（設定の表示・書き出し用）。
    pub fn raw(&self) -> impl Iterator<Item = &str> {
        self.patterns.iter().map(IgnorePattern::as_str)
    }

    /// このセッションを一覧から外すか。
    pub fn matches(&self, s: &Session) -> bool {
        self.patterns.iter().any(|p| p.matches(s))
    }
}

// ---------------------------------------------------------------------------
// 照合
// ---------------------------------------------------------------------------

/// パスを空でないセグメントに割る。`/a//b/` → `["a", "b"]`。
fn split_path(p: &str) -> Vec<&str> {
    p.split('/').filter(|s| !s.is_empty()).collect()
}

/// 前方一致。**セグメント単位で比べる**ので、`/a/foo` は `/a/foobar` に当たらず、
/// 区切りの重複（`/a//foo`）や末尾の `/` も自然に吸収される。
///
/// `prefix` が空（条件が `/`）なら「すべて」。書けてしまう以上、当たると
/// 決めておく方が説明しやすい。
fn path_prefix_matches(prefix: &[String], cwd: &str) -> bool {
    let segs = split_path(cwd);
    prefix.len() <= segs.len() && prefix.iter().zip(&segs).all(|(a, b)| a == b)
}

/// セグメント列どうしの照合。`**` は 0 段以上に当たる。
///
/// **2 ポインタ法**（`**` の戻り先を 1 つだけ覚える）で書く。素朴な再帰だと
/// `**/a/**/a/**/b` のような条件で段数に対して指数時間になる。
fn match_segments(pats: &[Seg], texts: &[Vec<char>]) -> bool {
    let (mut p, mut t) = (0usize, 0usize);
    // `star` は直近の `**` の位置、`mark` はそこで消費を再開する位置。
    let (mut star, mut mark) = (usize::MAX, 0usize);
    while t < texts.len() {
        match pats.get(p) {
            Some(Seg::DoubleStar) => {
                star = p;
                mark = t;
                p += 1;
            }
            Some(Seg::Pat(pat)) if glob_chars(pat, &texts[t]) => {
                p += 1;
                t += 1;
            }
            _ if star != usize::MAX => {
                // 直近の `**` に 1 段よけいに食わせてやり直す。
                mark += 1;
                t = mark;
                p = star + 1;
            }
            _ => return false,
        }
    }
    // 余った `**` は 0 段に当たる（`**/cron-jobs/**` が `/x/cron-jobs` に当たる）。
    while matches!(pats.get(p), Some(Seg::DoubleStar)) {
        p += 1;
    }
    p == pats.len()
}

/// 1 段ぶんの glob。`*` は 0 文字以上、`?` はちょうど 1 文字。`/` は現れない。
///
/// こちらも 2 ポインタ法。`a*a*a*a*a*b` のような条件を素朴な再帰で書くと
/// 指数時間になり、設定ファイル経由とはいえ poller が毎 tick 踏む。
fn glob_chars(pat: &[char], text: &[char]) -> bool {
    let (mut p, mut t) = (0usize, 0usize);
    let (mut star, mut mark) = (usize::MAX, 0usize);
    while t < text.len() {
        match pat.get(p) {
            // **`*` の判定はリテラル一致より先。** 逆にすると、対象の側に `*` が
            // 実在したときに「たまたま同じ文字」として 1 文字消費してしまい、
            // 戻り先を覚えないまま進む（パターン `a*` が `a*b` に当たらなくなる）。
            Some('*') => {
                star = p;
                mark = t;
                p += 1;
            }
            Some('?') => {
                p += 1;
                t += 1;
            }
            Some(c) if *c == text[t] => {
                p += 1;
                t += 1;
            }
            _ if star != usize::MAX => {
                mark += 1;
                t = mark;
                p = star + 1;
            }
            _ => return false,
        }
    }
    while matches!(pat.get(p), Some('*')) {
        p += 1;
    }
    p == pat.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::SessionState;
    use std::path::PathBuf;

    fn home() -> PathBuf {
        PathBuf::from("/Users/tester")
    }

    fn session(cwd: &str) -> Session {
        Session {
            id: "s1".into(),
            name: Session::name_from_cwd(cwd),
            title: None,
            cwd: cwd.into(),
            state: SessionState::Working,
            since: 0,
            updated: 0,
            agents: vec![],
            main_stopped: false,
            error_kind: None,
            pid: None,
        }
    }

    /// 条件 1 つを cwd 1 つに当てる。テストの本文を表に近づけるための小道具。
    fn hits(pattern: &str, cwd: &str) -> bool {
        IgnorePattern::parse_in(pattern, &home())
            .unwrap_or_else(|e| panic!("{pattern:?} が読めない: {e}"))
            .matches(&session(cwd))
    }

    // ---- ワイルドカード無し（そのパスと配下） ---------------------------------

    #[test]
    fn a_plain_path_matches_itself_and_everything_below() {
        assert!(hits("/a/foo", "/a/foo"));
        assert!(hits("/a/foo", "/a/foo/b/c"));
    }

    /// **前方一致はディレクトリ境界で切る。** ここを文字列の `starts_with` だけで
    /// 書くと、`/a/foo` を無視したつもりで `/a/foobar` まで消える。
    #[test]
    fn a_plain_path_does_not_match_a_longer_sibling_name() {
        assert!(!hits("/a/foo", "/a/foobar"));
        assert!(!hits("/a/foo", "/a/fo"));
        assert!(!hits("/a/foo", "/b/a/foo"));
    }

    #[test]
    fn a_trailing_slash_does_not_change_a_plain_path() {
        assert!(hits("/a/foo/", "/a/foo"));
        assert!(hits("/a/foo", "/a/foo/"));
    }

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

    // ---- glob -----------------------------------------------------------------

    #[test]
    fn a_double_star_crosses_separators_in_both_directions() {
        assert!(hits("**/cron-jobs/**", "/Users/x/cron-jobs/a"));
        assert!(hits("**/cron-jobs/**", "/Users/x/cron-jobs/a/b/c"));
    }

    /// **末尾の `/**` は 0 段にも当たる。** そうしないと
    /// 「ディレクトリごと畳む」つもりで書いた条件が、そのディレクトリ自身に
    /// 走っているセッションだけ取りこぼす。
    #[test]
    fn a_trailing_double_star_also_matches_the_directory_itself() {
        assert!(hits("**/cron-jobs/**", "/Users/x/cron-jobs"));
    }

    /// **`*` は `/` をまたがない**（glob の慣習どおり）。ここが崩れると `*` と
    /// `**` の区別が消える。
    #[test]
    fn a_single_star_does_not_cross_separators() {
        assert!(hits("~/work/tmp/*", "/Users/tester/work/tmp/a"));
        assert!(!hits("~/work/tmp/*", "/Users/tester/work/tmp/a/b"));
        assert!(!hits("~/work/tmp/*", "/Users/tester/work/tmp"));
    }

    #[test]
    fn a_star_matches_a_whole_segment() {
        assert!(hits("/a/*/tmp", "/a/b/tmp"));
        assert!(!hits("/a/*/tmp", "/a/b/c/tmp"));
        assert!(!hits("/a/*/tmp", "/a/tmp"));
    }

    /// **対象の側に `*` が実在しても壊れない。** glob の判定をリテラル一致より
    /// あとに置くと、`*` を「たまたま同じ文字」として消費して戻り先を覚えず、
    /// `a*` が `a*b` に当たらなくなる。
    #[test]
    fn a_star_in_the_path_itself_does_not_confuse_the_matcher() {
        assert!(hits("/a/x*", "/a/x*y"));
        assert!(hits("/a/x*", "/a/x*"));
        assert!(hits("/a/*", "/a/*"));
        assert!(!hits("/a/x*", "/a/y*z"));
    }

    #[test]
    fn a_question_mark_matches_exactly_one_character() {
        assert!(hits("/a/?", "/a/b"));
        assert!(!hits("/a/?", "/a/bc"));
        assert!(!hits("/a/?", "/a"));
    }

    /// マルチバイトでも `?` は 1 文字。バイト単位で書くと壊れる。
    #[test]
    fn a_question_mark_counts_characters_not_bytes() {
        assert!(hits("/x/??", "/x/定期"));
        assert!(!hits("/x/?", "/x/定期"));
    }

    /// **glob は書いたとおり。** 配下まで含めたければ `**` を書く。
    #[test]
    fn a_glob_is_not_silently_extended_to_the_subtree() {
        assert!(hits("**/cron-*", "/Users/x/cron-jobs"));
        assert!(!hits("**/cron-*", "/Users/x/cron-jobs/a"));
        assert!(hits("**/cron-*/**", "/Users/x/cron-jobs/a"));
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

    /// **相対の条件は弾く。** 照合の相手は必ず絶対パスなので当たらない。黙って
    /// 受けると「書いたのに何も隠れず、警告も出ない」になり、gitignore のように
    /// 「どの深さでも」と解釈すると 1 語で意図せず広く消える。`**/` を明示させる。
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

    /// **制御文字は入口で弾く。** 通すと当たらないうえ、設定ファイルの
    /// 書き出しが TOML の禁止文字に触れる（`config::toml_escape` と二重に閉じる）。
    #[test]
    fn a_pattern_containing_a_control_character_is_refused() {
        for c in ['\u{1B}', '\u{00}', '\u{0B}', '\u{7F}', '\n'] {
            let src = format!("/tmp/a{c}b");
            let err = IgnorePattern::parse_in(&src, &home())
                .expect_err(&format!("U+{:04X} が通ってしまった", c as u32));
            assert!(err.contains("control character"), "理由が分からない: {err}");
        }
    }

    /// **`.` / `..` は弾く。** 通すとセグメント比較で永久に不一致になり、
    /// 「書いたのに何も隠れないが警告も出ない」という一番たちの悪い形になる。
    #[test]
    fn a_dot_segment_is_refused_instead_of_silently_never_matching() {
        for src in ["./cron-jobs", "../x", "/a/./b", "~/x/../y"] {
            assert!(
                IgnorePattern::parse_in(src, &home()).is_err(),
                "{src:?} が通ってしまった（黙って何にも当たらなくなる）"
            );
        }
    }

    /// 区切りが重なった cwd でも前方一致が外れない。セグメント比較にしてある
    /// ので、glob 側（同じ `split_path` を通る）と挙動がずれない。
    #[test]
    fn a_doubled_separator_in_the_cwd_does_not_break_a_prefix() {
        assert!(hits("/a/foo", "/a//foo/b"));
        assert!(hits("/a/foo", "//a/foo"));
        assert!(!hits("/a/foo", "/a//foobar"));
    }

    /// ホームが `/` のとき、条件 `~` は「すべて」。空文字になって
    /// 「空の条件」に化けてはいけない。
    #[test]
    fn a_tilde_still_works_when_home_is_the_root() {
        let p = IgnorePattern::parse_in("~", &PathBuf::from("/")).unwrap();
        assert!(p.matches(&session("/anywhere/at/all")));
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

    /// **原文を保つ。** 展開後のパスを保存し直すと、`~/work` と書いた設定が
    /// 保存のたびに `/Users/tester/work` へ化ける。
    #[test]
    fn the_written_form_is_preserved_for_round_tripping() {
        for src in ["~/work/tmp", "**/cron-jobs/**", "/a/foo"] {
            assert_eq!(IgnorePattern::parse_in(src, &home()).unwrap().as_str(), src);
        }
    }

    // ---- 束 -------------------------------------------------------------------

    #[test]
    fn any_matching_pattern_hides_the_session() {
        let (rules, errors) =
            IgnoreRules::parse_lines_in(&["/nope", "**/cron-jobs/**", "/also-nope"], &home());
        assert!(errors.is_empty());
        assert_eq!(rules.len(), 3);
        assert!(rules.matches(&session("/Users/x/cron-jobs/a")));
        assert!(!rules.matches(&session("/Users/x/proj")));
    }

    /// **壊れた条件はその行だけ捨てる。** 打ち間違い 1 つで設定ごと既定へ
    /// 落とさないための保証。
    #[test]
    fn a_broken_pattern_is_dropped_and_the_rest_still_applies() {
        let (rules, errors) = IgnoreRules::parse_lines_in(&["~alice/x", "/a/foo", ""], &home());
        assert_eq!(errors.len(), 2, "壊れた 2 件の理由が返ること: {errors:?}");
        assert_eq!(rules.len(), 1);
        assert!(rules.matches(&session("/a/foo")));
    }

    #[test]
    fn an_empty_rule_set_matches_nothing() {
        let rules = IgnoreRules::default();
        assert!(rules.is_empty());
        assert!(!rules.matches(&session("/anywhere/at/all")));
    }

    // ---- 計算量 ---------------------------------------------------------------

    /// **素朴な再帰だと指数時間になる条件**が、実用時間で返ること。
    /// poller が毎 tick 踏む経路なので、ここが崩れると常駐が固まる。
    #[test]
    fn a_pathological_glob_still_returns_quickly() {
        let start = std::time::Instant::now();
        let p = IgnorePattern::parse_in("/a*a*a*a*a*a*a*a*a*b", &home()).unwrap();
        assert!(!p.matches(&session(&format!("/{}", "a".repeat(64)))));
        let deep = IgnorePattern::parse_in("**/a/**/a/**/a/**/a/**/b", &home()).unwrap();
        let long_path: String = std::iter::repeat_n("/a", 64).collect();
        assert!(!deep.matches(&session(&long_path)));
        assert!(
            start.elapsed() < std::time::Duration::from_millis(500),
            "glob の照合に {:?} かかった（バックトラックが爆発している）",
            start.elapsed()
        );
    }
}
