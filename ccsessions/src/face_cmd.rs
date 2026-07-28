//! `ccsessions face [list|check|render|gallery]` — 顔を作る人のための道具。
//!
//! **画面収録権限が無い環境ではスクリーンショットが撮れない**ので、
//! 顔の見た目を確かめる手段は SVG プレビューになる。
//! 同時にこれは「顔を足す PR に SVG を貼る」というコントリビュータ体験そのもの。

use ccsessions_core::face::{svg, validate, FaceSpec, Registry, Size, Source};
use ccsessions_core::session::SessionState;

pub fn run(args: &[String]) -> i32 {
    match args.first().map(String::as_str) {
        None | Some("list") => cmd_list(),
        Some("check") => cmd_check(&args[1..]),
        Some("render") => cmd_render(&args[1..]),
        Some("gallery") => cmd_gallery(),
        Some(other) => {
            eprintln!("ccsessions: face: unknown subcommand: {other}");
            eprintln!("usage: ccsessions face [list|check <id|path>|render <id>|gallery]");
            1
        }
    }
}

fn registry() -> Registry {
    Registry::load_in(&ccsessions_core::faces_dir())
}

// ---------------------------------------------------------------------------
// list
// ---------------------------------------------------------------------------

fn cmd_list() -> i32 {
    let reg = registry();
    println!("{:<12} {:<16} {:<10} 作者", "ID", "名前", "出どころ");
    for f in reg.all() {
        let src = match &f.source {
            Source::Builtin => "組込み".to_string(),
            Source::User(p) => p
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "ユーザ".to_string()),
        };
        println!(
            "{:<12} {:<16} {:<10} {}",
            f.id,
            f.label,
            src,
            f.author.as_deref().unwrap_or("-")
        );
    }
    // 読めなかった顔は最後に列挙する（一覧そのものは壊さない）。
    if !reg.problems().is_empty() {
        eprintln!();
        reg.report_problems();
    }
    0
}

// ---------------------------------------------------------------------------
// check
// ---------------------------------------------------------------------------

/// `ccsessions face check [<id> | <path.toml>]`。引数なしで全部。
///
/// exit 1 になるのは**検証エラーがあったときだけ**。`notch-width` の警告は exit 0 のまま
/// （システムが正しく扱えるものを弾かない）。
fn cmd_check(args: &[String]) -> i32 {
    let reg = registry();
    let mut failed = false;

    // ファイルパスを直接渡された場合は、レジストリに入っていなくても検査する。
    if let Some(arg) = args.first() {
        let p = std::path::Path::new(arg);
        if p.is_file() {
            return match std::fs::read_to_string(p) {
                Ok(text) => {
                    match ccsessions_core::face::parse::parse(&text, Source::User(p.to_path_buf()))
                    {
                        Ok(face) => {
                            if report_one(&face, &format!("{}", p.display())) {
                                0
                            } else {
                                1
                            }
                        }
                        Err(ps) => {
                            println!("NG: {}", p.display());
                            for x in ps {
                                println!("  {x}");
                            }
                            1
                        }
                    }
                }
                Err(e) => {
                    eprintln!("ccsessions: face: {} を読めません: {e}", p.display());
                    1
                }
            };
        }
        // パスでなければ id として引く。
        let Some(face) = reg.get(arg) else {
            eprintln!("ccsessions: face: {arg:?} という顔もファイルもありません");
            eprintln!("使える顔: {}", reg.ids().join(", "));
            return 1;
        };
        return if report_one(face, &face.id.clone()) {
            0
        } else {
            1
        };
    }

    for face in reg.all() {
        if !report_one(face, &face.id.clone()) {
            failed = true;
        }
    }
    // 読み込めなかったファイルも失敗として扱う。
    if !reg.problems().is_empty() {
        reg.report_problems();
        failed = true;
    }
    i32::from(failed)
}

/// 顔 1 つを検証して結果を出す。戻り値は「通ったか」。
fn report_one(face: &FaceSpec, label: &str) -> bool {
    let result = validate::validate(face);
    let warning = validate::notch_width_warning(face);

    match &result {
        Ok(()) => println!("OK: {label}"),
        Err(ps) => {
            println!("NG: {label}");
            for p in ps {
                println!("  {p}");
            }
        }
    }
    if let Some(w) = warning {
        println!("  warning{w}");
    }
    result.is_ok()
}

// ---------------------------------------------------------------------------
// render
// ---------------------------------------------------------------------------

/// `ccsessions face render <id> [--state <s>] [--size <bar|dock>]` → SVG を stdout。
fn cmd_render(args: &[String]) -> i32 {
    let Some(id) = args.first() else {
        eprintln!("usage: ccsessions face render <id> [--state <state>] [--size <bar|dock>]");
        return 1;
    };
    let mut state = SessionState::Working;
    let mut size = Size::Bar;

    let mut i = 1;
    while i < args.len() {
        let need = |i: usize| -> Option<&String> { args.get(i + 1) };
        match args[i].as_str() {
            "--state" => {
                let Some(v) = need(i) else {
                    eprintln!("ccsessions: face: --state に値がありません");
                    return 1;
                };
                let Some(s) = SessionState::from_str(v) else {
                    eprintln!(
                        "ccsessions: face: 未知の状態 {v:?}（working, wait_user, wait_agent, idle, done, error）"
                    );
                    return 1;
                };
                state = s;
                i += 2;
            }
            "--size" => {
                let Some(v) = need(i) else {
                    eprintln!("ccsessions: face: --size に値がありません");
                    return 1;
                };
                size = match v.as_str() {
                    "bar" => Size::Bar,
                    "dock" => Size::Dock,
                    other => {
                        eprintln!("ccsessions: face: 未知の配置 {other:?}（bar か dock）");
                        return 1;
                    }
                };
                i += 2;
            }
            other => {
                eprintln!("ccsessions: face: 未知の引数 {other:?}");
                return 1;
            }
        }
    }

    let reg = registry();
    let Some(face) = reg.get(id) else {
        eprintln!("ccsessions: face: {id:?} という顔がありません");
        eprintln!("使える顔: {}", reg.ids().join(", "));
        return 1;
    };
    print!("{}", svg::render(face, state, size));
    0
}

// ---------------------------------------------------------------------------
// gallery
// ---------------------------------------------------------------------------

/// `ccsessions face gallery` → 全顔 × 全状態 × bar/dock の HTML を stdout。
fn cmd_gallery() -> i32 {
    let reg = registry();
    print!("{}", svg::gallery(reg.all()));
    0
}
