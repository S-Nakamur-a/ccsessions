//! セッションファイルストア: `sessions_dir()/<session_id>.json` に 1 セッション
//! 1 ファイルで読み書きする。
//!
//! 「1 名 1 ファイル・last write wins」の素朴な設計を採る。
//!
//! **注意（当初の想定は誤りだった）**: 当初は「セッションごとに書き手は 1 プロセス
//! （そのセッションの hook）だけなので、ファイル単位の atomic write で lost update を
//! 避けられる」と考えていた。しかし Claude Code は**マッチした hook をすべて並列
//! プロセスで実行する**ため、同一セッションに対して複数の `ccsessions hook` が同時に
//! `read → reduce → write` を行う。`write_atomic` は「途中状態を読み手に晒さない」
//! ことだけを保証し、read-modify-write 全体は保護しない
//! （実測: サブエージェントを 8 並列で起動すると `agents` が 2 件しか残らなかった）。
//!
//! そのため**書き手は [`lock_exclusive`] を `load` から `save`/`remove` まで
//! 保持する**（`lock.rs`。なぜストア全体で 1 個なのか・なぜセッションファイル
//! 自身を flock しないのかもそちらに書いてある）。ロックは書き手の規律であって
//! `load`/`save` に内蔵されてはいない — 内蔵しても reduce を挟む
//! read-modify-write は守れないため。
//!
//! **読み手（daemon の poller）はロックを取らなくてよい**。rename の atomicity
//! により、古い内容か新しい内容のどちらかしか見えない。
//!
//! テストは env var (`CCSESSIONS_STATE_DIR`) に依存すると並列実行時に干渉するため、
//! 明示的にディレクトリを受け取る `*_in` 関数を内部に切り、公開 API はそれを
//! `sessions_dir()` で包むだけにしてある。

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::lock::lock_exclusive_in;
use crate::session::{DeadReason, Session};
use crate::{sessions_dir, write_atomic};

pub use crate::lock::StoreLock;

// ---------------------------------------------------------------------------
// Path / id validation
// ---------------------------------------------------------------------------

/// セッション id をファイル名の一部として使えるか検証する。
///
/// `[A-Za-z0-9._-]` のみ許可・先頭 `.` 禁止・空文字禁止 — `..` や `/` による
/// パストラバーサルと、隠しファイル扱いされて `list()` から漏れる事故を防ぐ。Claude Code の session_id は通常
/// UUID だが、hook payload は外部入力なので信用せずここで弾く。
fn validate_session_id(id: &str) -> Result<(), String> {
    if id.is_empty() {
        return Err("session id must not be empty".into());
    }
    if id.starts_with('.') {
        return Err(format!("session id must not start with '.': {id:?}"));
    }
    for ch in id.chars() {
        if !matches!(ch, 'A'..='Z' | 'a'..='z' | '0'..='9' | '.' | '_' | '-') {
            return Err(format!(
                "session id contains invalid character {ch:?} \
                 (only [A-Za-z0-9._-] allowed): {id:?}"
            ));
        }
    }
    Ok(())
}

/// `id` に対応するセッションファイルのパス。id が不正なら `Err`。
pub fn session_path(id: &str) -> io::Result<PathBuf> {
    validate_session_id(id).map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
    Ok(sessions_dir().join(format!("{id}.json")))
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// ストアへの書き込みを排他するロックを取る。
///
/// **`load` から `save`/`remove` までガードを保持し続けること。**
/// hook はマッチしたぶんだけ並列プロセスで起動されるので、これを挟まないと
/// read-modify-write が後勝ちになる（モジュール doc 参照）。
///
/// 取得できないとき（最大 500ms 待って諦めた・flock 非対応 FS 等）は `Err`。
/// **hook はこれを致命的として扱ってはいけない** — 警告を出してロック無しで
/// 続行するほうが、状態更新を丸ごと落とすより実害が小さい。
pub fn lock_exclusive() -> io::Result<StoreLock> {
    lock_exclusive_in(&sessions_dir())
}

/// セッションを読む。無ければ `Ok(None)`。壊れた JSON も `Ok(None)`
/// （daemon が不正なファイル 1 つでクラッシュしないための fail-safe）。
pub fn load(id: &str) -> io::Result<Option<Session>> {
    load_in(&sessions_dir(), id)
}

/// セッションを atomic write で保存する。
pub fn save(s: &Session) -> io::Result<()> {
    save_in(&sessions_dir(), s)
}

/// セッションファイルを削除する。無くてもエラーにしない。
pub fn remove(id: &str) -> io::Result<()> {
    remove_in(&sessions_dir(), id)
}

/// 全セッションを列挙する。`updated` の降順。壊れたファイル・ドットファイル・
/// `.tmp` はスキップ。ディレクトリが無ければ空 `Vec`。
pub fn list() -> Vec<Session> {
    list_in(&sessions_dir())
}

/// 生きているセッションを最大 `max` 件、`updated` 降順で返す。
///
/// 「生きている」の定義は `Session::dead_reason` — TTL 内で、かつ持ち主の
/// プロセスが居ること。**ファイルが残っていることは生きている証拠にならない**
/// （`SessionEnd` が飛ばない終わり方があるため）。
pub fn list_live(now: u64, session_ttl_ms: u64, max: usize) -> Vec<Session> {
    list_live_in(
        &sessions_dir(),
        now,
        session_ttl_ms,
        max,
        &crate::process::is_alive,
    )
}

/// 死んだセッションのファイルを削除し、消したものを理由つきで返す。
///
/// 件数ではなく中身を返すのは、**生きているセッションを誤って消してしまった
/// ときに理由を追えるようにする**ため（呼び出し側がログへ出す）。
///
/// 掃除は「読んで消す」＝**書き手**なので、モジュール doc の規律どおり
/// `lock_exclusive` を保持したまま行う。取れなければロック無しで続行する
/// （hook と同じ degradation）。最悪でも、hook が read-modify-write している
/// 最中のセッションを消してしまい、その hook の save で作り直されるだけ
/// ＝ 掃除が 1 回空振りするのと同じで、状態は壊れない。
pub fn sweep(now: u64, session_ttl_ms: u64) -> Vec<(Session, DeadReason)> {
    let dir = sessions_dir();
    let _guard = lock_exclusive_in(&dir).map_err(|e| {
        eprintln!("ccsessions: warning: sweeping without the store lock: {e}");
    });
    sweep_in(&dir, now, session_ttl_ms, &crate::process::is_alive)
}

// ---------------------------------------------------------------------------
// Inner functions (explicit dir; used by tests for env-var-free isolation)
// ---------------------------------------------------------------------------

fn load_in(dir: &Path, id: &str) -> io::Result<Option<Session>> {
    let path = session_path_in(dir, id)?;
    let content = match fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    // パース失敗は「壊れたファイル」として無いものとして扱う。session.json の
    // フォーマットが将来変わっても、古い daemon がクラッシュしないようにする。
    Ok(serde_json::from_str(&content).ok())
}

fn save_in(dir: &Path, s: &Session) -> io::Result<()> {
    let path = session_path_in(dir, &s.id)?;
    let content = serde_json::to_string(s)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
    write_atomic(&path, &content)
}

fn remove_in(dir: &Path, id: &str) -> io::Result<()> {
    let path = session_path_in(dir, id)?;
    match fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

fn session_path_in(dir: &Path, id: &str) -> io::Result<PathBuf> {
    validate_session_id(id).map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
    Ok(dir.join(format!("{id}.json")))
}

pub(crate) fn list_in(dir: &Path) -> Vec<Session> {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        // ディレクトリ未作成は「まだ 1 件もセッションが無い」ときの通常状態。
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Vec::new(),
        // それ以外（権限エラー等）は poller を落とさず空を返す。
        Err(e) => {
            eprintln!(
                "ccsessions: warning: cannot read sessions directory {}: {}",
                dir.display(),
                e
            );
            return Vec::new();
        }
    };

    let mut sessions = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let file_name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n,
            None => continue,
        };
        // write_atomic の tmp ファイル（`.foo.<pid>.<seq>.tmp`）と、その他の
        // ドットファイルは読み物として扱わない。
        if file_name.starts_with('.') || file_name.ends_with(".tmp") {
            continue;
        }
        if !file_name.ends_with(".json") {
            continue;
        }
        let content = match fs::read_to_string(&path) {
            Ok(c) => c,
            Err(_) => continue,
        };
        // パース失敗は壊れたファイルとしてスキップする（fail-safe）。
        if let Ok(session) = serde_json::from_str::<Session>(&content) {
            sessions.push(session);
        }
    }

    sessions.sort_by_key(|s| std::cmp::Reverse(s.updated));
    sessions
}

/// `alive` はプロセスの生存確認。テストから偽物を差し込めるように引数で受ける
/// （`*_in` を切ってあるのと同じ理由 — 本物のプロセスに依存させない）。
fn list_live_in(
    dir: &Path,
    now: u64,
    session_ttl_ms: u64,
    max: usize,
    alive: &dyn Fn(u32) -> bool,
) -> Vec<Session> {
    list_in(dir)
        .into_iter()
        .filter(|s| s.is_live(now, session_ttl_ms, alive))
        .take(max)
        .collect()
}

fn sweep_in(
    dir: &Path,
    now: u64,
    session_ttl_ms: u64,
    alive: &dyn Fn(u32) -> bool,
) -> Vec<(Session, DeadReason)> {
    let mut removed = Vec::new();
    for s in list_in(dir) {
        let Some(reason) = s.dead_reason(now, session_ttl_ms, alive) else {
            continue;
        };
        // 削除に失敗したもの（権限・競合）は「消した」と報告しない。次の掃除で
        // もう一度試されるだけなので、ここでは黙って見送る。
        if remove_in(dir, &s.id).is_ok() {
            removed.push((s, reason));
        }
    }
    removed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::SessionState;
    use tempfile::TempDir;

    fn session(id: &str, updated: u64) -> Session {
        Session {
            id: id.into(),
            name: "proj".into(),
            title: None,
            cwd: "/tmp/proj".into(),
            state: SessionState::Working,
            since: updated,
            updated,
            agents: vec![],
            main_stopped: false,
            error_kind: None,
            pid: None,
        }
    }

    /// 同じプロジェクト（cwd）で走った別セッション。ゾンビが積み上がる実際の形。
    fn session_in(id: &str, cwd: &str, updated: u64, pid: Option<u32>) -> Session {
        Session {
            cwd: cwd.into(),
            name: Session::name_from_cwd(cwd),
            pid,
            ..session(id, updated)
        }
    }

    /// テスト用の生存確認: ここに挙げた pid だけが生きている。
    fn only_alive(pids: &[u32]) -> impl Fn(u32) -> bool + '_ {
        move |pid| pids.contains(&pid)
    }

    /// すべて生きている扱い（pid を見ない従来どおりの挙動を確かめるとき用）。
    fn all_alive(_: u32) -> bool {
        true
    }

    // ---- round trip ---------------------------------------------------------

    #[test]
    fn save_then_load_round_trips() {
        let dir = TempDir::new().unwrap();
        let s = session("abc-123", 1000);
        save_in(dir.path(), &s).unwrap();
        let loaded = load_in(dir.path(), "abc-123").unwrap().unwrap();
        assert_eq!(loaded, s);
    }

    #[test]
    fn load_missing_returns_none() {
        let dir = TempDir::new().unwrap();
        assert!(load_in(dir.path(), "no-such-id").unwrap().is_none());
    }

    #[test]
    fn remove_deletes_file() {
        let dir = TempDir::new().unwrap();
        let s = session("abc", 1000);
        save_in(dir.path(), &s).unwrap();
        remove_in(dir.path(), "abc").unwrap();
        assert!(load_in(dir.path(), "abc").unwrap().is_none());
    }

    #[test]
    fn remove_missing_is_not_an_error() {
        let dir = TempDir::new().unwrap();
        remove_in(dir.path(), "never-existed").unwrap();
    }

    // ---- id validation --------------------------------------------------------

    #[test]
    fn invalid_id_rejected_by_load_and_save() {
        let dir = TempDir::new().unwrap();
        assert!(load_in(dir.path(), "../escape").is_err());
        assert!(load_in(dir.path(), "").is_err());
        assert!(load_in(dir.path(), ".hidden").is_err());
        assert!(load_in(dir.path(), "a/b").is_err());

        let mut s = session("ok", 1000);
        s.id = "../escape".into();
        assert!(save_in(dir.path(), &s).is_err());
    }

    // ---- malformed JSON ---------------------------------------------------------

    #[test]
    fn malformed_json_load_returns_none() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("broken.json"), "{not valid json").unwrap();
        assert!(load_in(dir.path(), "broken").unwrap().is_none());
    }

    #[test]
    fn malformed_json_skipped_in_list() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("broken.json"), "{not valid json").unwrap();
        save_in(dir.path(), &session("ok", 1000)).unwrap();
        let sessions = list_in(dir.path());
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id, "ok");
    }

    // ---- list: dotfiles / tmp / sort ------------------------------------------

    #[test]
    fn list_skips_dotfiles_and_tmp() {
        let dir = TempDir::new().unwrap();
        save_in(dir.path(), &session("real", 1000)).unwrap();
        fs::write(dir.path().join(".real.123.0.tmp"), "{}").unwrap();
        fs::write(dir.path().join(".hidden.json"), "{}").unwrap();
        let sessions = list_in(dir.path());
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id, "real");
    }

    #[test]
    fn list_sorts_by_updated_descending() {
        let dir = TempDir::new().unwrap();
        save_in(dir.path(), &session("old", 1000)).unwrap();
        save_in(dir.path(), &session("newest", 3000)).unwrap();
        save_in(dir.path(), &session("mid", 2000)).unwrap();
        let sessions = list_in(dir.path());
        let ids: Vec<&str> = sessions.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, vec!["newest", "mid", "old"]);
    }

    #[test]
    fn list_on_missing_dir_returns_empty() {
        let dir = TempDir::new().unwrap();
        let missing = dir.path().join("does-not-exist");
        assert!(list_in(&missing).is_empty());
    }

    // ---- list_live: TTL exclusion + max -----------------------------------------

    #[test]
    fn list_live_excludes_stale_sessions() {
        let dir = TempDir::new().unwrap();
        save_in(dir.path(), &session("fresh", 9000)).unwrap();
        save_in(dir.path(), &session("stale", 0)).unwrap();
        let now = 10_000;
        let ttl = 5_000;
        let live = list_live_in(dir.path(), now, ttl, 100, &all_alive);
        let ids: Vec<&str> = live.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, vec!["fresh"]);
    }

    #[test]
    fn list_live_respects_max() {
        let dir = TempDir::new().unwrap();
        for i in 0..5 {
            save_in(dir.path(), &session(&format!("s{i}"), 1000 + i)).unwrap();
        }
        let live = list_live_in(dir.path(), 2000, 10_000, 3, &all_alive);
        assert_eq!(live.len(), 3);
    }

    // ---- list_live: 死んだプロセスの除外（ゾンビ表示の回帰防止）------------------

    /// 本題の回帰テスト。同じ作業ディレクトリで走った 4 セッションのうち
    /// 生きているのは 1 つだけ、という実際に踏んだ状況を再現する。
    ///
    /// 死んだ 3 つはどれも TTL の内側（さっき更新された）なので、**時間では
    /// 区別できない**。プロセスの生存だけが手掛かりになる。
    #[test]
    fn dead_sessions_from_the_same_workdir_are_not_listed() {
        let dir = TempDir::new().unwrap();
        let wd = "/Users/x/.claudep/clean-workdir";
        save_in(dir.path(), &session_in("zombie-1", wd, 9_000, Some(101))).unwrap();
        save_in(dir.path(), &session_in("zombie-2", wd, 9_100, Some(102))).unwrap();
        save_in(dir.path(), &session_in("zombie-3", wd, 9_200, Some(103))).unwrap();
        save_in(dir.path(), &session_in("running", wd, 9_300, Some(999))).unwrap();

        let live = list_live_in(dir.path(), 10_000, 8 * 3600 * 1000, 12, &only_alive(&[999]));
        let ids: Vec<&str> = live.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(
            ids,
            vec!["running"],
            "同じ workdir の死んだセッションは 1 件も残ってはいけない"
        );
    }

    /// 逆方向の保証（こちらの方が事故として深刻）: 生きているセッションは、
    /// 同じ workdir に死んだ兄弟がいくつ居ようと必ず残る。
    #[test]
    fn live_sessions_sharing_a_workdir_are_all_kept() {
        let dir = TempDir::new().unwrap();
        let wd = "/Users/x/proj";
        save_in(dir.path(), &session_in("live-a", wd, 9_000, Some(11))).unwrap();
        save_in(dir.path(), &session_in("live-b", wd, 9_100, Some(22))).unwrap();
        save_in(dir.path(), &session_in("dead", wd, 9_200, Some(33))).unwrap();
        // pid を持たない古い形式のセッションも、生存確認できないだけで生きている。
        save_in(dir.path(), &session_in("legacy", wd, 9_300, None)).unwrap();

        let live = list_live_in(
            dir.path(),
            10_000,
            8 * 3600 * 1000,
            12,
            &only_alive(&[11, 22]),
        );
        let mut ids: Vec<&str> = live.iter().map(|s| s.id.as_str()).collect();
        ids.sort_unstable();
        assert_eq!(ids, vec!["legacy", "live-a", "live-b"]);
    }

    /// 死んだセッションが `max` の枠を食い潰さないこと。ゾンビの実害はこれ
    /// （枠が 12 しかないところに死んだセッションが 9 匹居座り、生きている
    /// セッションが押し出されて見えなくなっていた）。
    #[test]
    fn dead_sessions_do_not_consume_the_max_slots() {
        let dir = TempDir::new().unwrap();
        // 死んだセッションの方が新しい ＝ `updated` 降順では先に並ぶ。
        for i in 0..5u32 {
            save_in(
                dir.path(),
                &session_in(
                    &format!("dead-{i}"),
                    "/w",
                    9_000 + u64::from(i),
                    Some(i + 10),
                ),
            )
            .unwrap();
        }
        save_in(dir.path(), &session_in("alive", "/w", 8_000, Some(777))).unwrap();

        let live = list_live_in(dir.path(), 10_000, 8 * 3600 * 1000, 3, &only_alive(&[777]));
        let ids: Vec<&str> = live.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, vec!["alive"]);
    }

    // ---- sweep ------------------------------------------------------------------

    #[test]
    fn sweep_removes_expired_and_reports_them() {
        let dir = TempDir::new().unwrap();
        save_in(dir.path(), &session("fresh", 9000)).unwrap();
        save_in(dir.path(), &session("stale-1", 0)).unwrap();
        save_in(dir.path(), &session("stale-2", 100)).unwrap();
        let removed = sweep_in(dir.path(), 10_000, 5_000, &all_alive);
        assert_eq!(removed.len(), 2);
        assert!(removed
            .iter()
            .all(|(_, reason)| *reason == DeadReason::Expired));
        let remaining = list_in(dir.path());
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].id, "fresh");
    }

    #[test]
    fn sweep_removes_sessions_whose_process_is_gone() {
        let dir = TempDir::new().unwrap();
        save_in(dir.path(), &session_in("gone", "/w", 9_500, Some(4242))).unwrap();
        save_in(dir.path(), &session_in("running", "/w", 9_500, Some(777))).unwrap();
        save_in(dir.path(), &session_in("legacy", "/w", 9_500, None)).unwrap();

        let removed = sweep_in(dir.path(), 10_000, 8 * 3600 * 1000, &only_alive(&[777]));
        assert_eq!(removed.len(), 1);
        assert_eq!(removed[0].0.id, "gone");
        assert_eq!(removed[0].1, DeadReason::ProcessGone(4242));

        let mut remaining: Vec<String> = list_in(dir.path()).into_iter().map(|s| s.id).collect();
        remaining.sort();
        assert_eq!(
            remaining,
            vec!["legacy", "running"],
            "生きているセッションと持ち主不明のセッションは消してはいけない"
        );
    }

    /// 掃除は何度走らせても同じ結果（消したものは 2 度報告されない）。
    #[test]
    fn sweep_is_idempotent() {
        let dir = TempDir::new().unwrap();
        save_in(dir.path(), &session_in("gone", "/w", 9_500, Some(4242))).unwrap();
        let alive = only_alive(&[]);
        assert_eq!(sweep_in(dir.path(), 10_000, 5_000, &alive).len(), 1);
        assert!(sweep_in(dir.path(), 10_000, 5_000, &alive).is_empty());
    }
}
