//! `ccsessions hook` — Claude Code の hook から呼ばれる producer。
//!
//! stdin の JSON を読み、reducer に通し、結果をセッションストアへ反映する。
//! **何があっても exit 0 で終わる**契約: hook が失敗扱いになると Claude Code
//! 側の動作に影響するため、内部エラーはすべて stderr に 1 行出すだけに留め、
//! 呼び出し元へは常に成功として返す。標準出力には何も書かない
//! （Claude Code が hook の stdout を解釈する場合があるため）。
//!
//! `--record <dir>` を渡すと、Claude Code が実際に送ってくる hook payload を
//! そのままファイルへ収録できる（開発用）。パース成否に関わらず収録し、
//! 収録の失敗は通常の状態更新処理を妨げない。

use std::fs;
use std::io::{self, Read, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use ccsessions_core::event::{reduce, HookPayload, Outcome};
use ccsessions_core::{config, now_ms, store, transcript};

pub fn run(args: &[String]) -> i32 {
    let event_override = flag_value(args, "--event");
    let record_dir = flag_value(args, "--record");

    let mut input = String::new();
    if let Err(e) = std::io::stdin().read_to_string(&mut input) {
        eprintln!("ccsessions: hook: failed to read stdin: {e}");
        return 0;
    }

    let parsed: Result<HookPayload, serde_json::Error> = serde_json::from_str(&input);

    // `--record` はパース成否に関わらず生の stdin を収録する（パースに失敗
    // する payload こそ一番知りたい情報のため）。収録に失敗しても状態更新は
    // 通常どおり続行する（record_payload の呼び出し側でエラーを吸収する）。
    if let Some(dir) = &record_dir {
        let event_name = match &parsed {
            Ok(p) => {
                sanitize_event_name(event_override.as_deref().or(p.hook_event_name.as_deref()))
            }
            Err(_) => "unparsed".to_string(),
        };
        if let Err(e) = record_payload(Path::new(dir), &input, &event_name) {
            eprintln!("ccsessions: hook: failed to record payload: {e}");
        }
    }

    let mut payload: HookPayload = match parsed {
        Ok(p) => p,
        Err(e) => {
            eprintln!("ccsessions: hook: failed to parse payload JSON: {e}");
            return 0;
        }
    };
    // `--event` は payload の hook_event_name より優先する（payload に欠けて
    // いる場合の保険として使われる想定だが、明示的に渡された以上は上書きする）。
    if let Some(ev) = event_override {
        payload.hook_event_name = Some(ev);
    }

    let cfg = config::load(&ccsessions_core::config_path()).unwrap_or_else(|e| {
        eprintln!("ccsessions: hook: config load error, using defaults: {e}");
        config::builtin_default()
    });

    let is_stop = payload.hook_event_name.as_deref() == Some("Stop");
    let error = is_stop
        && cfg.detect_errors
        && payload
            .transcript_path
            .as_deref()
            .map(|p| transcript::last_turn_errored(Path::new(p)))
            .unwrap_or(false);

    let title = read_title(&payload);

    // ここから下（load → reduce → save/remove）が read-modify-write のクリティカル
    // セクション。Claude Code はマッチした hook をすべて**並列プロセス**で起動する
    // ので、排他しないと後勝ちで更新が消える（8 並列の SubagentStart で agents が
    // 2 件しか残らなかった）。`write_atomic` は途中状態を晒さないことしか保証しない。
    //
    // ロックが取れなくても hook は失敗させない — 「何があっても exit 0」の契約は
    // ここでも優先する。取れなければ警告を 1 行出して、ロック導入前と同じ経路で
    // 続行する（更新が消えるかもしれないが、更新を丸ごと落とすよりはよい）。
    //
    // transcript の読み取りと config のロードはロックの外に置いてある（最大 64KB
    // のファイル読みを他プロセスに待たせる理由が無い）。
    let _store_lock = match store::lock_exclusive() {
        Ok(lock) => Some(lock),
        Err(e) => {
            eprintln!("ccsessions: hook: proceeding without the store lock: {e}");
            None
        }
    };

    let prev = match payload.session_id.as_deref() {
        Some(id) if !id.is_empty() => match store::load(id) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("ccsessions: hook: failed to load session {id:?}: {e}");
                None
            }
        },
        _ => None,
    };

    match reduce(prev, &payload, now_ms(), error) {
        Outcome::Upsert(mut s) => {
            // セッションの持ち主（＝この hook を起動した Claude Code プロセス）を
            // 毎回記録し直す。`SessionEnd` が飛ばない終わり方をしたセッションは、
            // この pid の生存だけが「もう居ない」と分かる手掛かりになる。
            //
            // reducer に渡さずここで押すのは、pid が payload（stdin の JSON）では
            // なく**実行環境から取れる事実**だから — `reduce` を純関数のまま
            // 保つ方針に従い、環境を触るのは I/O 層のこちらの責務にする。
            // 毎イベント上書きなので、`--resume` で別プロセスが引き継いだ場合も
            // 次のイベントで自然に追従する。
            s.pid = ccsessions_core::process::owner_pid();
            // タイトルも payload には無く transcript から取る事実なので、pid と
            // 同じくここで押す。**取れなかったときに既存の値を消さない** —
            // タイトル行が tail に届かない回があっても、一度出た名前が
            // ちらつきながら消えるより据え置く方がよい。
            if title.is_some() {
                s.title = title;
            }
            if let Err(e) = store::save(&s) {
                eprintln!("ccsessions: hook: failed to save session {:?}: {}", s.id, e);
            }
        }
        Outcome::Remove(id) => {
            if let Err(e) = store::remove(&id) {
                eprintln!("ccsessions: hook: failed to remove session {id:?}: {e}");
            }
        }
        Outcome::Ignore => {}
    }

    0
}

/// このイベントでタイトルを読むか。
///
/// **全イベントでは読まない。** `SubagentStart`/`SubagentStop`/`Notification` は
/// 1 ターンに何度も飛ぶのに対し、タイトルはターンをまたいで滅多に変わらない。
/// ターンの区切り（プロンプト・停止）とセッション開始だけに絞れば、読む回数は
/// ターンあたり数回に収まる。
fn wants_title(event: Option<&str>) -> bool {
    matches!(
        event,
        Some("SessionStart") | Some("UserPromptSubmit") | Some("Stop") | Some("StopFailure")
    )
}

/// transcript からセッションタイトルを読む。読まない・読めない場合は `None`。
///
/// ストアのロックを取る**前**に呼ぶこと（最大 256KB のファイル読みで他の hook
/// プロセスを待たせない — `last_turn_errored` と同じ理由）。
fn read_title(payload: &HookPayload) -> Option<String> {
    if !wants_title(payload.hook_event_name.as_deref()) {
        return None;
    }
    let path = payload.transcript_path.as_deref()?;
    let session_id = payload.session_id.as_deref()?;
    if session_id.is_empty() {
        return None;
    }
    transcript::session_title(Path::new(path), session_id)
}

/// `--flag <value>` の値を取り出す。値が続いていなければ `None`。
///
/// 値なしを **`None` に畳むのは意図的**。ここで異常終了すると
/// 「何があっても exit 0」の契約を破ることになるので、引数の書き損じは
/// 「そのフラグは無かった」として扱う。
fn flag_value(args: &[String], flag: &str) -> Option<String> {
    let i = args.iter().position(|a| a == flag)?;
    args.get(i + 1).cloned()
}

// ---------------------------------------------------------------------------
// --record
// ---------------------------------------------------------------------------

/// 生の payload をファイルへ収録する。
///
/// payload には `prompt`（ユーザが打った生プロンプト）・
/// `last_assistant_message`・`cwd` が含まれうるので、**新規に作る**ディレクトリは
/// 0700、ファイルは 0600 にして他ユーザから読めないようにする。
///
/// ディレクトリの権限は `DirBuilder::mode` で「作るときに」与える。
/// `set_permissions` で後から与えると、`--record ~/Documents` のように**既存の**
/// ディレクトリを指されたときにそれを 0700 へ chmod してしまう（ユーザの持ち物を
/// 勝手に変える破壊的な副作用）。収録機能が持ってよい権限ではない。
/// `recursive(true)` は既存ディレクトリをエラーにせず、権限も変更しない。
///
/// ファイル側は `OpenOptions::mode` が umask で緩められうるため、書き込み後に
/// `set_permissions` で 0600 を明示的に固定する（こちらは常に自分が作った
/// ファイルなので、既存物を書き換える心配がない）。
fn record_payload(dir: &Path, raw: &str, event: &str) -> io::Result<()> {
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)?;

    static SEQ: AtomicU64 = AtomicU64::new(0);
    // <pid> は並列に起動されうる複数の hook プロセスを区別するため、<seq> は
    // 同一プロセス内で複数回書く場合の衝突を避けるため（ccsessions_core の
    // write_atomic の tmp ファイル名と同じパターン）。hook はマッチした
    // 設定ごとに別プロセスとして並列に起動されるため、pid が無いと
    // プロセス内 seq だけでは同一ミリ秒に複数プロセスの出力ファイル名が
    // 衝突しうる。
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let path = dir.join(format!(
        "{}-{}-{}-{}.json",
        now_ms(),
        event,
        std::process::id(),
        seq
    ));

    let mut f = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&path)?;
    f.write_all(raw.as_bytes())?;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
    Ok(())
}

/// hook イベント名をファイル名の一部として安全に使えるよう正規化する。
///
/// payload は外部入力であり、`hook_event_name` をそのままファイル名に混ぜる
/// と `../` 等でパストラバーサルに使われうる。`ccsessions_core::store` の
/// `validate_session_id`（セッション id をファイル名に使う前に検証する
/// 関数）と同じ理由で、許可文字だけを残すホワイトリスト方式にする。
fn sanitize_event_name(name: Option<&str>) -> String {
    let raw = name.unwrap_or("");
    let cleaned: String = raw
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect();
    // 先頭の '.' はドットファイル扱いされ、一覧コマンドから隠れたり
    // 隠しファイルとして扱われたりするため許さない。
    let trimmed = cleaned.trim_start_matches('.');
    let truncated: String = trimmed.chars().take(40).collect();
    if truncated.is_empty() {
        "unknown".to_string()
    } else {
        truncated
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- sanitize_event_name -------------------------------------------------

    #[test]
    fn sanitize_event_name_passes_through_normal_names() {
        assert_eq!(sanitize_event_name(Some("Stop")), "Stop");
        assert_eq!(sanitize_event_name(Some("PreToolUse")), "PreToolUse");
    }

    #[test]
    fn sanitize_event_name_replaces_unsafe_characters() {
        // '/' は '_' に置換され、内部の '.' はそのまま残る（先頭だけ剥がす）。
        assert_eq!(
            sanitize_event_name(Some("../../etc/passwd")),
            "_.._etc_passwd"
        );
        assert_eq!(sanitize_event_name(Some("a/b c")), "a_b_c");
    }

    #[test]
    fn sanitize_event_name_strips_leading_dots() {
        assert_eq!(sanitize_event_name(Some(".hidden")), "hidden");
        assert_eq!(sanitize_event_name(Some("...")), "unknown");
    }

    #[test]
    fn sanitize_event_name_defaults_to_unknown() {
        assert_eq!(sanitize_event_name(None), "unknown");
        assert_eq!(sanitize_event_name(Some("")), "unknown");
    }

    #[test]
    fn sanitize_event_name_truncates_to_40_chars() {
        let long = "A".repeat(100);
        let sanitized = sanitize_event_name(Some(&long));
        assert_eq!(sanitized.len(), 40);
        assert_eq!(sanitized, "A".repeat(40));
    }

    // ---- flag_value ------------------------------------------------------------

    #[test]
    fn flag_value_finds_the_value_after_the_flag() {
        let args = vec![
            "--event".to_string(),
            "Stop".to_string(),
            "--record".to_string(),
            "/tmp/rec".to_string(),
        ];
        assert_eq!(flag_value(&args, "--record"), Some("/tmp/rec".to_string()));
        assert_eq!(flag_value(&args, "--event"), Some("Stop".to_string()));
    }

    #[test]
    fn flag_value_absent_returns_none() {
        let args = vec!["--event".to_string(), "Stop".to_string()];
        assert_eq!(flag_value(&args, "--record"), None);
    }

    /// 値が続いていないフラグでも `None` に畳む（exit 0 の契約を守るため）。
    #[test]
    fn a_flag_without_a_value_returns_none() {
        let args = vec!["--record".to_string()];
        assert_eq!(flag_value(&args, "--record"), None);
    }

    // ---- title -----------------------------------------------------------------

    #[test]
    fn only_turn_boundary_events_read_the_title() {
        for e in ["SessionStart", "UserPromptSubmit", "Stop", "StopFailure"] {
            assert!(wants_title(Some(e)), "{e} should read the title");
        }
        for e in [
            "SubagentStart",
            "SubagentStop",
            "Notification",
            "SessionEnd",
        ] {
            assert!(!wants_title(Some(e)), "{e} should not read the title");
        }
        assert!(!wants_title(None));
    }

    fn title_payload(dir: &Path, event: &str, session_id: &str) -> HookPayload {
        let transcript = dir.join("t.jsonl");
        fs::write(
            &transcript,
            "{\"type\":\"ai-title\",\"aiTitle\":\"顔の SVG 出力を直す\",\"sessionId\":\"s1\"}\n",
        )
        .unwrap();
        HookPayload {
            session_id: Some(session_id.to_string()),
            transcript_path: Some(transcript.display().to_string()),
            hook_event_name: Some(event.to_string()),
            ..Default::default()
        }
    }

    #[test]
    fn read_title_picks_up_the_title_from_the_transcript() {
        let dir = tempfile::TempDir::new().unwrap();
        let p = title_payload(dir.path(), "Stop", "s1");
        assert_eq!(read_title(&p).as_deref(), Some("顔の SVG 出力を直す"));
    }

    #[test]
    fn read_title_returns_none_for_a_high_frequency_event() {
        let dir = tempfile::TempDir::new().unwrap();
        let p = title_payload(dir.path(), "SubagentStart", "s1");
        assert_eq!(read_title(&p), None);
    }

    #[test]
    fn read_title_returns_none_without_a_transcript_path() {
        let p = HookPayload {
            session_id: Some("s1".into()),
            hook_event_name: Some("Stop".into()),
            ..Default::default()
        };
        assert_eq!(read_title(&p), None);
    }

    // ---- record_payload --------------------------------------------------------

    #[test]
    fn record_payload_writes_file_with_expected_permissions() {
        let dir = tempfile::TempDir::new().unwrap();
        let target = dir.path().join("rec");
        record_payload(&target, r#"{"a":1}"#, "Stop").unwrap();

        let entries: Vec<_> = fs::read_dir(&target).unwrap().collect();
        assert_eq!(entries.len(), 1);
        let entry = entries.into_iter().next().unwrap().unwrap();

        assert!(entry.file_name().to_string_lossy().contains("-Stop-"));
        let content = fs::read_to_string(entry.path()).unwrap();
        assert_eq!(content, r#"{"a":1}"#);

        let file_mode = fs::metadata(entry.path()).unwrap().permissions().mode() & 0o777;
        assert_eq!(file_mode, 0o600);
        let dir_mode = fs::metadata(&target).unwrap().permissions().mode() & 0o777;
        assert_eq!(dir_mode, 0o700);
    }

    #[test]
    fn record_payload_does_not_chmod_an_existing_directory() {
        // `--record ~/Documents` のように既存ディレクトリを指されても、その
        // 権限を勝手に 0700 へ変えてはいけない（ユーザの持ち物を書き換える）。
        let dir = tempfile::TempDir::new().unwrap();
        let target = dir.path().join("preexisting");
        fs::create_dir(&target).unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o755)).unwrap();

        record_payload(&target, "{}", "Stop").unwrap();

        let mode = fs::metadata(&target).unwrap().permissions().mode() & 0o777;
        assert_eq!(
            mode, 0o755,
            "an existing directory's permissions must be left untouched"
        );
        let entry = fs::read_dir(&target).unwrap().next().unwrap().unwrap();
        let file_mode = fs::metadata(entry.path()).unwrap().permissions().mode() & 0o777;
        assert_eq!(
            file_mode, 0o600,
            "the payload file itself must still be 0600"
        );
    }

    #[test]
    fn record_payload_filename_includes_pid_and_seq() {
        let dir = tempfile::TempDir::new().unwrap();
        record_payload(dir.path(), "{}", "Stop").unwrap();
        record_payload(dir.path(), "{}", "Stop").unwrap();

        let entries: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(entries.len(), 2, "two distinct files expected: {entries:?}");
        let pid = std::process::id().to_string();
        for name in &entries {
            assert!(name.contains(&pid), "{name} should contain pid {pid}");
        }
    }
}
