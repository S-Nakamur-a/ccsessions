//! Claude Code の settings.json を**読む**ためのヘルパー。`doctor` 専用。
//!
//! **ここには書き込みが 1 つも無い。** hook を settings.json に配線するのは
//! Claude Code プラグイン（`plugins/ccsessions/`）の仕事で、ccsessions は
//! 他人の設定ファイルを一切書き換えない
//! （[ADR 0021](../../docs/adr/0021-distribution.md)）。かつてあった
//! `install-hooks` / `uninstall-hooks` はプラグインに置き換えて消した。
//!
//! 読む目的は診断だけで、見るものは 2 つ。
//!
//! - **`hooks`** … 手で書かれた（あるいは旧版が書いた）ccsessions のエントリ。
//!   目印は `MARKER` の部分一致（`ccsessions_events`）。
//! - **`enabledPlugins`** … プラグイン経由の配線（`enabled_ccsessions_plugins`）。
//!   プラグインが配る hook は `hooks` に現れないので、こちらでしか分からない。
//!
//! Claude Code の設定はユーザ全体・プロジェクト・ローカルに分かれて同居しうる
//! ので、`known_settings_paths` が候補を列挙して全部読む。

use std::path::{Path, PathBuf};
use std::{fs, io};

/// ccsessions 自身の hook であることを判定する目印。command 文字列にこの
/// 部分文字列が含まれていれば ccsessions のエントリとみなす。
///
/// **プラグインが配る hook はこれに一致しない**（`hooks.json` の command は
/// `sh "${CLAUDE_PLUGIN_ROOT}/hooks/ccsessions-hook.sh"`）。ここで拾えるのは、
/// 手で書かれた配線と、`install-hooks` があった頃の残骸だけ。
pub const MARKER: &str = "ccsessions hook";

/// マッチャー無しで単純にコマンドを追加するイベント一覧。
///
/// `PreToolUse` はここに含めない: サブエージェントの起動追跡を
/// `SubagentStart`（実際に起動した）に一本化したため。以前のように
/// `PreToolUse(Agent|Task)`（ツールを呼んだ時点）も購読すると、1 回の
/// サブエージェント起動で agents を二重に push してしまう。どちらか
/// 一方でなければならず、より正確な `SubagentStart` を採る。
pub const SIMPLE_EVENTS: [&str; 10] = [
    "SessionStart",
    "UserPromptSubmit",
    "Notification",
    "PermissionRequest",
    "SubagentStart", // サブエージェントの起動追跡（agent_id 照合の起点）
    "SubagentStop",
    "PostToolBatch", // 判断待ち（WaitUser）からの復帰
    "Stop",
    "StopFailure", // API エラーで終わったターンの検出（Stop の代わりに飛ぶ）
    "SessionEnd",
];

// 以下の timeout 3 つは `#[cfg(test)]`。**hook を書くコードがもう無い**
// （settings.json に書くのは Claude Code プラグイン）ので、実行時に参照する
// ものが 1 つも無い一方、ADR 0006 の契約そのものはここにしか書かれていない。
// プラグインの `hooks.json` がこの契約を満たしているかを検証する仕様として
// 残してある（`the_plugin_declares_the_timeout_the_contract_requires`）。
// 消すと、`hooks.json` の timeout を誰でも黙って書き換えられるようになる。

/// hook エントリの既定 `timeout`（秒）。Claude Code は明示しないとほとんどの
/// イベントで既定 600 秒を適用するため、ccsessions の hook が万一ブロックすると
/// 最悪ユーザのターンが 10 分止まりうる。`ccsessions hook` は数 ms で終わる
/// ので 5 秒あれば十分すぎる。
#[cfg(test)]
pub const HOOK_TIMEOUT_SECS: u64 = 5;

/// `SessionEnd` に書く `timeout`（秒）。
///
/// **`SessionEnd` だけは Claude Code 側の既定が 1.5 秒**（バイナリ 2.1.220 の
/// `timeoutMs: M$o`、`M$o = 1500`）で、他イベントの 600 秒とは桁が違う。ここに
/// 一律で 5 を書くと**予算を 1.5 秒から 5 秒へ引き上げる**ことになり、
/// 「表示ツールがユーザの待ち時間を延ばさない」という意図と逆になる。
/// 既定より短い値を書いて意図を保つ。
///
/// 参考（実測した既定値）: 全般 600 秒 / `UserPromptSubmit` 30 秒 /
/// `MessageDisplay` 10 秒 / `SessionEnd` 1.5 秒。購読しているイベントのうち
/// 既定が 5 秒を下回るのは `SessionEnd` だけ。
#[cfg(test)]
pub const SESSION_END_TIMEOUT_SECS: u64 = 1;

/// そのイベントに書くべき `timeout`（秒）。
///
/// **ccsessions が書く timeout は、そのイベントの既定を決して上回らない**という
/// のがここの契約。新しく購読するイベントを足すときは、そのイベントの既定が
/// 5 秒より短くないかを確認すること。
#[cfg(test)]
pub fn hook_timeout_secs(event: &str) -> u64 {
    if event == "SessionEnd" {
        SESSION_END_TIMEOUT_SECS
    } else {
        HOOK_TIMEOUT_SECS
    }
}

/// Claude Code プラグインとしての名前。`enabledPlugins` のキーは
/// `<plugin>@<marketplace>` なので、`@` の左がこれなら我々のもの。
pub const PLUGIN_NAME: &str = "ccsessions";

/// `enabledPlugins` で**有効になっている** ccsessions のプラグイン一覧
/// （キーそのものを返す。例 `"ccsessions@ccsessions-marketplace"`）。
///
/// プラグインが配る hook は `settings.json` の `hooks` に現れないので、
/// `MARKER` の走査では絶対に見つからない。プラグイン経由で入れた人へ
/// 「NOT installed」と嘘をつかないために、`doctor` はこちらも見る
/// （[ADR 0021](../../docs/adr/0021-distribution.md)）。
///
/// marketplace 名を固定で照合しないのは、同じプラグインを別名の
/// marketplace から入れた人も拾うため。
pub fn enabled_ccsessions_plugins(root: &serde_json::Value) -> Vec<String> {
    let Some(obj) = root.get("enabledPlugins").and_then(|v| v.as_object()) else {
        return Vec::new();
    };
    let mut found: Vec<String> = obj
        .iter()
        .filter(|(_, enabled)| enabled.as_bool() == Some(true))
        .map(|(key, _)| key)
        .filter(|key| key.split('@').next() == Some(PLUGIN_NAME))
        .cloned()
        .collect();
    found.sort();
    found
}

pub fn home_dir() -> PathBuf {
    #[allow(deprecated)]
    std::env::home_dir().expect("$HOME is not set")
}

/// Claude Code が読みうる settings ファイルの候補を、ユーザ全体 → カレント
/// ディレクトリ → その祖先の順で列挙する（存在確認はしない純関数）。
///
/// プロジェクトのサブディレクトリで打っても拾えるように祖先まで辿る。
/// **enterprise の managed settings とプラグイン由来の hook はここに現れない**
/// ―― 走査で「無い」と言えるのはこの一覧の範囲だけ、という限界がある。
pub fn known_settings_paths(home: &Path, cwd: &Path) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = Vec::new();
    let push_dir = |paths: &mut Vec<PathBuf>, dir: &Path| {
        for name in ["settings.json", "settings.local.json"] {
            let p = dir.join(".claude").join(name);
            if !paths.contains(&p) {
                paths.push(p);
            }
        }
    };
    push_dir(&mut paths, home);
    let mut dir = Some(cwd);
    while let Some(d) = dir {
        push_dir(&mut paths, d);
        dir = d.parent();
    }
    paths
}

/// `root` に含まれる ccsessions のエントリを「イベント名 → 件数」で返す
/// （イベント名の昇順）。在り処の報告と診断で使う。
pub fn ccsessions_events(root: &serde_json::Value) -> Vec<(String, usize)> {
    let Some(hooks) = root.get("hooks").and_then(serde_json::Value::as_object) else {
        return Vec::new();
    };
    let mut found: Vec<(String, usize)> = hooks
        .iter()
        .filter_map(|(event, groups)| {
            let count = groups
                .as_array()?
                .iter()
                .filter_map(|g| g.get("hooks")?.as_array())
                .flatten()
                .filter(|h| {
                    h.get("command")
                        .and_then(serde_json::Value::as_str)
                        .map(|c| c.contains(MARKER))
                        .unwrap_or(false)
                })
                .count();
            (count > 0).then(|| (event.clone(), count))
        })
        .collect();
    found.sort();
    found
}

/// 自分自身の絶対パス。取得できなければ `PATH` 解決に委ねる `"ccsessions"`。
pub fn current_exe_string() -> String {
    std::env::current_exe()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "ccsessions".to_string())
}

/// `path` を読み、無ければ空オブジェクト `{}` として扱う（settings.json が
/// まだ存在しない環境でも診断が動くように）。
pub fn read_or_empty(path: &Path) -> io::Result<serde_json::Value> {
    let content = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(serde_json::json!({})),
        Err(e) => return Err(e),
    };
    serde_json::from_str(&content)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("invalid JSON: {e}")))
}

/// `root["hooks"][event]` に ccsessions の hook（command に `MARKER` を含む
/// もの）が 1 つでもあるか。command の完全一致では別マシンでビルドした別パスの
/// 実行ファイルが仕込んだエントリを見分けられないので、`MARKER` の部分一致で
/// 判定する。`doctor` が「どのイベントが未導入か」を調べるのに使う。
pub fn event_has_ccsessions_entry(root: &serde_json::Value, event: &str) -> bool {
    root.get("hooks")
        .and_then(|h| h.get(event))
        .and_then(serde_json::Value::as_array)
        .map(|groups| {
            groups.iter().any(|g| {
                g.get("hooks")
                    .and_then(serde_json::Value::as_array)
                    .map(|list| {
                        list.iter().any(|h| {
                            h.get("command")
                                .and_then(serde_json::Value::as_str)
                                .map(|c| c.contains(MARKER))
                                .unwrap_or(false)
                        })
                    })
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false)
}

/// ccsessions の hook のうち `--record` を付けたまま settings.json に残っている
/// ものがあるイベント名を返す。
///
/// `--record` は受け取った payload をそのままディスクへ落とす開発用の機能で、
/// payload には**ユーザが打った生のプロンプト**（`prompt`）や
/// `last_assistant_message` が入る。収録用プロファイルを本番の settings.json に
/// 付けっぱなしにするのが最も起こりやすい事故なので、`doctor` で気づけるようにする。
pub fn events_with_recording_enabled(root: &serde_json::Value) -> Vec<String> {
    let Some(hooks) = root.get("hooks").and_then(serde_json::Value::as_object) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for (event, groups) in hooks {
        let recording = groups
            .as_array()
            .map(|gs| {
                gs.iter().any(|g| {
                    g.get("hooks")
                        .and_then(serde_json::Value::as_array)
                        .map(|list| {
                            list.iter().any(|h| {
                                h.get("command")
                                    .and_then(serde_json::Value::as_str)
                                    .map(|c| c.contains(MARKER) && c.contains("--record"))
                                    .unwrap_or(false)
                            })
                        })
                        .unwrap_or(false)
                })
            })
            .unwrap_or(false);
        if recording {
            found.push(event.clone());
        }
    }
    found.sort();
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Claude Code プラグインが配る hook 定義。**同じ内容を 2 か所に持つ**ので
    /// （Rust の定数と、プラグインの `hooks.json`）、真実は
    /// この module の定数 1 か所だと決めて、ずれたらテストで落とす。
    fn plugin_hooks_json() -> serde_json::Value {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../plugins/ccsessions/hooks/hooks.json");
        let raw = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("{} が読めない: {e}", path.display()));
        serde_json::from_str(&raw).expect("プラグインの hooks.json が JSON として壊れている")
    }

    #[test]
    fn the_plugin_subscribes_to_exactly_the_declared_events() {
        // ここがずれると、購読しているつもりのイベントが実際には届かない
        // （SubagentStart を落とせばエージェント待ちが一切出なくなる）。
        let hooks = plugin_hooks_json();
        let obj = hooks["hooks"]
            .as_object()
            .expect("hooks オブジェクトが無い");
        let mut in_plugin: Vec<&str> = obj.keys().map(String::as_str).collect();
        in_plugin.sort_unstable();
        let mut expected: Vec<&str> = SIMPLE_EVENTS.to_vec();
        expected.sort_unstable();
        assert_eq!(in_plugin, expected);
    }

    #[test]
    fn the_plugin_declares_the_timeout_the_contract_requires() {
        // timeout の契約（ADR 0006）＝ そのイベントの Claude Code 既定を
        // 上回らない。プラグイン側だけ抜けると SessionEnd で 1.5 秒の既定を
        // 使うことになり、意図が静かに失われる。
        let hooks = plugin_hooks_json();
        for event in SIMPLE_EVENTS {
            let entry = &hooks["hooks"][event][0]["hooks"][0];
            assert_eq!(
                entry["timeout"].as_u64(),
                Some(hook_timeout_secs(event)),
                "{event} の timeout がプラグインと CLI でずれている"
            );
        }
    }

    #[test]
    fn the_plugin_runs_its_bundled_wrapper_rather_than_a_baked_absolute_path() {
        // 絶対パスを焼くと `brew upgrade` で Cellar が変わった瞬間に hook が
        // 壊れる。プラグインは必ず ${CLAUDE_PLUGIN_ROOT} 経由で呼ぶ。
        let hooks = plugin_hooks_json();
        for event in SIMPLE_EVENTS {
            let cmd = hooks["hooks"][event][0]["hooks"][0]["command"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            assert!(
                cmd.contains("${CLAUDE_PLUGIN_ROOT}"),
                "{event} が CLAUDE_PLUGIN_ROOT を経由していない: {cmd}"
            );
        }
    }

    #[test]
    fn an_enabled_plugin_is_found_regardless_of_which_marketplace_it_came_from() {
        let root = json!({"enabledPlugins": {
            "ccsessions@ccsessions-marketplace": true,
            "ccsessions@someones-fork": true,
        }});
        assert_eq!(
            enabled_ccsessions_plugins(&root),
            vec![
                "ccsessions@ccsessions-marketplace",
                "ccsessions@someones-fork"
            ]
        );
    }

    #[test]
    fn a_disabled_plugin_does_not_count_as_installed() {
        // `/plugin` は無効化してもキーを残す。値を見ないと、外したのに
        // 「入っている」と言い続ける。
        let root = json!({"enabledPlugins": {"ccsessions@ccsessions-marketplace": false}});
        assert!(enabled_ccsessions_plugins(&root).is_empty());
    }

    #[test]
    fn other_peoples_plugins_are_never_reported_as_ours() {
        let root = json!({"enabledPlugins": {
            "conductor@conductor-marketplace": true,
            "ccsessions-extra@somewhere": true,
        }});
        assert!(enabled_ccsessions_plugins(&root).is_empty());
        assert!(enabled_ccsessions_plugins(&json!({})).is_empty());
    }

    #[test]
    fn timeout_is_shorter_for_session_end_than_the_default() {
        // SessionEnd の Claude Code 側の既定は 1.5 秒しかない。ccsessions が書く値は
        // そのイベントの既定を上回ってはいけない。
        assert_eq!(hook_timeout_secs("SessionEnd"), 1);
        assert_eq!(hook_timeout_secs("Stop"), HOOK_TIMEOUT_SECS);
        assert_eq!(hook_timeout_secs("PostToolBatch"), HOOK_TIMEOUT_SECS);
    }

    #[test]
    fn detects_a_ccsessions_hook_left_with_record_enabled() {
        // --record を本番の settings.json に残すと、ユーザの生プロンプトが
        // ディスクに溜まり続ける。doctor が気づけること。
        let root = json!({"hooks": {
            "Stop": [{"hooks": [
                {"type": "command", "command": "/bin/ccsessions hook --record /tmp/rec"}
            ]}],
            "SessionStart": [{"hooks": [
                {"type": "command", "command": "/bin/ccsessions hook"}
            ]}],
        }});
        assert_eq!(events_with_recording_enabled(&root), vec!["Stop"]);
    }

    #[test]
    fn other_tools_arguments_are_not_mistaken_for_recording() {
        // 他ツールが --record を持っていても ccsessions のものではない。
        let root = json!({"hooks": {
            "Stop": [{"hooks": [
                {"type": "command", "command": "some-other-tool --record /tmp/x"}
            ]}],
        }});
        assert!(events_with_recording_enabled(&root).is_empty());
    }

    #[test]
    fn no_recording_and_malformed_shapes_are_empty() {
        assert!(events_with_recording_enabled(&json!({})).is_empty());
        assert!(events_with_recording_enabled(&json!({"hooks": "nope"})).is_empty());
        assert!(events_with_recording_enabled(&json!({"hooks": {"Stop": "nope"}})).is_empty());
    }

    #[test]
    fn known_paths_cover_the_user_file_and_every_ancestor_of_the_cwd() {
        let home = Path::new("/Users/x");
        let paths = known_settings_paths(home, Path::new("/Users/x/work/repo/sub"));
        assert_eq!(paths[0], PathBuf::from("/Users/x/.claude/settings.json"));
        assert_eq!(
            paths[1],
            PathBuf::from("/Users/x/.claude/settings.local.json")
        );
        // プロジェクトのサブディレクトリで打っても、リポジトリ直下の
        // .claude/settings.json を見落とさないこと。
        assert!(paths.contains(&PathBuf::from("/Users/x/work/repo/.claude/settings.json")));
        assert!(paths.contains(&PathBuf::from(
            "/Users/x/work/repo/.claude/settings.local.json"
        )));
        // home が cwd の祖先でもあるので、同じパスが 2 度出ないこと。
        let user = PathBuf::from("/Users/x/.claude/settings.json");
        assert_eq!(paths.iter().filter(|p| **p == user).count(), 1);
    }

    #[test]
    fn ccsessions_events_counts_only_our_own_entries() {
        let root = json!({"hooks": {
            "Stop": [
                {"hooks": [{"type": "command", "command": "othertool"}]},
                {"hooks": [{"type": "command", "command": "/b/ccsessions hook"}]}
            ],
            "PreToolUse": [{"hooks": [
                {"type": "command", "command": "/b/ccsessions hook"},
                {"type": "command", "command": "/old/ccsessions hook"}
            ]}],
            "SessionEnd": [{"hooks": [{"type": "command", "command": "othertool"}]}],
        }});
        assert_eq!(
            ccsessions_events(&root),
            vec![("PreToolUse".to_string(), 2), ("Stop".to_string(), 1)]
        );
        assert!(ccsessions_events(&json!({})).is_empty());
    }
}
