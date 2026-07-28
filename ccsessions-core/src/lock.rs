//! セッションストアの `read → modify → write` を跨いで保持する排他ロック。
//!
//! `write_atomic` が保証するのは「途中状態を読み手に晒さない」ことだけで、
//! read-modify-write 全体は保護しない。Claude Code は**マッチした hook を
//! すべて並列プロセスで実行する**ため、同一セッションに対して複数の
//! `ccsessions hook` が同時に read-modify-write を行い、後勝ちで更新が消える
//! （実測: サブエージェントを 8 並列で起動すると `agents` は 2 件しか残らない）。
//! ここはその窓を閉じるためのプロセス間ロック。
//!
//! # なぜ「ストア全体で 1 個」なのか（セッションごとではなく）
//!
//! セッションごとにロックファイルを持つと、その**寿命**が新しい問題になる:
//!
//! - 消さないと `sessions/` にゼロバイトのファイルが溜まり続ける。daemon は
//!   500ms ごとにこのディレクトリを `read_dir` するので、溜まるほど遅くなる。
//! - かといって消すと、消した瞬間に**同じロックの別インスタンス**が生まれる。
//!   保持中のプロセス A の裏でファイルを消し、プロセス B が同名で作り直すと、
//!   A と B は別 inode をロックしていて排他になっていない。
//!
//! ストア全体で 1 個なら作りっぱなしでよく、この寿命問題が丸ごと消える。
//! 代償は「別セッションの hook 同士も直列化される」ことだが、クリティカル
//! セクションは数 KB の JSON の読み書き 1 往復であり、hook が飛ぶ頻度
//! （モデル往復ごとに数個）に対して無視できる。
//!
//! # なぜセッションファイル自身を flock しないのか
//!
//! `write_atomic` は tmp + rename で書くので、書くたびに **inode が入れ替わる**。
//! `sessions/<id>.json` を開いて flock しても、他のプロセスが rename した後に
//! 同じパスを開いた第三のプロセスは**別の inode** をロックする。パスではなく
//! inode に紐づくという flock の性質上、rename ベースの atomic write とは
//! 原理的に組み合わせられない。だから「rename も削除もされない専用のファイル」
//! を別に置く。
//!
//! # 読み手（daemon）はロックを取らない
//!
//! poller は `list()` で読むだけで、rename の atomicity により古い内容か新しい
//! 内容のどちらかしか見えない（半端な JSON は見えない）。ロックを取らせると
//! hook を待たせるだけで得るものが無い。

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// ロックファイルの名前。先頭が `.` なので `store::list` の
/// 「ドットファイルは読み物として扱わない」フィルタに自動的に引っかかる。
pub(crate) const LOCK_FILE_NAME: &str = ".lock";

/// ロック取得を諦めるまでの時間。
///
/// **待ち続けてはいけない**: hook には Claude Code 側のタイムアウトがあり、
/// `SessionEnd` は既定 1.5 秒しかない（`settings_json::hook_timeout_secs`）。
/// ロックを保持したまま固まったプロセスがいた場合に、こちらまで道連れで
/// タイムアウトするとユーザのターンを待たせることになる。
///
/// **500ms の根拠（実測、debug ビルド・APFS）**: 並列度を上げて総所要の増分を
/// 取ると、直列化される区間は **1 件あたり約 5ms** に収束する
/// （`write_atomic` の `sync_all()` ＝ 実 fsync が支配的）。したがって
/// 500ms は**同時に約 100 プロセスが競合するまで**待ち切れる。実測でも 64 並列
/// では 1 件も deadline を踏まず、128 並列で初めて 2 割ほどが踏んだ。
/// hook の現実的な同時発火数（1 モデル往復あたり数個・`MAX_AGENTS` も 32）に
/// 対して 1 桁以上の余裕がある。
///
/// 上限側も `SessionEnd` の 1.5 秒に収まる: 最悪 500ms 待ってから、hook 本体の
/// 実処理（プロセス起動込みで実測 35ms 前後）を行っても十分間に合う。
///
/// deadline を踏んだ場合は「ロック無しで続行（＝この修正が入る前と同じ挙動）」に
/// 落とす。状態更新を丸ごと落とすより実害が小さい。
const ACQUIRE_TIMEOUT: Duration = Duration::from_millis(500);

/// 再試行の間隔。`flock` の待ちはブロッキング版に任せず、`LOCK_NB` +
/// スリープのループにする（上の deadline を素直に実装できるため。ブロッキングの
/// `flock` を時間で打ち切るにはシグナルで割り込む必要があり、hook のような
/// 単純なプロセスに持ち込む複雑さに見合わない）。
///
/// **代償: 待ちが FIFO にならない。** ブロッキング `flock` ならカーネルが
/// 待ち行列を作るが、ポーリングでは取り損ねたプロセスが何度も競り負けうる。
/// 極端な競合下（実測で 128 並列）では一部が [`ACQUIRE_TIMEOUT`] まで
/// 待たされてロック無しに落ちる。現実的な同時発火数からは 1 桁以上離れており、
/// 落ちた先も「この修正前と同じ挙動」なので許容する。
const RETRY_INTERVAL: Duration = Duration::from_millis(2);

/// 保持している間だけストアへの書き込みを排他するガード。
///
/// `Drop` でファイルが閉じられ、`flock` は OS 側で自動的に解放される
/// （プロセスが panic やシグナルで死んだ場合も同じなので、stale lock が
/// 残らない — これが `O_EXCL` のロックファイル方式より優れている点）。
#[must_use = "ロックはガードが drop された時点で解放される。\
              `let _ = ...` で束縛すると即座に解放されてしまう"]
#[derive(Debug)]
pub struct StoreLock {
    /// fd を生かしておくためだけに保持する。閉じる＝解放。
    _file: fs::File,
    path: PathBuf,
}

impl StoreLock {
    /// ロックファイルのパス（診断・テスト用）。
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// `dir` のストアを排他ロックする。取得できるまで最大 [`ACQUIRE_TIMEOUT`] 待つ。
///
/// 待ち切れなかった場合は `ErrorKind::TimedOut` を返す。呼び出し側（hook）は
/// これを**致命的として扱ってはいけない** — 警告を出してロック無しで続行する。
pub(crate) fn lock_exclusive_in(dir: &Path) -> io::Result<StoreLock> {
    // hook は初回実行時にまだディレクトリが無い状態で叩かれる（write_atomic が
    // 親を作るのと同じ理由）。
    fs::create_dir_all(dir)?;
    let path = dir.join(LOCK_FILE_NAME);

    // `create(true)` は write アクセスを要求するので `write(true)` を付ける。
    // 中身は一切書かないので truncate はしない（他プロセスが保持中のファイルを
    // 開いた瞬間に切り詰めても意味は無いが、無駄な書き込みを発生させない）。
    let file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)?;

    let deadline = Instant::now() + ACQUIRE_TIMEOUT;
    loop {
        // `File::try_lock` の Unix 実装は `flock(2)` の `LOCK_EX | LOCK_NB` そのもの。
        // したがって「open file description に紐づく」「fd を閉じれば解放される」と
        // いう、この module が前提にしている性質はそのまま保たれる。
        match file.try_lock() {
            Ok(()) => return Ok(StoreLock { _file: file, path }),
            // 他プロセスが保持中。待って再試行する。
            Err(fs::TryLockError::WouldBlock) => {}
            // シグナルで中断された。待ちの意味は無いのでそのまま再試行する。
            Err(fs::TryLockError::Error(e)) if e.kind() == io::ErrorKind::Interrupted => continue,
            // それ以外（EBADF・EOPNOTSUPP 等）は再試行しても直らない。
            // NFS 等 flock を実装しないファイルシステムがここに来る。
            Err(fs::TryLockError::Error(e)) => return Err(e),
        }

        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!(
                    "could not acquire the ccsessions store lock at {} within {:?}",
                    path.display(),
                    ACQUIRE_TIMEOUT
                ),
            ));
        }
        std::thread::sleep(RETRY_INTERVAL);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn lock_creates_the_lock_file_and_missing_parent_dir() {
        let dir = TempDir::new().unwrap();
        let sessions = dir.path().join("sessions");
        let lock = lock_exclusive_in(&sessions).unwrap();
        assert!(lock.path().exists());
        assert_eq!(lock.path().file_name().unwrap(), LOCK_FILE_NAME);
    }

    #[test]
    fn a_second_acquisition_is_blocked_while_the_first_is_held() {
        // flock は「開いたファイル記述（open file description）」単位なので、
        // 同一プロセス内でも別々に open すれば競合する。プロセスを起動せずに
        // 排他そのものを確かめられる。
        let dir = TempDir::new().unwrap();
        let held = lock_exclusive_in(dir.path()).unwrap();

        let err = lock_exclusive_in(dir.path()).unwrap_err();
        assert_eq!(
            err.kind(),
            io::ErrorKind::TimedOut,
            "保持中は取得できず TimedOut になるべき: {err}"
        );

        drop(held);
        // 解放後は取れる。
        let _reacquired = lock_exclusive_in(dir.path()).unwrap();
    }

    #[test]
    fn the_lock_file_is_hidden_from_the_session_listing() {
        // `.lock` が `store::list` に「壊れたセッション」として拾われないこと。
        // 拾われると daemon 側で毎回パースを試みる無駄が出る。
        let dir = TempDir::new().unwrap();
        let _lock = lock_exclusive_in(dir.path()).unwrap();
        assert!(
            crate::store::list_in(dir.path()).is_empty(),
            "ロックファイルはセッション一覧に現れてはいけない"
        );
    }
}
