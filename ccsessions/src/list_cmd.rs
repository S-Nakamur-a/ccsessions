//! `ccsessions list [--json]` — 生きているセッションを一覧表示する。

use ccsessions_core::{config, now_ms, store};

pub fn run(args: &[String]) -> i32 {
    let json = args.iter().any(|a| a == "--json");

    let cfg = config::load(&ccsessions_core::config_path()).unwrap_or_else(|e| {
        eprintln!("ccsessions: list: config load error, using defaults: {e}");
        config::builtin_default()
    });
    let now = now_ms();
    let sessions = store::list_live(now, cfg.session_ttl_ms(), cfg.max_sessions);

    if json {
        match serde_json::to_string_pretty(&sessions) {
            Ok(s) => println!("{s}"),
            Err(e) => {
                eprintln!("ccsessions: list: failed to serialize sessions: {e}");
                return 1;
            }
        }
        return 0;
    }

    if sessions.is_empty() {
        println!("(no live sessions)");
        return 0;
    }
    for s in &sessions {
        let disp = s.display_state(now, cfg.done_ttl_ms());
        let elapsed = ccsessions_core::session::Session::fmt_dur(now.saturating_sub(s.since));
        println!(
            "{glyph} {label:<10} {name:<20} {elapsed:>6}  agents={agents}",
            glyph = disp.glyph(),
            label = disp.ja(),
            name = s.name,
            elapsed = elapsed,
            agents = s.agents.len(),
        );
    }
    0
}
