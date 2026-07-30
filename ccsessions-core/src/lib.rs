//! ccsessions-core — macOS FFI を含まない共通部分。
//!
//! セッション状態モデル（`session`）・ファイルストア（`store`）・設定（`config`）・
//! hook payload から状態遷移を決める reducer（`event`）・transcript のエラー判定
//! （`transcript`）・プロセスの生存確認（`process`）・一覧から外す条件
//! （`ignore`）・顔（生き物のデザイン）の共通データ型（`face`）・画面に出す文言の
//! 対訳（`lang`）を提供する。
//! `ccsessions`（CLI/hook producer）と `ccsessionsd`（常駐オーバーレイ）の両方から使われる。

pub mod config;
pub mod event;
pub mod face;
pub mod ignore;
pub mod lang;
mod lock;
pub mod process;
pub mod session;
pub mod store;
pub mod transcript;

use std::fs;
use std::io::{self, Write as IoWrite};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

// ---------------------------------------------------------------------------
// Paths
// ---------------------------------------------------------------------------

/// ccsessions の状態ディレクトリ。`CCSESSIONS_STATE_DIR` env > `~/.local/state/ccsessions`。
///
/// 設定ディレクトリではなく XDG の state 領域に置く
/// （セッションファイルは揮発性の実行時状態であり、設定ではないため）。
pub fn state_dir() -> PathBuf {
    if let Ok(v) = std::env::var("CCSESSIONS_STATE_DIR") {
        return PathBuf::from(v);
    }
    home_dir().join(".local/state/ccsessions")
}

/// セッションファイル（`<id>.json`）を置くディレクトリ。
pub fn sessions_dir() -> PathBuf {
    state_dir().join("sessions")
}

/// 設定ファイルのパス。`CCSESSIONS_CONFIG` env > `~/.config/ccsessions/config.toml`。
pub fn config_path() -> PathBuf {
    if let Ok(v) = std::env::var("CCSESSIONS_CONFIG") {
        return PathBuf::from(v);
    }
    home_dir().join(".config/ccsessions/config.toml")
}

/// ユーザ顔（`*.toml`）を置くディレクトリ。設定ファイルの隣。
///
/// ここに TOML を 1 つ置くだけで顔が増える（再ビルド不要）。
/// `CCSESSIONS_CONFIG` で設定ファイルを移している場合はその隣に付いていく。
pub fn faces_dir() -> PathBuf {
    config_path()
        .parent()
        .map(|d| d.join("faces"))
        .unwrap_or_else(|| home_dir().join(".config/ccsessions/faces"))
}

fn home_dir() -> PathBuf {
    #[allow(deprecated)]
    std::env::home_dir().expect("$HOME is not set")
}

/// `content` を `dest` へ atomic に書く（tmp sibling + rename）。
///
/// rename が同一ファイルシステム上で完結するので、途中状態を読み手（poller / hook）に晒さない。tmp 名は
/// `.<basename>.<pid>.<seq>.tmp` で、プロセス間・プロセス内の同時書き込みが
/// 衝突しない。
///
/// 親ディレクトリが無ければ `create_dir_all` で作る
/// （`~/.local/state/ccsessions/sessions/` は初回実行時にまだ存在しない。hook は
/// Claude Code から不意に叩かれるので、呼び出し側に事前準備を強制できない）。
pub fn write_atomic(dest: &Path, content: &str) -> io::Result<()> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);

    let dir = dest.parent().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "dest has no parent directory")
    })?;
    fs::create_dir_all(dir)?;

    let stem = dest
        .file_name()
        .map(|n| n.to_string_lossy())
        .unwrap_or("ccsessions.tmp".into());
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let tmp = dir.join(format!(".{}.{}.{}.tmp", stem, std::process::id(), seq));
    {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(content.as_bytes())?;
        f.sync_all()?;
    }
    fs::rename(&tmp, dest)?;
    Ok(())
}

/// 現在時刻を epoch ミリ秒で返す。
pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn write_atomic_creates_file() {
        let dir = TempDir::new().unwrap();
        let dest = dir.path().join("a.json");
        write_atomic(&dest, "hello\n").unwrap();
        assert_eq!(fs::read_to_string(&dest).unwrap(), "hello\n");
    }

    #[test]
    fn write_atomic_overwrites() {
        let dir = TempDir::new().unwrap();
        let dest = dir.path().join("a.json");
        write_atomic(&dest, "first\n").unwrap();
        write_atomic(&dest, "second\n").unwrap();
        assert_eq!(fs::read_to_string(&dest).unwrap(), "second\n");
    }

    #[test]
    fn write_atomic_creates_missing_parent_dir() {
        let dir = TempDir::new().unwrap();
        let dest = dir.path().join("sessions").join("abc.json");
        write_atomic(&dest, "{}").unwrap();
        assert_eq!(fs::read_to_string(&dest).unwrap(), "{}");
    }

    #[test]
    fn now_ms_is_monotonically_reasonable() {
        // 厳密な単調性は保証しないが、常識的な epoch 範囲であることだけ確認する。
        let ms = now_ms();
        assert!(
            ms > 1_700_000_000_000,
            "now_ms should be a plausible 2023+ epoch ms"
        );
    }
}
