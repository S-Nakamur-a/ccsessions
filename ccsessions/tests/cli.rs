//! `ccsessions hook` と `list` の統合テスト（実プロセスを起こす層）。
//!
//! **安全のための鉄則**: 本物の `~/.claude/settings.json` にも
//! `~/.local/state/ccsessions` にも一切触らない。状態ディレクトリは
//! `CCSESSIONS_STATE_DIR` で tempdir に隔離する。
//!
//! かつてここには `install-hooks` / `uninstall-hooks` の統合テストがあったが、
//! **hook を settings.json に書くコードごと消した**（配線は Claude Code
//! プラグインの仕事。[ADR 0021](../../docs/adr/0021-distribution.md)）。
//! プラグインが配る `hooks.json` の検証は `settings_json.rs` の単体テストにある。

use std::fs;
use std::io::Write;
use std::process::{Command, Stdio};

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_ccsessions")
}

/// `ccsessions hook` に payload を流し、`(exit code, stdout, stderr)` を返す。
/// 状態ディレクトリは呼び出し側が渡した tempdir に隔離する（本物の
/// `~/.local/state/ccsessions` には絶対に書かない）。
fn run_hook(state_dir: &std::path::Path, extra_args: &[&str], stdin_data: &str) -> (i32, String) {
    let mut child = Command::new(bin())
        .arg("hook")
        .args(extra_args)
        .env("CCSESSIONS_STATE_DIR", state_dir)
        // config も隔離する（実ユーザの config.toml を読ませない）。
        .env("CCSESSIONS_CONFIG", state_dir.join("config.toml"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(stdin_data.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
    )
}

/// **`ccsessions hook` の契約の唯一の機械的な番人。**
///
/// CLAUDE.md の不変条件: 何があっても exit 0、stdout には何も書かない。
/// 破ると実害が出る — `UserPromptSubmit` と `SessionStart` は exit 0 の stdout が
/// そのまま Claude に渡るのでユーザのプロンプトにゴミが混入し、exit 2 は
/// `PreToolUse` でツールをブロックし `PermissionRequest` で権限を拒否する。
/// 異常系を通しても決して破られないことをここで固定する。
#[test]
fn hook_always_exits_zero_and_writes_nothing_to_stdout() {
    let dir = tempfile::TempDir::new().unwrap();
    let state = dir.path().join("state");

    let cases: Vec<(&str, Vec<&str>, &str)> = vec![
        ("空の stdin", vec![], ""),
        ("壊れた JSON", vec![], "not json at all"),
        ("JSON だが配列", vec![], "[1,2,3]"),
        ("session_id 欠落", vec![], r#"{"hook_event_name":"Stop"}"#),
        (
            "未知イベント",
            vec![],
            r#"{"session_id":"s","hook_event_name":"FutureEvent"}"#,
        ),
        (
            "購読している全イベント: Stop",
            vec![],
            r#"{"session_id":"s","hook_event_name":"Stop","cwd":"/tmp/x"}"#,
        ),
        (
            "StopFailure",
            vec![],
            r#"{"session_id":"s","hook_event_name":"StopFailure","error":"rate_limit","cwd":"/tmp/x"}"#,
        ),
        (
            "PostToolBatch",
            vec![],
            r#"{"session_id":"s","hook_event_name":"PostToolBatch","tool_calls":[],"cwd":"/tmp/x"}"#,
        ),
        (
            "SubagentStart",
            vec![],
            r#"{"session_id":"s","hook_event_name":"SubagentStart","agent_id":"a1","agent_type":"t","cwd":"/tmp/x"}"#,
        ),
        (
            "SessionEnd（削除経路）",
            vec![],
            r#"{"session_id":"s","hook_event_name":"SessionEnd","reason":"clear"}"#,
        ),
        (
            "--record の値が無い",
            vec!["--record"],
            r#"{"session_id":"s","hook_event_name":"Stop","cwd":"/tmp/x"}"#,
        ),
        (
            "--record が書けないパス",
            vec!["--record", "/dev/null/nope"],
            r#"{"session_id":"s","hook_event_name":"Stop","cwd":"/tmp/x"}"#,
        ),
        (
            "非 UTF-8 混じりでない巨大 payload",
            vec![],
            // 1MB 弱の prompt を持つ payload。
            r#"{"session_id":"s","hook_event_name":"UserPromptSubmit","cwd":"/tmp/x","prompt":"PLACEHOLDER"}"#,
        ),
    ];

    for (name, args, payload) in cases {
        let body = if payload.contains("PLACEHOLDER") {
            payload.replace("PLACEHOLDER", &"あ".repeat(300_000))
        } else {
            payload.to_string()
        };
        let (code, stdout) = run_hook(&state, &args, &body);
        assert_eq!(code, 0, "`{name}` で exit code が 0 でない（{code}）");
        assert!(
            stdout.is_empty(),
            "`{name}` で stdout に出力があった: {stdout:?}"
        );
    }
}

/// **X-1（並列 hook による lost update）の回帰テスト。**
///
/// Claude Code はマッチした hook を**すべて並列プロセスで**実行する。
/// `ccsessions hook` の `load → reduce → save` は read-modify-write なので、
/// 排他しないと後勝ちで更新が消える（この対策を入れる前の実測: 8 並列の
/// `SubagentStart` で `agents` が 2 件しか残らなかった）。
///
/// ここは `ccsessions-core` の `lock.rs` の単体テスト（同一プロセス内の排他）
/// では捕まえられない層 — 実際に**別プロセスを同時に起動**して、8 件すべてが
/// 残ることを確かめる。プロセスを並べる必要があるので `run_hook`
/// （spawn して即 wait する）ではなく、全部 spawn してから wait する。
#[test]
fn parallel_hook_processes_do_not_lose_agents() {
    const N: usize = 8;

    let dir = tempfile::TempDir::new().unwrap();
    let state = dir.path().join("state");
    let session_id = "para-1";

    // 先にセッションを作っておく（並列部分の対象を「既存セッションの更新」に
    // 絞るため。SubagentStart は prev=None でも作るが、それだと「作成の競合」と
    // 「更新の競合」が混ざって何を測ったのか曖昧になる）。
    let (code, _) = run_hook(
        &state,
        &[],
        &format!(
            r#"{{"session_id":"{session_id}","hook_event_name":"UserPromptSubmit","cwd":"/tmp/x"}}"#
        ),
    );
    assert_eq!(code, 0);

    let children: Vec<_> = (1..=N)
        .map(|i| {
            let payload = format!(
                r#"{{"session_id":"{session_id}","hook_event_name":"SubagentStart",
                     "cwd":"/tmp/x","agent_id":"ag-{i}","agent_type":"general-purpose"}}"#
            );
            let mut child = Command::new(bin())
                .arg("hook")
                .env("CCSESSIONS_STATE_DIR", &state)
                .env("CCSESSIONS_CONFIG", state.join("config.toml"))
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            child
                .stdin
                .as_mut()
                .unwrap()
                .write_all(payload.as_bytes())
                .unwrap();
            // stdin を閉じて相手の read_to_string を終わらせる。閉じないと
            // 全プロセスが stdin 待ちで止まり、並列にならない。
            drop(child.stdin.take());
            child
        })
        .collect();

    for child in children {
        let out = child.wait_with_output().unwrap();
        assert_eq!(out.status.code(), Some(0), "hook は常に exit 0");
        assert!(
            out.stdout.is_empty(),
            "hook は stdout に何も書かない: {:?}",
            String::from_utf8_lossy(&out.stdout)
        );
    }

    let content = fs::read_to_string(state.join("sessions").join(format!("{session_id}.json")))
        .expect("セッションファイルが読めること");
    let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
    let agents = parsed["agents"].as_array().expect("agents は配列");

    let mut ids: Vec<&str> = agents
        .iter()
        .map(|a| a["id"].as_str().unwrap_or_default())
        .collect();
    ids.sort();
    let mut want: Vec<String> = (1..=N).map(|i| format!("ag-{i}")).collect();
    want.sort();

    assert_eq!(
        ids, want,
        "並列に起動した {N} 個の SubagentStart がすべて残るべき（lost update）"
    );
}

/// ロックファイルは `sessions/` 直下に置くので、セッション一覧に紛れ込まない
/// ことを実物の CLI でも確かめる（`.` 始まりなので `store::list` のドット
/// ファイルフィルタに引っかかる、という設計への依存を固定する）。
#[test]
fn the_store_lock_file_does_not_show_up_as_a_session() {
    let dir = tempfile::TempDir::new().unwrap();
    let state = dir.path().join("state");
    let (code, _) = run_hook(
        &state,
        &[],
        r#"{"session_id":"only-one","hook_event_name":"UserPromptSubmit","cwd":"/tmp/x"}"#,
    );
    assert_eq!(code, 0);

    // ロックファイルが実際に作られていること（作られていなければ排他が
    // 効いていないので、このテスト自体が無意味になる）。
    assert!(
        state.join("sessions").join(".lock").exists(),
        "hook はストアロックを取るはずなので .lock が存在するべき"
    );

    let out = Command::new(bin())
        .arg("list")
        .arg("--json")
        .env("CCSESSIONS_STATE_DIR", &state)
        .env("CCSESSIONS_CONFIG", state.join("config.toml"))
        .output()
        .unwrap();
    let parsed: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let sessions = parsed.as_array().expect("list --json は配列");
    assert_eq!(
        sessions.len(),
        1,
        "ロックファイルがセッションとして数えられている: {}",
        String::from_utf8_lossy(&out.stdout)
    );
}

// ---------------------------------------------------------------------------
// `ccsessions hook` — セッションの持ち主 pid の記録
// ---------------------------------------------------------------------------

/// 書かれたセッション JSON を読む。
fn saved_session(state_dir: &std::path::Path, id: &str) -> serde_json::Value {
    let path = state_dir.join("sessions").join(format!("{id}.json"));
    serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap()
}

fn payload(session_id: &str, event: &str, cwd: &str) -> String {
    serde_json::json!({
        "session_id": session_id,
        "hook_event_name": event,
        "cwd": cwd,
    })
    .to_string()
}

/// 二度と生き返らない「確実に死んでいる pid」を作る。
/// 回収（`wait`）まで済ませないとゾンビのまま生存扱いになる。
fn a_pid_that_is_certainly_gone() -> u32 {
    let mut child = Command::new("/usr/bin/true").spawn().unwrap();
    let pid = child.id();
    child.wait().unwrap();
    pid
}

/// **ゾンビセッションの回収はこれが成り立つことに全面的に依存している**:
/// hook は自分の親プロセス（＝セッションを持っている Claude Code）の pid を
/// 記録する。ここが狂うと、生きているセッションを死んだと誤判定して消して
/// しまうか、死んだセッションを永遠に表示し続けるかのどちらかになる。
///
/// テストプロセスが hook を直接 spawn するので、期待値は自分自身の pid。
#[test]
fn hook_records_the_owning_process_pid() {
    let dir = tempfile::TempDir::new().unwrap();
    let state = dir.path().join("state");
    let (code, stdout) = run_hook(
        &state,
        &[],
        &payload("sess-pid-1", "SessionStart", "/tmp/proj"),
    );
    assert_eq!(code, 0);
    assert!(stdout.is_empty(), "hook must not write to stdout");

    assert_eq!(
        saved_session(&state, "sess-pid-1")["pid"].as_u64(),
        Some(u64::from(std::process::id())),
        "hook は自分の親プロセスの pid を持ち主として記録するはず"
    );
}

/// **番人: プラグインが配るラッパー越しでも、セッションが生きたまま見えること。**
///
/// 実際に踏んだ不具合の再現テスト。`ccsessions-hook.sh` は `exec` を使わない
/// （「何があっても exit 0」を守るため）ので、`ccsessions hook` から見た直接の親は
/// **ラッパーの `sh`** になる。それを持ち主として記録していた頃は、hook が書いた
/// セッションの pid が数ミリ秒で死に、`ccsessionsd` が 0.5 秒以内に「pid が居ない」
/// と回収していた（`reaped session ... — pid 15801 が居ない`）。hook もストアも
/// daemon も正常なのに、**メニューバーには何も出ない**という壊れ方をする。
///
/// 単体テストは作り物の系図で `resolve_owner` を見るだけなので、ここでは
/// **本物のラッパーを本物の `sh` で起動して**、経路ごと固定する。ラッパーを
/// `exec` 無しのまま持ち主判定を直接の親に戻すと、このテストが落ちる。
///
/// テストプロセス（シェルではない）がラッパーを起こすので、claude 本体に
/// 相当する持ち主は自分自身。
#[test]
fn a_session_recorded_through_the_plugin_wrapper_is_still_live_afterwards() {
    let dir = tempfile::TempDir::new().unwrap();
    let state = dir.path().join("state");

    // ラッパーは `command -v ccsessions` で PATH から拾う。**PATH をこの
    // tempdir だけにして**、brew で入っている本物ではなくビルドしたバイナリを
    // 必ず使わせる（実ユーザの環境に結果を左右させない）。
    let bin_dir = dir.path().join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    std::os::unix::fs::symlink(bin(), bin_dir.join("ccsessions")).unwrap();

    let wrapper = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../plugins/ccsessions/hooks/ccsessions-hook.sh");
    assert!(
        wrapper.exists(),
        "プラグインのラッパーが見つからない: {wrapper:?}"
    );

    // `sh` は絶対パスで起こす（PATH をこの tempdir だけに絞るため、PATH からは
    // 引けない）。Claude Code が `sh "<script>"` を起動するのと同じ形。
    let mut child = Command::new("/bin/sh")
        .arg(&wrapper)
        .env("PATH", &bin_dir)
        .env("CCSESSIONS_STATE_DIR", &state)
        .env("CCSESSIONS_CONFIG", state.join("config.toml"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(payload("sess-wrapped", "SessionStart", "/tmp/wrapped-proj").as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(0), "ラッパーは必ず exit 0");
    assert!(out.stdout.is_empty(), "ラッパーは stdout を汚さない");

    // ここが要点。ラッパーの `sh` はもう終了しているので、その pid を記録して
    // いたら死んだセッション扱いになる。
    assert_eq!(
        saved_session(&state, "sess-wrapped")["pid"].as_u64(),
        Some(u64::from(std::process::id())),
        "ラッパーの sh ではなく、それを起動した側を持ち主として記録するはず"
    );

    let listed = Command::new(bin())
        .arg("list")
        .env("CCSESSIONS_STATE_DIR", &state)
        .env("CCSESSIONS_CONFIG", state.join("config.toml"))
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&listed.stdout);
    assert!(
        stdout.contains("wrapped-proj"),
        "ラッパー経由で作ったセッションが一覧に出るはず: {stdout:?}"
    );
}

/// 以後のイベントでも押し直されること（`--resume` で別プロセスが引き継いだ
/// ときに古い pid が残らないため）。
#[test]
fn hook_refreshes_the_pid_on_every_event() {
    let dir = tempfile::TempDir::new().unwrap();
    let state = dir.path().join("state");
    run_hook(
        &state,
        &[],
        &payload("sess-pid-2", "SessionStart", "/tmp/proj"),
    );

    // 別の pid が書かれた状態を作り、次のイベントで上書きされることを見る。
    let path = state.join("sessions").join("sess-pid-2.json");
    let mut saved = saved_session(&state, "sess-pid-2");
    saved["pid"] = serde_json::json!(999_999);
    fs::write(&path, saved.to_string()).unwrap();

    run_hook(
        &state,
        &[],
        &payload("sess-pid-2", "UserPromptSubmit", "/tmp/proj"),
    );

    let after = saved_session(&state, "sess-pid-2");
    assert_eq!(after["pid"].as_u64(), Some(u64::from(std::process::id())));
    assert_eq!(after["state"], "working");
}

/// `SessionEnd` は今までどおりファイルごと消す（正常終了の経路）。pid による
/// 回収は、この経路が**通らなかった**ときの受け皿でしかない。
#[test]
fn session_end_still_removes_the_file() {
    let dir = tempfile::TempDir::new().unwrap();
    let state = dir.path().join("state");
    run_hook(
        &state,
        &[],
        &payload("sess-end", "SessionStart", "/tmp/proj"),
    );
    let path = state.join("sessions").join("sess-end.json");
    assert!(path.exists());

    run_hook(&state, &[], &payload("sess-end", "SessionEnd", "/tmp/proj"));
    assert!(!path.exists(), "SessionEnd must delete the session file");
}

/// 死んだセッションが `ccsessions list` に出ないこと（表示側の回帰防止）。
///
/// hook で作ったセッションの pid を「確実に居ないプロセス」に差し替えて一覧を
/// 取る。同じ workdir で生きているセッションは残らなければならない。
#[test]
fn list_hides_sessions_whose_process_is_gone() {
    let dir = tempfile::TempDir::new().unwrap();
    let state = dir.path().join("state");
    for id in ["zombie", "alive"] {
        run_hook(
            &state,
            &[],
            &payload(id, "UserPromptSubmit", "/tmp/clean-workdir"),
        );
    }

    let path = state.join("sessions").join("zombie.json");
    let mut zombie = saved_session(&state, "zombie");
    zombie["pid"] = serde_json::json!(a_pid_that_is_certainly_gone());
    fs::write(&path, zombie.to_string()).unwrap();

    let out = Command::new(bin())
        .arg("list")
        .env("CCSESSIONS_STATE_DIR", &state)
        .env("CCSESSIONS_CONFIG", state.join("config.toml"))
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        stdout.lines().count(),
        1,
        "生きているセッション 1 件だけが出るはず: {stdout:?}"
    );
    assert!(stdout.contains("clean-workdir"));
}

// ---------------------------------------------------------------------------
// `ccsessions list` — ignore フィルタ
// ---------------------------------------------------------------------------

/// `config.toml` を tempdir に書く。**本物の `~/.config/ccsessions` には
/// 一切触らない** — `CCSESSIONS_CONFIG` で完全に隔離する。
fn write_config(state: &std::path::Path, ignore_patterns: &[&str]) {
    let list = ignore_patterns
        .iter()
        .map(|p| format!("{p:?}"))
        .collect::<Vec<_>>()
        .join(", ");
    fs::write(state.join("config.toml"), format!("ignore = [{list}]\n")).unwrap();
}

/// `write_config` に `max_sessions` も足した版。「live が `max_sessions` を
/// 超える」場面を作る回帰テスト（`--all` の枠外し・`doctor` の stale）に要る。
fn write_config_with_max(state: &std::path::Path, ignore_patterns: &[&str], max_sessions: usize) {
    let list = ignore_patterns
        .iter()
        .map(|p| format!("{p:?}"))
        .collect::<Vec<_>>()
        .join(", ");
    fs::write(
        state.join("config.toml"),
        format!("max_sessions = {max_sessions}\nignore = [{list}]\n"),
    )
    .unwrap();
}

fn list_output(state: &std::path::Path, extra_args: &[&str]) -> String {
    let out = Command::new(bin())
        .arg("list")
        .args(extra_args)
        .env("CCSESSIONS_STATE_DIR", state)
        .env("CCSESSIONS_CONFIG", state.join("config.toml"))
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// `ignore` に当たるセッションが `ccsessions list` から消えること。
#[test]
fn list_hides_ignored_sessions() {
    let dir = tempfile::TempDir::new().unwrap();
    let state = dir.path().join("state");
    run_hook(
        &state,
        &[],
        &payload("hidden", "UserPromptSubmit", "/tmp/cron-jobs"),
    );
    run_hook(
        &state,
        &[],
        &payload("shown", "UserPromptSubmit", "/tmp/visible"),
    );
    write_config(&state, &["**/cron-jobs/**"]);

    let stdout = list_output(&state, &[]);
    assert!(stdout.contains("visible"), "{stdout:?}");
    assert!(!stdout.contains("cron-jobs"), "{stdout:?}");
}

/// `--all` を付けると ignore を無視して全件出ること。
#[test]
fn list_all_shows_them_again() {
    let dir = tempfile::TempDir::new().unwrap();
    let state = dir.path().join("state");
    run_hook(
        &state,
        &[],
        &payload("hidden", "UserPromptSubmit", "/tmp/cron-jobs"),
    );
    write_config(&state, &["**/cron-jobs/**"]);

    let stdout = list_output(&state, &["--all"]);
    assert!(stdout.contains("cron-jobs"), "{stdout:?}");
    // `--all` のときは非表示件数の案内も出さない（全部出しているので不要）。
    assert!(!stdout.contains("hidden by ignore"), "{stdout:?}");
}

/// 非表示にした件数が一覧の末尾に 1 行出ること。
#[test]
fn list_reports_how_many_were_hidden() {
    let dir = tempfile::TempDir::new().unwrap();
    let state = dir.path().join("state");
    run_hook(
        &state,
        &[],
        &payload("hidden", "UserPromptSubmit", "/tmp/cron-jobs"),
    );
    run_hook(
        &state,
        &[],
        &payload("shown", "UserPromptSubmit", "/tmp/visible"),
    );
    write_config(&state, &["**/cron-jobs/**"]);

    let stdout = list_output(&state, &[]);
    assert!(
        stdout.contains("1 hidden by ignore; pass --all to show them"),
        "{stdout:?}"
    );
}

/// **回帰テスト。** `--all` は ignore だけでなく `max_sessions` の打ち切りも
/// 外すこと。`max_sessions` を残したまま ignore だけ外すと、live が枠を超える
/// 場面で `--all` を付けても隠れているセッションが永久に見えない
/// （隠した側が新しければ、逆に `--all` を付けたぶん実セッションの表示が
/// 減ることさえある）。
#[test]
fn list_all_is_not_capped_by_max_sessions() {
    let dir = tempfile::TempDir::new().unwrap();
    let state = dir.path().join("state");
    for i in 0..2 {
        run_hook(
            &state,
            &[],
            &payload(
                &format!("hidden-{i}"),
                "UserPromptSubmit",
                &format!("/tmp/cron-jobs/{i}"),
            ),
        );
    }
    for i in 0..3 {
        run_hook(
            &state,
            &[],
            &payload(
                &format!("visible-{i}"),
                "UserPromptSubmit",
                &format!("/tmp/visible-{i}"),
            ),
        );
    }
    // 枠を 2 に絞る。live は 5 件（ignore 対象 2 ＋ 非 ignore 3）なので枠を超える。
    write_config_with_max(&state, &["**/cron-jobs/**"], 2);

    let out = Command::new(bin())
        .arg("list")
        .arg("--json")
        .arg("--all")
        .env("CCSESSIONS_STATE_DIR", &state)
        .env("CCSESSIONS_CONFIG", state.join("config.toml"))
        .output()
        .unwrap();
    let parsed: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let sessions = parsed.as_array().expect("list --json は配列");
    assert_eq!(
        sessions.len(),
        5,
        "--all は max_sessions の打ち切りも外して全件出すべき: {}",
        String::from_utf8_lossy(&out.stdout)
    );
}

// ---------------------------------------------------------------------------
// `ccsessions doctor` — ignore の回帰（stale の二重計上）
// ---------------------------------------------------------------------------

/// **doctor の回帰テスト。** ignore で外したセッションを「掃除されていない
/// 死骸」（`stale entries`）として数えてはいけない。`live` から ignore ぶんを
/// 差し引くだけの実装だと、ignore に当たる生きたセッションが `stale` に化けて
/// この行が出てしまう。
///
/// あわせて **`max_sessions` を枠より多い live で超えさせる**（`max_sessions=2`
/// に live 4 件）。`shown.len()` だけを引く実装だと、枠から溢れた生きている
/// セッションまで死骸として数えられ、死骸が 1 件も無いのに `stale entries` が
/// 出てしまう（これは `live.ignored` の足し戻しだけでは直らない）。
#[test]
fn an_ignored_session_is_not_counted_as_stale() {
    let dir = tempfile::TempDir::new().unwrap();
    let state = dir.path().join("state");
    run_hook(
        &state,
        &[],
        &payload("hidden", "UserPromptSubmit", "/tmp/cron-jobs"),
    );
    for i in 0..3 {
        run_hook(
            &state,
            &[],
            &payload(
                &format!("visible-{i}"),
                "UserPromptSubmit",
                &format!("/tmp/visible-{i}"),
            ),
        );
    }
    write_config_with_max(&state, &["**/cron-jobs/**"], 2);

    let out = Command::new(bin())
        .arg("doctor")
        .env("CCSESSIONS_STATE_DIR", &state)
        .env("CCSESSIONS_CONFIG", state.join("config.toml"))
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        !stdout.contains("stale entries"),
        "ignore で外しただけのセッションを stale として数えてはいけない: {stdout:?}"
    );
    assert!(
        stdout.contains("ignored:"),
        "ignore で非表示にした件数を報告するべき: {stdout:?}"
    );
}
