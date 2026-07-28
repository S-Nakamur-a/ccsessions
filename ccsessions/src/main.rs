//! ccsessions CLI — hook producer 兼、セッション一覧・設定コマンド。
//!
//! clap 等の依存を増やさず `std::env::args()` を手でパースする（サブコマンドの数も少なく、依存を足すほどの複雑さがないため）。
//!
//! **hook の配線コマンドはここには無い。** `settings.json` に書くのは Claude Code
//! プラグイン（`plugins/ccsessions/`）の仕事で、この CLI は他人の設定ファイルを
//! 一切書き換えない（[ADR 0021](../../docs/adr/0021-distribution.md)）。読むのは
//! `doctor` の診断のためだけ。

mod config_cmd;
mod doctor;
mod face_cmd;
mod hook;
mod list_cmd;
mod set_cmd;
mod settings_json;
mod ui_cmd;

use std::env;

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    std::process::exit(run(&args));
}

fn run(args: &[String]) -> i32 {
    match args.first().map(String::as_str) {
        None | Some("help") | Some("-h") | Some("--help") => {
            print_help();
            0
        }
        Some("hook") => hook::run(&args[1..]),
        Some("list") => list_cmd::run(&args[1..]),
        Some("set") => set_cmd::run(&args[1..]),
        Some("config") => config_cmd::run(&args[1..]),
        Some("face") => face_cmd::run(&args[1..]),
        Some("ui") => ui_cmd::run(&args[1..]),
        Some("doctor") => doctor::run(),
        Some(other) => {
            eprintln!("ccsessions: unknown subcommand: {other}");
            print_help();
            1
        }
    }
}

fn print_help() {
    println!(
        r#"ccsessions — Session Creatures の hook producer / CLI

USAGE:
    ccsessions hook [--event <name>] [--record <dir>]
        stdin から Claude Code の hook payload (JSON) を読み、セッション状態を更新する。
        必ず exit 0 で終わり、stdout には何も書かない。
        --record <dir> を渡すと、受け取った payload をパース成否に関わらず
        <dir> にそのまま収録する（開発用。0700/0600 で作成）。

    ccsessions list [--json]
        生きているセッションを一覧表示する。

    ccsessions set --session <id> --state <state> [--cwd <cwd>]
        セッション状態を直接書き換える（デバッグ／外部 producer 用）。
        <state> は working, wait_user, wait_agent, idle, done, error のいずれか。

    ccsessions ui [--port <n>] [--faces-dir <path>] [--config <path>] [--no-open]
        設定とキャラクタービルダーの Web UI を 127.0.0.1 に立てる（`make config`）。
        既定のポートは 8787。設定は保存した瞬間に走っている ccsessionsd が拾い、
        顔は ~/.config/ccsessions/faces/<id>.toml として保存される。

    ccsessions config [get|set <key> <value>|path|edit]
        設定の表示・変更（画面から設定するなら ccsessions ui）。
        既定は get（現在の設定を TOML で表示）。

    ccsessions face [list|check <id|path>|render <id>|gallery]
        顔（生き物のデザイン）を一覧・検証・プレビューする。
        render は SVG を、gallery は全顔 × 全状態の HTML を stdout に書く。
        顔は faces/*.toml と ~/.config/ccsessions/faces/*.toml から読む。

    ccsessions doctor
        設定・状態ディレクトリ・hook 導入状況を診断表示する。

    ccsessions help
        このメッセージを表示する。

HOOK の入れ方:
    Claude Code の中で次を実行する。ccsessions は settings.json を書き換えない
    （配線するのはプラグインで、入るのは enabledPlugins の 1 行だけ）。

        /plugin marketplace add S-Nakamur-a/ccsessions
        /plugin install ccsessions@ccsessions-marketplace
"#
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_subcommand_returns_1() {
        assert_eq!(run(&["bogus".to_string()]), 1);
    }

    #[test]
    fn no_args_prints_help_and_returns_0() {
        assert_eq!(run(&[]), 0);
    }
}
