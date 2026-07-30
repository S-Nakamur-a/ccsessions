//! セッションを一覧表示から外す条件（`config.toml` の `ignore`）。
//!
//! **これは表示のフィルタであって、セッションの生死ではない。** 弾いた
//! セッションもファイルはそのまま残り、`store::sweep` はこの判定を一切見ない
//! （消えるのは今までどおり pid か TTL で死んだときだけ）。`ccsessions list --all`
//! で全件見えるのも、データを触っていないことの裏返し。
//!
//! I/O を持たない。`~` の展開に使うホームディレクトリすら引数で受ける
//! （`parse_in`）ので、テストは本物の `$HOME` にも設定ファイルにも依存しない
//! — `store` / `config` の `*_in` と同じ流儀。
//!
//! 中身は 2 段に分かれる。[`parse`] が原文を検証して照合器に落とし、[`glob`] が
//! その照合器を cwd に当てる。`glob` は `Session` も設定も知らない。
//!
//! # 条件の書き方
//!
//! 当てる相手は cwd だけで、書き方は 2 つしかない。
//!
//! | 書き方 | 意味 |
//! |---|---|
//! | `/Users/x/work/tmp` · `~/work/tmp` | ワイルドカード無し ＝ そのパスと配下すべて。ディレクトリ境界で切るので `/a/foo` は `/a/foobar` に当たらない |
//! | `~/work/tmp/**` · `**/cron-jobs/**` | glob。`*` `?` は `/` をまたがず、`**` はまたぐ（gitignore / ripgrep と同じ方言） |
//!
//! glob は書いたとおりに照合する。`~/work/tmp/*` は直下の 1 段だけで、配下まで
//! 含めたければ `**` と書く — 「配下も含む」の暗黙の拡張を glob にも与えると
//! `*` と `**` の区別が消え、glob を知っている人ほど裏切られる。
//!
//! 根から書かれていない条件は受け付けない。照合の相手は必ず絶対パスなので
//! `cron-jobs` の 1 語では当たらず、当たらない条件を黙って受けるのが一番たちが
//! 悪い（書いたのに何も隠れず、警告も出ない）。かといって gitignore のように
//! 「どの深さでも」と解釈すると、1 語書いただけで意図しない深さまで消える。
//! `**/cron-jobs/**` と明示させる。

mod glob;
mod parse;

use std::path::Path;

use crate::session::Session;
use glob::Matcher;

/// 一覧から外す条件 1 つ。
///
/// 展開後のパスではなく書かれた原文（`raw`）を持ち続ける。展開後を持つと、
/// 設定画面が読んだ値を書き戻すたびに `~/work` が `/Users/x/work` へ化ける
/// （`config::field_value` → `config::set_field`）。
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
        let (raw, matcher) = parse::compile(raw, home)?;
        Ok(IgnorePattern { raw, matcher })
    }

    /// このセッションを一覧から外すか。
    pub fn matches(&self, s: &Session) -> bool {
        self.matcher.matches(&s.cwd)
    }
}

/// 一覧から外す条件の束。いずれか 1 つに当たれば外す。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IgnoreRules {
    patterns: Vec<IgnorePattern>,
}

impl IgnoreRules {
    /// 1 条件ずつパースし、**壊れた条件はその行だけ捨てて理由を返す**。
    ///
    /// 全体を `Err` にしないのは、打ち間違い 1 つで設定ごと既定へ落ちるのを
    /// 避けるため — daemon は `config::load` が `Err` を返すと last-good か
    /// 組込みデフォルトへ落ちるので、無関係な設定変更まで巻き添えになる。
    /// 捨てた理由は呼び出し側（`config::load`）がログに出す。
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
    /// 経路（hook を含む）に panic 面を増やさない。hook は何があっても exit 0。
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

/// `parse` / `glob` のテストからも使う小道具。
#[cfg(test)]
mod testing {
    use super::*;
    use crate::session::SessionState;
    use std::path::PathBuf;

    pub(super) fn home() -> PathBuf {
        PathBuf::from("/Users/tester")
    }

    pub(super) fn session(cwd: &str) -> Session {
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

    /// 条件 1 つを cwd 1 つに当てる。テストの本文を書き方の表に近づけるための小道具。
    pub(super) fn hits(pattern: &str, cwd: &str) -> bool {
        IgnorePattern::parse_in(pattern, &home())
            .unwrap_or_else(|e| panic!("{pattern:?} が読めない: {e}"))
            .matches(&session(cwd))
    }
}

#[cfg(test)]
mod tests {
    use super::testing::{home, session};
    use super::*;

    #[test]
    fn any_matching_pattern_hides_the_session() {
        let (rules, errors) =
            IgnoreRules::parse_lines_in(&["/nope", "**/cron-jobs/**", "/also-nope"], &home());
        assert!(errors.is_empty());
        assert_eq!(rules.len(), 3);
        assert!(rules.matches(&session("/Users/x/cron-jobs/a")));
        assert!(!rules.matches(&session("/Users/x/proj")));
    }

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

    /// 展開後を保存し直すと、`~/work` が保存のたびに `/Users/tester/work` へ化ける。
    #[test]
    fn the_written_form_is_preserved_for_round_tripping() {
        for src in ["~/work/tmp", "**/cron-jobs/**", "/a/foo"] {
            assert_eq!(IgnorePattern::parse_in(src, &home()).unwrap().as_str(), src);
        }
    }
}
