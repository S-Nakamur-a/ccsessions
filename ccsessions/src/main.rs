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
        r#"ccsessions — the hook producer / CLI behind the session creatures

USAGE:
    ccsessions hook [--event <name>] [--record <dir>]
        Read a Claude Code hook payload (JSON) from stdin and update the session
        state. Always exits 0 and writes nothing to stdout.
        With --record <dir>, every payload received is recorded verbatim into
        <dir> whether or not it parses (for development; created 0700/0600).

    ccsessions list [--json]
        List the live sessions.

    ccsessions set --session <id> --state <state> [--cwd <cwd>]
        Overwrite a session's state directly (for debugging / external producers).
        <state> is one of working, wait_user, wait_agent, idle, done, error.

    ccsessions ui [--port <n>] [--faces-dir <path>] [--config <path>] [--no-open]
        Serve the settings and character-builder web UI on 127.0.0.1 (`make config`).
        Port 8787 by default. A running ccsessionsd picks settings up the moment
        they are saved; faces are written to ~/.config/ccsessions/faces/<id>.toml.

    ccsessions config [get|set <key> <value>|path|edit]
        Show or change the settings (use `ccsessions ui` for a screen).
        Defaults to get, which prints the current settings as TOML.

    ccsessions face [list|check <id|path>|render <id>|gallery]
        List, validate, and preview faces (the creature designs).
        render writes an SVG to stdout; gallery writes HTML of every face in
        every state. Faces are read from faces/*.toml and
        ~/.config/ccsessions/faces/*.toml.

    ccsessions doctor
        Diagnose the settings, the state directory, and how the hooks are wired.

    ccsessions help
        Show this message.

INSTALLING THE HOOKS:
    Run this inside Claude Code. ccsessions never edits settings.json itself —
    the plugin does the wiring, and all that lands there is one enabledPlugins line.

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
