//! `ccsessions doctor` — 現在の設定・状態ディレクトリ・hook 導入状況を
//! まとめて表示する診断コマンド。

use ccsessions_core::config::{BarAlign, Placement};
use ccsessions_core::{config, now_ms, state_dir, store};

use std::path::{Path, PathBuf};

use crate::settings_json::{
    ccsessions_events, current_exe_string, enabled_ccsessions_plugins, event_has_ccsessions_entry,
    events_with_recording_enabled, home_dir, known_settings_paths, read_or_empty, SIMPLE_EVENTS,
};

pub fn run() -> i32 {
    let cfg_path = ccsessions_core::config_path();
    let state_dir = state_dir();
    let cfg = config::load(&cfg_path).unwrap_or_else(|_| config::builtin_default());
    let live = store::list_live(now_ms(), cfg.session_ttl_ms(), cfg.max_sessions);
    // ディスクに残っているファイル数との差 ＝ まだ掃除されていない死んだセッション。
    // 「一覧に出ないのにファイルがある」を目で確かめられるようにしておく
    // （掃除するのは常駐している ccsessionsd の役目なので、差が減らないままなら
    //  daemon が動いていないというサインになる）。
    let stale = store::list().len().saturating_sub(live.len());

    // Claude Code の設定は複数ファイルに分かれて同居しうる（ユーザ全体・
    // プロジェクト・ローカル）。1 ファイルだけ見ると、hook を別のファイルに
    // 置いた人へ「NOT installed」と嘘をつく。読むのは全候補にする
    // （書くのは名指しされたファイルだけ、という規則とは非対称でよい）。
    // 読めない（未作成・壊れた JSON）ファイルは「何も入っていない」として扱う。
    let installed: Vec<(PathBuf, Vec<(String, usize)>)> =
        known_settings_paths(&home_dir(), &cwd_or_dot())
            .into_iter()
            .filter(|p| p.exists())
            .map(|path| {
                let root = read_or_empty(&path).unwrap_or_else(|_| serde_json::json!({}));
                (path, root)
            })
            .filter_map(|(path, root)| {
                let events = ccsessions_events(&root);
                (!events.is_empty()).then_some((path, events))
            })
            .collect();
    // 欠落イベントは全ファイルの和で判定する（Claude Code 側もマージして読む）。
    let covered: Vec<&str> = SIMPLE_EVENTS
        .iter()
        .copied()
        .filter(|event| {
            installed
                .iter()
                .any(|(_, events)| events.iter().any(|(e, _)| e == event))
        })
        .collect();
    let missing_events: Vec<&str> = SIMPLE_EVENTS
        .iter()
        .copied()
        .filter(|e| !covered.contains(e))
        .collect();

    println!(
        "config path:     {} ({})",
        cfg_path.display(),
        exists_label(cfg_path.exists())
    );
    println!(
        "state dir:       {} ({})",
        state_dir.display(),
        exists_label(state_dir.exists())
    );
    println!("live sessions:   {}", live.len());
    if stale > 0 {
        println!(
            "stale entries:   {stale} (次の掃除で消える。ccsessionsd が常駐していれば 1 分以内)"
        );
    }
    println!("placement:       {}", cfg.placement.as_str());
    println!("bar_align:       {}", cfg.bar_align.as_str());
    println!("compact_flock:   {}", cfg.compact_flock.as_str());
    if cfg.placement == Placement::Bar && cfg.bar_align == BarAlign::Center {
        // `bar_align` は bar 配置のときにしか意味を持たない（dock は画面下部中央に
        // 固定で、ノッチとは無関係）。`placement` は cfg から静的に分かるので、
        // dock のまま bar_align=center が残っている（bar で試したあと dock に
        // 戻した等）ケースでは警告を出さない ― 出すと「群れが隠れる」という
        // 事実に反する診断になり、しかも auto にしても dock では何も変わらない。
        //
        // 一方でノッチの有無は `ccsessions` からは測れない（objc2 に依存させない
        // 設計方針）。ノッチはカメラの位置上常に画面の水平中央に置かれるので、
        // ノッチ機であれば帯の幅に関わらず center 配置は必ずノッチの下に隠れる
        // （`ccsessionsd::geometry` の `center_hits_notch` が実測込みで固定している）。
        // 実行中の Mac がノッチ機かはここでは分からないので、bar × center を
        // 選んだ全ユーザに一律で知らせる（`ccsessionsd` 側は起動時に実測して警告する）。
        println!("                 ⚠ ノッチのある Mac では群れがノッチの下に隠れます。");
        println!("                   隠れる場合は \"auto\"（既定）にしてください。");
    }
    // プラグイン経由の配線。hook 本体は settings.json の `hooks` に現れないので
    // `MARKER` の走査では見つからない。`enabledPlugins` だけが手掛かりになる。
    let plugins: Vec<String> = known_settings_paths(&home_dir(), &cwd_or_dot())
        .into_iter()
        .filter(|p| p.exists())
        .flat_map(|path| {
            let root = read_or_empty(&path).unwrap_or_else(|_| serde_json::json!({}));
            enabled_ccsessions_plugins(&root)
        })
        .collect();
    for key in &plugins {
        println!("plugin:          {key} (hook はプラグインが配る)");
    }
    if installed.is_empty() && !plugins.is_empty() {
        // プラグインが配線を持っているので、これは正常な状態。
        // ここで「NOT installed」と言うと、入れた人に嘘をつくことになる。
        println!("hooks:           プラグイン経由（settings.json の hooks は使っていない）");
    } else if installed.is_empty() {
        println!("hooks:           NOT installed in any settings file it knows about");
        println!("                 Claude Code の中で次を実行する:");
        println!("                   /plugin marketplace add S-Nakamur-a/ccsessions");
        println!("                   /plugin install ccsessions@ccsessions-marketplace");
    } else {
        // どのファイルに入っているかまで出す。「入れたのとは別のファイルに
        // 残っている」に自力で気づけるのは、この一覧があるときだけ。
        for (path, events) in &installed {
            let names: Vec<&str> = events.iter().map(|(e, _)| e.as_str()).collect();
            println!("hooks:           {} ({})", path.display(), names.join(", "));
        }
        if !plugins.is_empty() {
            // **状態は壊れない** — reducer は agent_id で冪等なので、同じ payload が
            // 2 回来ても生き物もエージェント数も変わらない（`reduce_subagent_start`）。
            // 壊れないぶん気づきにくいので、無駄だと明示しておく。
            println!("                 ⚠ プラグインと settings.json の両方から配線されています。");
            println!(
                "                   表示は壊れませんが、イベントごとにプロセスが 2 つ起きます。"
            );
            println!("                   どちらか一方にすること（プラグインを使うなら、");
            println!("                   上のファイルから ccsessions のエントリを手で消す）");
        }
    }
    if missing_events.is_empty() {
        println!(
            "hook events:     all {} events installed",
            SIMPLE_EVENTS.len()
        );
    } else if !installed.is_empty() {
        // 手で書いた配線が欠けている場合。ccsessions はもう settings.json を
        // 書かないので、直すのは手か、プラグインへの乗り換え。
        println!("hook events:     missing {}", missing_events.join(", "));
        println!("                 手で足すか、プラグインに移ること");
    }
    for (path, _) in &installed {
        let root = read_or_empty(path).unwrap_or_else(|_| serde_json::json!({}));
        // `--record` の付けっぱなしは、ユーザの生プロンプトがディスクに溜まり続ける
        // ということ。開発用の一時設定なので、本番の設定に残っていたら知らせる。
        let recording = events_with_recording_enabled(&root);
        if !recording.is_empty() {
            println!(
                "recording:       {} で --record が有効のまま（{}）",
                recording.join(", "),
                path.display()
            );
            println!("                 payload にはユーザの生プロンプトが含まれる。");
            println!("                 開発用の一時設定なら外すこと");
        }
        // 旧バージョンは PreToolUse(matcher=Task) を仕込んでいた。今はサブエージェント
        // の追跡を SubagentStart に一本化したので reducer はこのイベントを無視する
        // ＝ ツール呼び出しごとにプロセスを起動して何もしない、純粋な無駄になる。
        if event_has_ccsessions_entry(&root, STALE_PRE_TOOL_USE_EVENT) {
            println!(
                "stale hook:      {STALE_PRE_TOOL_USE_EVENT} has a ccsessions entry but is no longer used"
            );
            println!("                 ({})", path.display());
            println!("                 ツール呼び出しごとにプロセスを 1 つ無駄にするだけなので、");
            println!("                 手で消すこと");
        }
    }
    for line in residency_lines(&installed_residencies(&home_dir()), running_daemons()) {
        println!("{line}");
    }
    println!("current_exe:     {}", current_exe_string());
    0
}

/// 走査の起点となるカレントディレクトリ。取れなければ `.`。
fn cwd_or_dot() -> PathBuf {
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

/// 常駐させる入口。**複数が仕込まれていると生き物が二重に出る**
/// （[ADR 0021](../../docs/adr/0021-distribution.md)）。どれも
/// `~/Library/LaunchAgents/<label>.plist` なので、ラベルからファイル名が決まる。
///
/// 旧名のラベルを載せてあるのは、`ccstatus` から改名したときに**旧 plist が
/// そのまま残る**ため。`brew services` と `make start` の衝突より、まず
/// この新旧の併走が起きる。
struct Residency {
    label: &'static str,
    /// この plist を書く主体。ユーザが「どっちを消すか」を決めるのに要る。
    origin: &'static str,
    /// 改名前のラベルか。**これだけが残っている状態は「正常」ではない** —
    /// 走っているのは旧バイナリで、新しい設定・状態・hook はどれも読まれない。
    legacy: bool,
}

const RESIDENCIES: &[Residency] = &[
    Residency {
        label: "dev.ccsessions.ccsessionsd",
        origin: "make start",
        legacy: false,
    },
    Residency {
        label: "homebrew.mxcl.ccsessions",
        origin: "brew services start",
        legacy: false,
    },
    Residency {
        label: "dev.ccstatus.ccstatusd",
        origin: "改名前の make start",
        legacy: true,
    },
];

/// 走っている常駐の数え方。改名の前後で実行ファイル名が変わるので両方数える。
const DAEMON_PROCESS_NAMES: &[&str] = &["ccsessionsd", "ccstatusd"];

/// `~/Library/LaunchAgents/` に plist が置かれている入口。
fn installed_residencies(home: &Path) -> Vec<&'static Residency> {
    let dir = home.join("Library/LaunchAgents");
    RESIDENCIES
        .iter()
        .filter(|r| dir.join(format!("{}.plist", r.label)).exists())
        .collect()
}

/// 実際に走っている常駐の数。**測れなければ `None`**（`0` にしない —
/// 「走っていない」と「数えられなかった」を混ぜると診断が嘘をつく）。
fn running_daemons() -> Option<usize> {
    let mut total = 0;
    for name in DAEMON_PROCESS_NAMES {
        // `pgrep -x` は完全一致。見つからないときの exit 1 は失敗ではないので、
        // status ではなく stdout の行数で数える。
        let out = std::process::Command::new("pgrep")
            .arg("-x")
            .arg(name)
            .output()
            .ok()?;
        total += String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter(|l| !l.trim().is_empty())
            .count();
    }
    Some(total)
}

/// 常駐の状況を診断行にする。**I/O を持たない**のは、テストが本物の
/// `~/Library/LaunchAgents` と実行中のプロセスに依存しないため。
fn residency_lines(installed: &[&'static Residency], running: Option<usize>) -> Vec<String> {
    let mut out = Vec::new();
    match installed {
        [] => out.push("daemon:          常駐の設定なし（群れは出ない）".to_string()),
        [only] => {
            out.push(format!("daemon:          {} ({})", only.label, only.origin));
            if only.legacy {
                // 二重起動より静かで、より紛らわしい状態。群れは出ているのに、
                // 新しい名前で書いた設定も hook も一切効かない。
                out.push(
                    "                 ⚠ 走っているのは改名前の常駐です。新しい設定・hook は"
                        .to_string(),
                );
                out.push(
                    "                   読まれません。`make stop` で止めてから `make start`"
                        .to_string(),
                );
            }
        }
        many => {
            // 二重起動。**どちらが正しいかは ccsessions には決められない**ので、
            // 消し方だけ示して選択はユーザに残す。
            out.push(format!(
                "daemon:          ⚠ 常駐の入口が {} 個ある — 生き物が二重に出ます",
                many.len()
            ));
            for r in many {
                out.push(format!("                   {} ({})", r.label, r.origin));
            }
            out.push(
                "                 1 つだけ残すこと。`make stop` / `brew services stop ccsessions`"
                    .to_string(),
            );
            out.push("                 で止めてから、要らない方の plist を消す".to_string());
        }
    }
    match running {
        // plist が 1 つでも、`make dev` と併走していれば 2 つ走る。
        Some(n) if n > 1 => out.push(format!(
            "                 ⚠ 常駐が {n} 個走っています（`make dev` の走らせっぱなしも含む）"
        )),
        Some(0) if !installed.is_empty() => out.push(
            "                 ⚠ 仕込まれているが走っていません（`make start` で開始）".to_string(),
        ),
        _ => {}
    }
    out
}

/// 旧バージョンが購読していたイベント。今は使わない（`SIMPLE_EVENTS` の
/// doc コメント参照）。
const STALE_PRE_TOOL_USE_EVENT: &str = "PreToolUse";

fn exists_label(exists: bool) -> &'static str {
    if exists {
        "exists"
    } else {
        "absent"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn residency(label: &str) -> &'static Residency {
        RESIDENCIES
            .iter()
            .find(|r| r.label == label)
            .expect("テストが存在しないラベルを指している")
    }

    fn joined(installed: &[&'static Residency], running: Option<usize>) -> String {
        residency_lines(installed, running).join("\n")
    }

    #[test]
    fn a_single_running_residency_is_reported_without_a_warning() {
        let out = joined(&[residency("dev.ccsessions.ccsessionsd")], Some(1));
        assert!(out.contains("dev.ccsessions.ccsessionsd"));
        assert!(
            !out.contains('⚠'),
            "正常な状態で警告を出してはいけない: {out}"
        );
    }

    #[test]
    fn two_residencies_warn_that_the_flock_would_be_doubled() {
        let out = joined(
            &[
                residency("dev.ccsessions.ccsessionsd"),
                residency("homebrew.mxcl.ccsessions"),
            ],
            Some(2),
        );
        assert!(out.contains("二重に出ます"));
        // どちらを消すかはユーザが決めるので、両方の出所が見えている必要がある。
        assert!(out.contains("make start"));
        assert!(out.contains("brew services start"));
    }

    #[test]
    fn the_launch_agent_left_by_the_old_name_is_detected_as_a_second_residency() {
        // 改名直後に必ず起きる状態。旧 plist を検出できないと、
        // 「二重に出るのに doctor は正常と言う」になる。
        let out = joined(
            &[
                residency("dev.ccsessions.ccsessionsd"),
                residency("dev.ccstatus.ccstatusd"),
            ],
            Some(2),
        );
        assert!(out.contains("二重に出ます"));
        assert!(out.contains("dev.ccstatus.ccstatusd"));
    }

    #[test]
    fn a_lone_leftover_from_the_old_name_is_not_reported_as_healthy() {
        // 改名直後の既定の状態。1 個しか無いので二重起動ではないが、
        // 走っているのは旧バイナリで新しい設定も hook も効かない。
        let out = joined(&[residency("dev.ccstatus.ccstatusd")], Some(1));
        assert!(
            out.contains('⚠'),
            "旧常駐だけの状態を正常と言ってはいけない: {out}"
        );
        assert!(out.contains("改名前の常駐"));
    }

    #[test]
    fn no_residency_says_the_flock_will_not_appear() {
        let out = joined(&[], Some(0));
        assert!(out.contains("常駐の設定なし"));
        // plist が無いなら「走っていない」は当たり前で、警告にはしない。
        assert!(!out.contains('⚠'), "{out}");
    }

    #[test]
    fn an_installed_but_stopped_daemon_is_reported() {
        let out = joined(&[residency("dev.ccsessions.ccsessionsd")], Some(0));
        assert!(out.contains("走っていません"));
    }

    #[test]
    fn an_unmeasurable_process_count_never_claims_the_daemon_is_stopped() {
        // `pgrep` が無い・呼べない環境で「走っていません」と出すと、
        // 動いている常駐を止めさせる嘘の診断になる。
        let out = joined(&[residency("dev.ccsessions.ccsessionsd")], None);
        assert!(!out.contains("走っていません"), "{out}");
        assert!(!out.contains('⚠'), "{out}");
    }

    #[test]
    fn more_than_one_running_daemon_warns_even_with_a_single_plist() {
        // `make dev` の走らせっぱなしは plist を増やさないが、群れは二重に出る。
        let out = joined(&[residency("dev.ccsessions.ccsessionsd")], Some(2));
        assert!(out.contains("2 個走っています"));
    }
}
