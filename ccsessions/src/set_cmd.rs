//! `ccsessions set --session <id> --state <state> [--cwd <cwd>]`
//!
//! hook を介さず状態を直接書き換えるデバッグ／外部 producer 向けコマンド。
//! セッションが無ければ新規作成する。

use ccsessions_core::session::{Session, SessionState};
use ccsessions_core::{now_ms, store};

pub fn run(args: &[String]) -> i32 {
    let mut session_id: Option<String> = None;
    let mut state_arg: Option<String> = None;
    let mut cwd_arg: Option<String> = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--session" => {
                i += 1;
                session_id = args.get(i).cloned();
            }
            "--state" => {
                i += 1;
                state_arg = args.get(i).cloned();
            }
            "--cwd" => {
                i += 1;
                cwd_arg = args.get(i).cloned();
            }
            other => {
                eprintln!("ccsessions: set: unknown argument: {other}");
                return 1;
            }
        }
        i += 1;
    }

    let session_id = match session_id.filter(|s| !s.is_empty()) {
        Some(id) => id,
        None => {
            eprintln!("ccsessions: set: --session <id> is required");
            return 1;
        }
    };
    let state = match state_arg.as_deref().and_then(SessionState::from_str) {
        Some(s) => s,
        None => {
            eprintln!(
                "ccsessions: set: --state must be one of: working, wait_user, wait_agent, idle, done, error"
            );
            return 1;
        }
    };

    // hook と同じく load → save の read-modify-write なので、同じロックを取る
    // （`ccsessions set` を叩いた瞬間に hook が走っていれば競合する）。
    // hook と違いこちらは対話的なコマンドなので、取れなければ黙って続けず
    // 警告を出す。ただし失敗にはしない — debug 用コマンドがロック取得の失敗で
    // 使えなくなるほうが困る。
    let _store_lock = match store::lock_exclusive() {
        Ok(lock) => Some(lock),
        Err(e) => {
            eprintln!("ccsessions: set: warning: proceeding without the store lock: {e}");
            None
        }
    };

    let prev = match store::load(&session_id) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("ccsessions: set: invalid session id {session_id:?}: {e}");
            return 1;
        }
    };

    let now = now_ms();
    let mut session = prev.unwrap_or_else(|| {
        // 新規作成時のみ --cwd（省略時はカレントディレクトリ）を使う。既存
        // セッションの cwd/name は hook 経由の更新に委ねるべきで、debug 用の
        // set コマンドが横から書き換えるべきではないため。
        let cwd = cwd_arg
            .clone()
            .or_else(|| {
                std::env::current_dir()
                    .ok()
                    .map(|p| p.display().to_string())
            })
            .unwrap_or_default();
        Session {
            id: session_id.clone(),
            name: Session::name_from_cwd(&cwd),
            // タイトルは transcript から拾うもので、`set` には transcript が無い。
            title: None,
            cwd,
            state,
            since: now,
            updated: now,
            agents: Vec::new(),
            // `set` はターンの外から状態を差し込む debug 用なので、メインスレッドの
            // ターンが終わっているかは分からない。進行中側（false）に倒しておく。
            main_stopped: false,
            error_kind: None,
            // 持ち主の pid は記録しない。`set` を叩いたシェルは Claude Code の
            // セッションではないので、その生死をセッションの生死とみなせない。
            // pid 無し ＝「持ち主不明」で、TTL だけで掃除される（`session.rs`）。
            pid: None,
        }
    });
    session.set_state(state, now);

    if let Err(e) = store::save(&session) {
        eprintln!("ccsessions: set: failed to save session: {e}");
        return 1;
    }
    0
}
