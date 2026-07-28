//! transcript（Claude Code の JSONL）から拾える補助情報。
//!
//! - 直近ターンのエラー判定（`last_turn_errored`）— `Stop` hook のときだけ呼ばれる
//! - セッションタイトル（`session_title`）— hook payload に無い唯一の入手経路
//!
//! どちらも全文を読むとコストがかさむため、**末尾だけを見る**。
//!
//! **これは補助手段であって第一手段ではない。** API エラーで終わったターンは
//! `Stop` ではなく `StopFailure` を出すので、`Stop` を起点にするこの経路は
//! 本来狙っていた場面では発火しない。エラー種別つきの検出は `event::reduce` の
//! `StopFailure` 分岐が行う。ここが拾えるのは「`Stop` は来たが直近の assistant 行が
//! エラー」という場合だけで、それがどれだけ実在するかは未検証。そのため
//! `detect_errors` の既定は `false`。
//!
//! タイトル側も**未文書の内部フォーマットへの依存**で、公式の hook スキーマには
//! 出てこない（[ADR 0023](../../docs/adr/0023-session-title-from-transcript.md)）。
//! 形式が変わったときは「見つからない ＝ 出さない」に倒れるだけで、状態表示は
//! 一切壊れない。

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use serde::Deserialize;

/// tail として読む最大バイト数。
const TAIL_BYTES: u64 = 65536;

/// タイトル行を探すときに読む最大バイト数。
///
/// タイトル行は「そのとき書かれた 1 回きり」ではなく、セッションのメタ情報
/// ブロックとして**繰り返し追記される**。実測（Claude Code 2.1.220、2.4MB の
/// transcript）では追記の間隔は 30〜91KB、最後の 1 件は EOF から 8KB だった。
/// エラー判定側の 64KB では実測の最大間隔（91KB）を跨げないので、その 2 倍強を
/// 取る。届かなければタイトルが出ないだけなので、安全側は「広め」。
const TITLE_TAIL_BYTES: u64 = 262_144;

/// タイトルとして保存する最大文字数。
///
/// 表示側の省略は `ccsessionsd` の責務だが、**ストアに際限なく長い文字列を
/// 書かない**ための歯止めをここに置く（transcript は外部が書くファイルであり、
/// 長さを信用できない）。
const MAX_TITLE_CHARS: usize = 120;

/// JSONL の 1 行から `type` フィールドだけを取り出す軽量パース用。
/// 他のフィールドは無視する（`assistant` かどうかの判定にしか使わないため）。
#[derive(Deserialize)]
struct Kind {
    r#type: Option<String>,
}

/// transcript の末尾から、直近のターンがエラーで終わったかを判定する。
/// ファイルが無い・読めない・判定できない場合は false（エラー扱いしない ＝ fail-safe）。
///
/// 実装: ファイル末尾 `TAIL_BYTES` を読み、行に分割して**最後の完全な行から
/// 遡って**最初に見つかった `"type":"assistant"` エントリを見る。判定は
/// `serde_json` で `type` だけ軽く取り出して assistant 行を特定したうえで、
/// その行に `isApiErrorMessage:true` / `is_error:true` という部分文字列一致
/// があるかで見る（行全体を厳密にパースする構造体を用意するより、フィールド
/// の有無だけ見れば十分なうえ、将来のスキーマ変更にも強い）。
pub fn last_turn_errored(path: &Path) -> bool {
    let text = match tail_text(path, TAIL_BYTES) {
        Some(t) => t,
        None => return false,
    };

    for line in text.lines().rev() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let is_assistant = serde_json::from_str::<Kind>(line)
            .ok()
            .map(|k| k.r#type.as_deref() == Some("assistant"))
            .unwrap_or(false);
        if !is_assistant {
            continue;
        }
        return line.contains("\"isApiErrorMessage\":true") || line.contains("\"is_error\":true");
    }
    false
}

// ---------------------------------------------------------------------------
// セッションタイトル
// ---------------------------------------------------------------------------

/// タイトル行の軽量パース用。`custom-title` / `ai-title` のどちらかで、
/// **どちらも `sessionId` を持つ**（resume / fork で別セッションの行が同じ
/// ファイルに混ざりうるので、この照合が要る）。
#[derive(Deserialize)]
struct TitleLine {
    r#type: Option<String>,
    #[serde(rename = "sessionId")]
    session_id: Option<String>,
    #[serde(rename = "customTitle")]
    custom_title: Option<String>,
    #[serde(rename = "aiTitle")]
    ai_title: Option<String>,
}

/// transcript の末尾から、そのセッションのタイトルを探す。無ければ `None`。
///
/// Claude Code はタイトルを hook payload では渡さず、transcript へ独立した行として
/// 追記する（実測 · 2.1.220）:
///
/// ```text
/// {"type":"ai-title","aiTitle":"…","sessionId":"…"}
/// {"type":"custom-title","customTitle":"…","sessionId":"…"}
/// ```
///
/// 優先順位は本体の解決順（`customTitle || aiTitle`）に合わせる。**古い
/// `custom-title` は新しい `ai-title` より強い** — ユーザが明示的に付けた名前を
/// 自動生成が上書きしてはいけないため、末尾から先に見つかった方ではなく種別で決める。
///
/// 見つからない・読めない・空文字はすべて `None`（＝表示しない）に倒す。
pub fn session_title(path: &Path, session_id: &str) -> Option<String> {
    let text = tail_text(path, TITLE_TAIL_BYTES)?;
    let mut ai: Option<String> = None;

    for line in text.lines().rev() {
        // 大半の行はタイトル行ではないので、serde に渡す前に安く弾く。
        if !line.contains("-title\"") {
            continue;
        }
        let parsed = match serde_json::from_str::<TitleLine>(line) {
            Ok(t) => t,
            Err(_) => continue,
        };
        if parsed.session_id.as_deref() != Some(session_id) {
            continue;
        }
        match parsed.r#type.as_deref() {
            Some("custom-title") => {
                if let Some(t) = clean_title(parsed.custom_title) {
                    return Some(t);
                }
            }
            Some("ai-title") if ai.is_none() => {
                ai = clean_title(parsed.ai_title);
            }
            _ => {}
        }
    }
    ai
}

/// タイトル文字列を表示に耐える形へ均す。空になったら `None`。
///
/// 改行やタブがそのまま入ると 1 行のテキストレイヤで表示が崩れるので、空白の
/// 並びは単一のスペースへ潰す。長さは `MAX_TITLE_CHARS` で切る（バイトではなく
/// 文字数 — 日本語のタイトルが普通にある）。
fn clean_title(raw: Option<String>) -> Option<String> {
    let raw = raw?;
    let normalized = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.is_empty() {
        return None;
    }
    Some(normalized.chars().take(MAX_TITLE_CHARS).collect())
}

// ---------------------------------------------------------------------------
// 共通の tail 読み
// ---------------------------------------------------------------------------

/// ファイル末尾 `max_bytes` を読み、**完全な行だけ**からなるテキストを返す。
/// 開けない・読めない・空ならすべて `None`。
///
/// 途中から読み始めた場合、シーク位置がちょうど改行の直後とは限らないので、
/// 先頭行を「不完全かもしれない行」として捨てる。tail が UTF-8 のマルチバイト
/// 文字境界で切れている可能性があるので lossy で読む。
fn tail_text(path: &Path, max_bytes: u64) -> Option<String> {
    let mut file = File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    if len == 0 {
        return None;
    }

    let read_from_start = len <= max_bytes;
    let seek_pos = if read_from_start { 0 } else { len - max_bytes };
    file.seek(SeekFrom::Start(seek_pos)).ok()?;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf).ok()?;

    let text = String::from_utf8_lossy(&buf);
    if read_from_start {
        return Some(text.into_owned());
    }
    // 先頭の 1 行（切れているかもしれない行）を落とす。
    text.find('\n').map(|i| text[i + 1..].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn write_fixture(dir: &TempDir, name: &str, content: &str) -> std::path::PathBuf {
        let p = dir.path().join(name);
        fs::write(&p, content).unwrap();
        p
    }

    #[test]
    fn normal_completion_returns_false() {
        let dir = TempDir::new().unwrap();
        let content = "{\"type\":\"user\",\"message\":\"hi\"}\n\
             {\"type\":\"assistant\",\"message\":{\"content\":\"done\"}}\n";
        let p = write_fixture(&dir, "t.jsonl", content);
        assert!(!last_turn_errored(&p));
    }

    #[test]
    fn api_error_message_returns_true() {
        let dir = TempDir::new().unwrap();
        let content = "{\"type\":\"user\"}\n\
             {\"type\":\"assistant\",\"isApiErrorMessage\":true}\n";
        let p = write_fixture(&dir, "t.jsonl", content);
        assert!(last_turn_errored(&p));
    }

    #[test]
    fn is_error_field_returns_true() {
        let dir = TempDir::new().unwrap();
        let content = "{\"type\":\"assistant\",\"is_error\":true}\n";
        let p = write_fixture(&dir, "t.jsonl", content);
        assert!(last_turn_errored(&p));
    }

    #[test]
    fn empty_file_returns_false() {
        let dir = TempDir::new().unwrap();
        let p = write_fixture(&dir, "t.jsonl", "");
        assert!(!last_turn_errored(&p));
    }

    #[test]
    fn missing_file_returns_false() {
        let dir = TempDir::new().unwrap();
        let p = dir.path().join("does-not-exist.jsonl");
        assert!(!last_turn_errored(&p));
    }

    #[test]
    fn skips_non_assistant_trailing_lines() {
        // 最後の行が assistant でなくても、遡って直近の assistant エントリを見る。
        let dir = TempDir::new().unwrap();
        let content = "{\"type\":\"assistant\",\"isApiErrorMessage\":true}\n\
             {\"type\":\"user\",\"message\":\"ok, retrying\"}\n";
        let p = write_fixture(&dir, "t.jsonl", content);
        assert!(last_turn_errored(&p));
    }

    #[test]
    fn large_file_reads_tail_only() {
        // 64KB を大きく超えるファイルでも末尾のエントリだけで判定できること。
        let dir = TempDir::new().unwrap();
        let mut content = String::new();
        for i in 0..3000 {
            content.push_str(&format!(
                "{{\"type\":\"user\",\"message\":\"filler line number {i} padding padding\"}}\n"
            ));
        }
        assert!(
            content.len() as u64 > TAIL_BYTES,
            "fixture must exceed TAIL_BYTES to exercise the tail-read path"
        );
        content.push_str("{\"type\":\"assistant\",\"isApiErrorMessage\":true}\n");
        let p = write_fixture(&dir, "t.jsonl", &content);
        assert!(
            last_turn_errored(&p),
            "must detect error in tail of large file"
        );
    }

    #[test]
    fn large_file_normal_completion_returns_false() {
        let dir = TempDir::new().unwrap();
        let mut content = String::new();
        for i in 0..3000 {
            content.push_str(&format!(
                "{{\"type\":\"user\",\"message\":\"filler line number {i} padding padding\"}}\n"
            ));
        }
        content.push_str("{\"type\":\"assistant\",\"message\":\"all good\"}\n");
        let p = write_fixture(&dir, "t.jsonl", &content);
        assert!(!last_turn_errored(&p));
    }

    // ---- session_title -------------------------------------------------------

    /// 実物の 1 行（Claude Code 2.1.220 が書いたもの）をそのまま写した形。
    fn ai_title_line(title: &str, sid: &str) -> String {
        format!("{{\"type\":\"ai-title\",\"aiTitle\":\"{title}\",\"sessionId\":\"{sid}\"}}\n")
    }

    fn custom_title_line(title: &str, sid: &str) -> String {
        format!(
            "{{\"type\":\"custom-title\",\"customTitle\":\"{title}\",\"sessionId\":\"{sid}\"}}\n"
        )
    }

    #[test]
    fn a_transcript_without_any_title_line_has_no_title() {
        let dir = TempDir::new().unwrap();
        let p = write_fixture(&dir, "t.jsonl", "{\"type\":\"user\",\"message\":\"hi\"}\n");
        assert_eq!(session_title(&p, "s1"), None);
    }

    #[test]
    fn a_missing_transcript_has_no_title() {
        let dir = TempDir::new().unwrap();
        let p = dir.path().join("does-not-exist.jsonl");
        assert_eq!(session_title(&p, "s1"), None);
    }

    #[test]
    fn the_ai_title_is_read_from_the_transcript() {
        let dir = TempDir::new().unwrap();
        let content = format!(
            "{{\"type\":\"user\"}}\n{}",
            ai_title_line("同一ディレクトリでの複数セッションの区別", "s1")
        );
        let p = write_fixture(&dir, "t.jsonl", &content);
        assert_eq!(
            session_title(&p, "s1").as_deref(),
            Some("同一ディレクトリでの複数セッションの区別")
        );
    }

    #[test]
    fn the_latest_ai_title_wins_over_earlier_ones() {
        let dir = TempDir::new().unwrap();
        let content = format!(
            "{}{}",
            ai_title_line("old subject", "s1"),
            ai_title_line("new subject", "s1")
        );
        let p = write_fixture(&dir, "t.jsonl", &content);
        assert_eq!(session_title(&p, "s1").as_deref(), Some("new subject"));
    }

    /// ユーザが付けた名前を、あとから書かれた自動生成が上書きしてはいけない。
    #[test]
    fn a_user_set_title_outranks_a_newer_generated_one() {
        let dir = TempDir::new().unwrap();
        let content = format!(
            "{}{}",
            custom_title_line("my name for it", "s1"),
            ai_title_line("generated later", "s1")
        );
        let p = write_fixture(&dir, "t.jsonl", &content);
        assert_eq!(session_title(&p, "s1").as_deref(), Some("my name for it"));
    }

    /// resume / fork では別セッションの行が同じファイルに混ざる。
    #[test]
    fn a_title_belonging_to_another_session_is_ignored() {
        let dir = TempDir::new().unwrap();
        let content = format!(
            "{}{}",
            ai_title_line("mine", "s1"),
            ai_title_line("someone elses", "s2")
        );
        let p = write_fixture(&dir, "t.jsonl", &content);
        assert_eq!(session_title(&p, "s1").as_deref(), Some("mine"));
    }

    #[test]
    fn an_empty_title_is_treated_as_absent() {
        let dir = TempDir::new().unwrap();
        let content = format!(
            "{}{}",
            ai_title_line("", "s1"),
            custom_title_line("   ", "s1")
        );
        let p = write_fixture(&dir, "t.jsonl", &content);
        assert_eq!(session_title(&p, "s1"), None);
    }

    #[test]
    fn newlines_inside_a_title_are_flattened() {
        let dir = TempDir::new().unwrap();
        // JSON としては \n を含む 1 行。表示は 1 行なので空白へ潰れること。
        let content =
            "{\"type\":\"ai-title\",\"aiTitle\":\"first\\nsecond\",\"sessionId\":\"s1\"}\n";
        let p = write_fixture(&dir, "t.jsonl", content);
        assert_eq!(session_title(&p, "s1").as_deref(), Some("first second"));
    }

    #[test]
    fn an_overlong_title_is_capped() {
        let dir = TempDir::new().unwrap();
        let long = "あ".repeat(500);
        let p = write_fixture(&dir, "t.jsonl", &ai_title_line(&long, "s1"));
        let title = session_title(&p, "s1").unwrap();
        assert_eq!(title.chars().count(), MAX_TITLE_CHARS);
    }

    #[test]
    fn a_broken_line_does_not_hide_an_earlier_title() {
        let dir = TempDir::new().unwrap();
        let content = format!(
            "{}{}",
            ai_title_line("still readable", "s1"),
            "{\"type\":\"ai-title\", broken json \"sessionId\":\"s1\"}\n"
        );
        let p = write_fixture(&dir, "t.jsonl", &content);
        assert_eq!(session_title(&p, "s1").as_deref(), Some("still readable"));
    }

    /// タイトル行はメタ情報として繰り返し追記されるが、tail に届かないほど
    /// 古ければ諦める（表示しないだけで、誤ったタイトルは出さない）。
    #[test]
    fn a_title_far_beyond_the_tail_window_is_given_up_on() {
        let dir = TempDir::new().unwrap();
        let mut content = ai_title_line("too far back", "s1");
        for i in 0..8000 {
            content.push_str(&format!(
                "{{\"type\":\"user\",\"message\":\"filler line number {i} padding padding padding padding\"}}\n"
            ));
        }
        assert!(
            content.len() as u64 > TITLE_TAIL_BYTES,
            "fixture must exceed TITLE_TAIL_BYTES to exercise the tail-read path"
        );
        let p = write_fixture(&dir, "t.jsonl", &content);
        assert_eq!(session_title(&p, "s1"), None);
    }

    /// 逆に、実測どおり末尾近くに再追記されていれば大きなファイルでも拾える。
    #[test]
    fn a_title_near_the_end_of_a_large_transcript_is_found() {
        let dir = TempDir::new().unwrap();
        let mut content = String::new();
        for i in 0..8000 {
            content.push_str(&format!(
                "{{\"type\":\"user\",\"message\":\"filler line number {i} padding padding padding padding\"}}\n"
            ));
        }
        content.push_str(&ai_title_line("recent enough", "s1"));
        let p = write_fixture(&dir, "t.jsonl", &content);
        assert_eq!(session_title(&p, "s1").as_deref(), Some("recent enough"));
    }
}
