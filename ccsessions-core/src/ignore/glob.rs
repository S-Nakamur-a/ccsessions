//! 照合器と、パスへの当て方。ここは cwd の文字列しか見ない（`Session` も設定も
//! 知らない）ので、当たる・当たらないだけを単体で確かめられる。

/// glob の 1 セグメント（`/` で区切った 1 段）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Seg {
    /// `**` — 0 段以上のセグメントに当たる。
    DoubleStar,
    /// `*` / `?` を含みうる 1 段ぶんのパターン。文字単位で持つのは、
    /// マルチバイトなディレクトリ名でも `?` が 1 文字として振る舞うため。
    Pat(Vec<char>),
}

/// 条件の実体（書き方はモジュール doc の表）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Matcher {
    /// セグメント単位の前方一致。空なら「すべて」（条件 `/`）。
    Prefix(Vec<String>),
    Glob(Vec<Seg>),
}

impl Matcher {
    /// この cwd に当たるか。
    pub(super) fn matches(&self, cwd: &str) -> bool {
        match self {
            Matcher::Prefix(p) => path_prefix_matches(p, cwd),
            // 文字列→`char` の変換はここで 1 回だけ行う。`match_segments` の中で
            // やるとバックトラックのたびに同じセグメントを変換し直す。
            Matcher::Glob(segs) => {
                let cwd: Vec<Vec<char>> = split_path(cwd)
                    .into_iter()
                    .map(|seg| seg.chars().collect())
                    .collect();
                match_segments(segs, &cwd)
            }
        }
    }
}

/// パスを空でないセグメントに割る。`/a//b/` → `["a", "b"]`。
pub(super) fn split_path(p: &str) -> Vec<&str> {
    p.split('/').filter(|s| !s.is_empty()).collect()
}

/// 前方一致。セグメント単位で比べるので、区切りの重複（`/a//foo`）や末尾の `/` も
/// 自然に吸収される。`prefix` が空（条件が `/`）なら「すべて」に当たる。
fn path_prefix_matches(prefix: &[String], cwd: &str) -> bool {
    let segs = split_path(cwd);
    prefix.len() <= segs.len() && prefix.iter().zip(&segs).all(|(a, b)| a == b)
}

/// セグメント列どうしの照合。`**` は 0 段以上に当たる。
///
/// **2 ポインタ法**（`**` の戻り先を 1 つだけ覚える）で書く。素朴な再帰だと
/// `**/a/**/a/**/b` のような条件が指数時間になり、poller が毎 tick 踏む。
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
/// 文字単位で見るだけで、構造は `match_segments` と同じ 2 ポインタ法。
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
    use super::super::testing::{hits, home, session};
    use super::super::IgnorePattern;

    // ---- ワイルドカード無し（そのパスと配下） ---------------------------------

    #[test]
    fn a_plain_path_matches_itself_and_everything_below() {
        assert!(hits("/a/foo", "/a/foo"));
        assert!(hits("/a/foo", "/a/foo/b/c"));
    }

    /// 前方一致を文字列の `starts_with` で書くと、`/a/foo` を無視したつもりで
    /// `/a/foobar` まで消える。
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

    /// 区切りが重なった cwd でも前方一致が外れない。セグメント比較にしてある
    /// ので、glob 側（同じ `split_path` を通る）と挙動がずれない。
    #[test]
    fn a_doubled_separator_in_the_cwd_does_not_break_a_prefix() {
        assert!(hits("/a/foo", "/a//foo/b"));
        assert!(hits("/a/foo", "//a/foo"));
        assert!(!hits("/a/foo", "/a//foobar"));
    }

    // ---- glob -----------------------------------------------------------------

    #[test]
    fn a_double_star_crosses_separators_in_both_directions() {
        assert!(hits("**/cron-jobs/**", "/Users/x/cron-jobs/a"));
        assert!(hits("**/cron-jobs/**", "/Users/x/cron-jobs/a/b/c"));
    }

    /// 0 段に当たらないと、「ディレクトリごと畳む」つもりで書いた条件が、
    /// そのディレクトリ自身で走っているセッションだけ取りこぼす。
    #[test]
    fn a_trailing_double_star_also_matches_the_directory_itself() {
        assert!(hits("**/cron-jobs/**", "/Users/x/cron-jobs"));
    }

    /// ここが崩れると `*` と `**` の区別が消える。
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

    /// `glob_chars` が `*` をリテラル一致より先に判定していることの番人
    /// （順序を逆にすると何が起きるかは同関数のコメント）。
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

    #[test]
    fn a_glob_is_not_silently_extended_to_the_subtree() {
        assert!(hits("**/cron-*", "/Users/x/cron-jobs"));
        assert!(!hits("**/cron-*", "/Users/x/cron-jobs/a"));
        assert!(hits("**/cron-*/**", "/Users/x/cron-jobs/a"));
    }

    // ---- 計算量 ---------------------------------------------------------------

    /// 素朴な再帰だと指数時間になる条件。poller が毎 tick 踏む経路なので、
    /// ここが崩れると常駐が固まる。
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
