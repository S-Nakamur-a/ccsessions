//! 顔レジストリ — 組込み顔とユーザ顔をまとめて引けるようにする層。
//!
//! - **組込み顔** … リポジトリルートの `faces/*.toml` を `include_str!` で焼き込む。
//!   投稿者が最初に見る場所（`faces/README.md` / `_template.toml` と同居）に
//!   実物があることが「誰でも足せる」の体験そのものなので、crate 内には隠さない。
//! - **ユーザ顔** … `~/.config/ccsessions/faces/*.toml`。再ビルド無しで足せる。
//!
//! # 決してパニックしない
//! 壊れた顔ファイルは **`problems` に積んでその顔だけ無視**し、プロセスは落とさない
//! （`config.rs` の「daemon は last-good を保持し、決してパニックしない」と同じ方針）。
//! 組込み顔が壊れているのは開発時のバグなので、`every_builtin_face_parses` が
//! テストで捕まえる。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::face::parse;
use crate::face::spec::{FaceSpec, Problem, ProblemCode, Source};

/// `design` が未知だったときに落ちる先。
pub const DEFAULT_FACE_ID: &str = "egg";

/// 組込み顔。**並びがそのままメニューと `ccsessions face list` の並びになる**ので、
/// 「素直な順（既定 → 単純 → 凝ったもの）」で持つ。
const BUILTIN: &[(&str, &str)] = &[
    ("egg", include_str!("../../../faces/egg.toml")),
    ("round", include_str!("../../../faces/round.toml")),
    ("squircle", include_str!("../../../faces/squircle.toml")),
    ("bean", include_str!("../../../faces/bean.toml")),
];

/// 組込み顔の id 一覧（パース不要で引けるので、設定ファイルのコメント等に使う）。
pub fn builtin_ids() -> Vec<&'static str> {
    BUILTIN.iter().map(|(id, _)| *id).collect()
}

/// 使える顔の一覧。
#[derive(Debug, Clone, Default)]
pub struct Registry {
    faces: Vec<Arc<FaceSpec>>,
    /// 読み込めなかった顔（表示用のラベル, 問題の一覧）。
    problems: Vec<(String, Vec<Problem>)>,
}

impl Registry {
    /// 組込み顔だけのレジストリ。
    pub fn builtin() -> Self {
        let mut r = Registry::default();
        for (id, text) in BUILTIN {
            match parse::parse(text, Source::Builtin) {
                Ok(face) => {
                    // ファイル名と id が食い違っていたら、参照する側が混乱するので弾く。
                    if face.id == *id {
                        r.faces.push(Arc::new(face));
                    } else {
                        r.problems.push((
                            format!("faces/{id}.toml"),
                            vec![Problem::new(
                                ProblemCode::Id,
                                format!("ファイル名 {id}.toml と id {:?} が一致しません", face.id),
                            )],
                        ));
                    }
                }
                Err(ps) => r.problems.push((format!("faces/{id}.toml"), ps)),
            }
        }
        r
    }

    /// 組込み顔 + `dir` 直下のユーザ顔。
    ///
    /// **`dir` を明示的に受け取る**のは、テストが本物の `~/.config/ccsessions/` に
    /// 触らずに済むようにするため（環境変数に依存させない — CLAUDE.md のテスト方針）。
    pub fn load_in(dir: &Path) -> Self {
        let mut r = Registry::builtin();
        for (path, text) in read_user_dir(dir) {
            let label = path.display().to_string();
            match parse::parse(&text, Source::User(path.clone())) {
                Ok(face) => {
                    if r.get(&face.id).is_some() {
                        // 組込みと同じ id は乗っ取らせない。ユーザ顔どうしの
                        // 衝突も先に読んだ方を残す（走査順は名前昇順で決定的）。
                        r.problems.push((
                            label,
                            vec![Problem::new(
                                ProblemCode::Id,
                                format!(
                                    "id {:?} は既にある顔と重複しています。別の id にしてください",
                                    face.id
                                ),
                            )],
                        ));
                    } else {
                        r.faces.push(Arc::new(face));
                    }
                }
                Err(ps) => r.problems.push((label, ps)),
            }
        }
        r
    }

    /// 顔を id で引く。
    pub fn get(&self, id: &str) -> Option<&Arc<FaceSpec>> {
        self.faces.iter().find(|f| f.id == id)
    }

    /// 顔を id で引き、無ければ既定（`egg`）へ落とす。
    ///
    /// **未知の id を `Err` にしない**のが要点。ユーザ顔ファイルの
    /// 存在は設定のパース時点では分からないので、検証はここ（解決時）で行い、
    /// 落ちる代わりにフォールバックする。
    pub fn resolve(&self, id: &str) -> Arc<FaceSpec> {
        if let Some(f) = self.get(id) {
            return Arc::clone(f);
        }
        eprintln!(
            "ccsessions: 顔 {id:?} が見つかりません。{DEFAULT_FACE_ID} を使います\
             （使える顔: {}）",
            self.ids().join(", ")
        );
        self.get(DEFAULT_FACE_ID)
            .or_else(|| self.faces.first())
            .cloned()
            .expect("組込み顔が 1 つも読めていない（every_builtin_face_parses を参照）")
    }

    /// 顔が 1 つも無いか。`resolve` が `expect` で落ちる条件でもある。
    pub fn is_empty(&self) -> bool {
        self.faces.is_empty()
    }

    pub fn ids(&self) -> Vec<&str> {
        self.faces.iter().map(|f| f.id.as_str()).collect()
    }

    pub fn all(&self) -> &[Arc<FaceSpec>] {
        &self.faces
    }

    /// 読み込めなかった顔。呼び出し側（daemon / CLI）が stderr に列挙する。
    pub fn problems(&self) -> &[(String, Vec<Problem>)] {
        &self.problems
    }

    /// 読み込めなかった顔を stderr に列挙する。**戻り値は問題があったかどうか**。
    pub fn report_problems(&self) -> bool {
        for (label, ps) in &self.problems {
            eprintln!("ccsessions: 顔 {label} を読み込めませんでした:");
            for p in ps {
                eprintln!("  {p}");
            }
        }
        !self.problems.is_empty()
    }
}

/// `dir` 直下の `*.toml` を名前昇順で読む。`_` で始まるものは雛形なので飛ばす。
///
/// ディレクトリが無い・読めないときは静かに空を返す（ユーザ顔は任意の機能なので、
/// 置いていないことをエラーにしない）。
fn read_user_dir(dir: &Path) -> Vec<(PathBuf, String)> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut paths: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.extension().is_some_and(|e| e == "toml")
                && p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| !n.starts_with('_'))
                // ディレクトリやシンボリックリンク先のディレクトリは読まない。
                && p.is_file()
        })
        .collect();
    paths.sort();

    paths
        .into_iter()
        .filter_map(|p| std::fs::read_to_string(&p).ok().map(|t| (p, t)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::face::Size;
    use std::fs;
    use tempfile::TempDir;

    /// 組込み顔が全部読めること。**壊れた組込み顔をここで捕まえる**
    /// （実行時は skip されるので、テストが無いと静かに顔が消える）。
    #[test]
    fn every_builtin_face_parses() {
        let r = Registry::builtin();
        assert!(
            r.problems().is_empty(),
            "組込み顔にパースエラーがある: {:?}",
            r.problems()
        );
        assert_eq!(
            r.ids(),
            vec!["egg", "round", "squircle", "bean"],
            "組込み顔の顔ぶれか並びが変わっている"
        );
    }

    /// 未知の id は `egg` へ落ちる（設定を `Err` にせずフォールバックする）。
    #[test]
    fn an_unknown_id_falls_back_to_egg() {
        let r = Registry::builtin();
        assert_eq!(r.resolve("no-such-face").id, DEFAULT_FACE_ID);
        assert_eq!(r.resolve("bean").id, "bean");
    }

    /// ユーザ顔が組込みに足される。
    #[test]
    fn user_faces_are_merged_after_the_builtins() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("mine.toml"), user_face("mine")).unwrap();
        let r = Registry::load_in(dir.path());
        assert!(r.problems().is_empty(), "{:?}", r.problems());
        assert_eq!(r.ids(), vec!["egg", "round", "squircle", "bean", "mine"]);
        assert_eq!(r.resolve("mine").body_size(Size::Bar), (20.0, 18.0));
    }

    /// 壊れたユーザ顔は**その顔だけ無視**され、他は生き残る（プロセスは落ちない）。
    #[test]
    fn a_broken_user_face_is_skipped_without_killing_the_rest() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("broken.toml"), "this is not toml [[[").unwrap();
        fs::write(dir.path().join("good.toml"), user_face("good")).unwrap();
        let r = Registry::load_in(dir.path());
        assert!(r.get("good").is_some(), "壊れていない顔まで落ちている");
        assert!(r.get("egg").is_some(), "組込みまで落ちている");
        assert_eq!(r.problems().len(), 1, "問題が記録されていない");
        assert!(r.problems()[0].0.contains("broken.toml"));
    }

    /// ユーザ顔は組込みの id を乗っ取れない。
    #[test]
    fn a_user_face_cannot_shadow_a_builtin_id() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("evil.toml"), user_face("egg")).unwrap();
        let r = Registry::load_in(dir.path());
        // 組込みの egg がそのまま残っている。
        assert_eq!(r.resolve("egg").body_size(Size::Bar), (22.0, 20.0));
        assert_eq!(r.problems().len(), 1);
        assert_eq!(r.problems()[0].1[0].code, ProblemCode::Id);
    }

    /// `_` で始まるファイル（雛形）は読まない。
    #[test]
    fn template_files_are_skipped() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("_template.toml"), user_face("tmpl")).unwrap();
        let r = Registry::load_in(dir.path());
        assert!(r.get("tmpl").is_none());
        assert!(r.problems().is_empty());
    }

    /// ディレクトリが無くても静かに組込みだけになる。
    #[test]
    fn a_missing_user_dir_is_not_an_error() {
        let dir = TempDir::new().unwrap();
        let r = Registry::load_in(&dir.path().join("does-not-exist"));
        assert!(r.problems().is_empty());
        assert_eq!(r.ids(), Registry::builtin().ids());
    }

    fn user_face(id: &str) -> String {
        format!(
            r#"
id = "{id}"
label = "ユーザ顔"
[size]
bar  = {{ w = 20, h = 18 }}
dock = {{ w = 32, h = 30 }}
[outline]
kind = "corners"
corners = [[0.5,0.5],[0.5,0.5],[0.5,0.5],[0.5,0.5]]
[eyes]
shape = "rounded"
gap  = {{ bar = 3.0, dock = 5.0 }}
size = {{ bar = [3.0, 4.0], dock = [4.0, 6.0] }}
"#
        )
    }
}
