//! `ccsessions list [--json] [--all]` — 生きているセッションを一覧表示する。

use ccsessions_core::ignore::IgnoreRules;
use ccsessions_core::{config, lang, now_ms, store};

pub fn run(args: &[String]) -> i32 {
    let json = args.iter().any(|a| a == "--json");
    // `--all` は「表示を絞る条件を全部外す」意図なので、ignore だけでなく
    // `max_sessions` の打ち切りも外す。max を残すと、生きているセッションが枠を
    // 超えたときに ignore 対象が枠を食って、`--all` を付けたほうが素の一覧より
    // 表示が減ることがある。`--json` にも効くが、JSON の形は配列のまま変えない。
    let all = args.iter().any(|a| a == "--all");

    let cfg = config::load(&ccsessions_core::config_path()).unwrap_or_else(|e| {
        eprintln!("ccsessions: list: config load error, using defaults: {e}");
        config::builtin_default()
    });
    let now = now_ms();
    let ignore = if all {
        IgnoreRules::default()
    } else {
        cfg.ignore.clone()
    };
    let max = if all { usize::MAX } else { cfg.max_sessions };
    let live = store::list_live(now, cfg.session_ttl_ms(), max, &ignore);
    let sessions = live.shown;

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
        // 「セッションが 0 件」と「全部 ignore で畳まれた」は別の状態なので、
        // 全滅していても隠れているぶんはここで知らせる。
        print_hidden_count(all, live.ignored);
        return 0;
    }
    // 状態名だけは設定の言語に従う（ホバーカードと同じ語が出ないと混乱するため）。
    // このコマンドの他の出力とエラーは英語で固定。
    let lang = cfg.language.resolve(lang::env_tag().as_deref());
    for s in &sessions {
        let disp = s.display_state(now, cfg.done_ttl_ms());
        let elapsed = ccsessions_core::session::Session::fmt_dur(now.saturating_sub(s.since));
        // 桁は一番長い英語ラベル（"Agents running" = 14）に合わせる。日本語だけを
        // 見て 10 にしていると、英語で名前の列が押し出される。
        println!(
            "{glyph} {label:<14} {name:<20} {elapsed:>6}  agents={agents}",
            glyph = disp.glyph(),
            label = disp.label(lang),
            name = s.name,
            elapsed = elapsed,
            agents = s.agents.len(),
        );
    }
    print_hidden_count(all, live.ignored);
    0
}

/// ignore で外した件数の 1 行。0 件のときは何も出さない（`--all` なら `ignored`
/// は常に 0 なので、`all` の判定は念のため）。
fn print_hidden_count(all: bool, ignored: usize) {
    if !all && ignored > 0 {
        println!("({ignored} hidden by ignore; pass --all to show them)");
    }
}
