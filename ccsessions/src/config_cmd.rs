//! `ccsessions config [get|set <key> <value>|path|edit]`
//!
//! 設定をいじる主な入口は **Web UI（`ccsessions ui`）** で、ここはその CLI 版。
//! キーの一覧・検証・エラーメッセージは `config::set_field` に置いてあり、
//! 画面とここで同じものを使う（`ccsessions-core/src/config.rs` のスキーマ節）。

use ccsessions_core::config::{self, Config};
use ccsessions_core::face::Registry;

pub fn run(args: &[String]) -> i32 {
    match args.first().map(String::as_str) {
        None | Some("get") => cmd_get(),
        Some("set") => cmd_set(&args[1..]),
        Some("path") => cmd_path(),
        Some("edit") => cmd_edit(),
        Some(other) => {
            eprintln!("ccsessions: config: unknown subcommand: {other}");
            eprintln!("usage: ccsessions config [get|set <key> <value>|path|edit]");
            eprintln!("（画面から設定するなら `ccsessions ui`）");
            1
        }
    }
}

fn load_or_default() -> Config {
    config::load(&ccsessions_core::config_path()).unwrap_or_else(|e| {
        eprintln!("ccsessions: config: load error, showing built-in defaults: {e}");
        config::builtin_default()
    })
}

fn cmd_get() -> i32 {
    print!("{}", config::render_toml(&load_or_default()));
    0
}

fn cmd_path() -> i32 {
    println!("{}", ccsessions_core::config_path().display());
    0
}

fn cmd_edit() -> i32 {
    let path = ccsessions_core::config_path();
    if !path.exists() {
        // 初回編集時は組込みデフォルトを実体化してからエディタを開く
        // （空ファイルより、選択肢コメント入りのテンプレートの方が編集しやすい）。
        if let Err(e) = config::save(&path, &config::builtin_default()) {
            eprintln!(
                "ccsessions: config: failed to create default config at {}: {}",
                path.display(),
                e
            );
            return 1;
        }
    }
    let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vi".to_string());
    match std::process::Command::new(&editor).arg(&path).status() {
        Ok(status) if status.success() => 0,
        Ok(status) => {
            eprintln!("ccsessions: config: {editor} exited with {status}");
            1
        }
        Err(e) => {
            eprintln!("ccsessions: config: failed to launch editor {editor:?}: {e}");
            1
        }
    }
}

fn cmd_set(args: &[String]) -> i32 {
    if args.len() != 2 {
        eprintln!("usage: ccsessions config set <key> <value>");
        eprintln!(
            "keys: {}",
            config::fields()
                .iter()
                .map(|f| f.key)
                .collect::<Vec<_>>()
                .join(", ")
        );
        return 1;
    }
    let (key, value) = (args[0].as_str(), args[1].as_str());

    let mut cfg = load_or_default();
    let faces = Registry::load_in(&ccsessions_core::faces_dir());
    if let Err(e) = config::set_field(&mut cfg, key, value, &faces) {
        eprintln!("ccsessions: config: {e}");
        return 1;
    }
    if let Err(e) = config::save(&ccsessions_core::config_path(), &cfg) {
        eprintln!("ccsessions: config: failed to save config: {e}");
        return 1;
    }
    0
}
