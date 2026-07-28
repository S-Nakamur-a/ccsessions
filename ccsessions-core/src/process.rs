//! プロセスの生存確認。
//!
//! セッションを消す唯一の正規経路は `SessionEnd` hook だが、**Claude Code が
//! 強制終了（SIGKILL・端末ごと閉じる・親デーモンの再起動）されると SessionEnd は
//! 飛んでこない**。そのままだと死んだセッションが `session_ttl`（既定 8 時間）
//! いっぱい居座り、`max_sessions` の枠を食って生きているセッションを押し出す。
//! そこで hook がセッションの持ち主 pid を記録し、こちらで生存を確かめる。
//!
//! **判定は必ず安全側（生きている）に倒す**。生きているセッションを消す事故は
//! 生き物が突然消える形で目に見えるが、死んだセッションが少し長く残るのは
//! TTL がいずれ回収するため実害が小さい。
//!
//! ただし「存在するか」だけでは足りない。**ゾンビ**（終了したが親が `wait`
//! していない残骸）は `kill(pid, 0)` に成功を返すため、親が回収しない限り
//! 永久に生きている扱いになる。回収しない親は実在する（Conductor は配下の
//! `claude` を `wait` しないことがあり、ゾンビが十数匹溜まっていた）ので、
//! ゾンビは明示的に弾く。この module が macOS 固有の API を使うのはそのため。

/// `pid` のプロセスが**存在し、かつゾンビでない**か。
/// **判断できないときは `true`（生きている扱い）**。
///
/// 2 段構えで、どちらも「死」と言い切れたときだけ `false` を返す:
///
/// 1. `exists` … `kill(pid, 0)` でプロセステーブルに居るか。
/// 2. `is_zombie` … 居るとして、それが**終了済みの残骸**でないか。
///
/// ゾンビを弾くのがこの関数の要点。`kill(pid, 0)` はゾンビにも成功を返すため、
/// 1 だけでは「親が `wait` しない子」を永久に生きている扱いにしてしまう
/// （実際に Conductor 配下の `claude` がそうなり、判断待ちのセッションが
/// 8 時間居座った）。詳しくは `is_zombie` の doc と ADR 0022。
///
/// 残っている既知の限界は **pid の再利用** だけ。死んだセッションの pid を
/// 無関係なプロセスが再利用すると「生きている」と誤判定する。表示が TTL まで
/// 残るだけで、消える事故にはならない（＝安全側）。
pub fn is_alive(pid: u32) -> bool {
    // pid 0 は `kill(2)` では「自分のプロセスグループ全員」を意味する特別値で、
    // 生存確認には使えない。1 は launchd/init で、セッションの持ち主ではありえない
    // （親が先に死んで里親に出された痕跡）。どちらも「分からない」として扱う。
    if pid <= 1 {
        return true;
    }
    exists(pid) && !is_zombie(pid)
}

/// プロセステーブルに `pid` のエントリがあるか。**ゾンビも「ある」に数える**。
///
/// `kill(pid, 0)` はシグナルを送らずに「その pid にシグナルを送れるか」だけを
/// 確かめる POSIX の定石。戻り値の意味は:
///
/// - `0`      … 存在する。
/// - `EPERM`  … 存在するが自分の権限では送れない（別ユーザ）。→ 存在する。
/// - `ESRCH`  … **そんなプロセスは居ない**。→ ここだけが「無い」の判定。
/// - その他   … 想定外。安全側に倒して存在する扱いにする。
fn exists(pid: u32) -> bool {
    // SAFETY: kill(2) にシグナル 0 を渡すだけ。プロセスの状態は一切変えない。
    let rc = unsafe { libc::kill(pid as libc::pid_t, 0) };
    if rc == 0 {
        return true;
    }
    std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
}

/// `pid` が**ゾンビ**（終了したが親が `wait` していない残骸）だと確認できるか。
/// **確認できないときは `false`（ゾンビではない＝生きている扱い）**。
///
/// macOS では libproc に問い合わせる。`kill(2)` と違い、libproc は
/// **ゾンビを「タスクが無い」として扱う**ので、ここで初めて残骸と区別がつく。
/// 実測（本物の Conductor 配下ゾンビ 3 匹＋作りたてのゾンビ 1 匹）:
///
/// | | `kill(pid, 0)` | `proc_pidinfo` |
/// |---|---|---|
/// | ゾンビ | `0`（生きて見える） | 失敗・`ESRCH` |
/// | 生きている | `0` | 成功、`pbsi_status` = `SRUN` 等 |
/// | 他ユーザの生きたプロセス | `-1`・`EPERM` | **成功**（権限を要らない flavor なので） |
///
/// したがって判定は「成功して `SZOMB`」または「`ESRCH` で失敗」。それ以外の
/// 失敗（`EPERM`・想定外の短い読み取り）は**分からない**として生きている側に倒す。
///
/// `sysctl(KERN_PROC_PID)` の `kp_proc.p_stat` を見る道もあり、そちらは
/// ゾンビに対して `SZOMB` を直接返すことも実測できている。採らなかったのは
/// `libc` が macOS 向けに `kinfo_proc`（648 バイト）を定義しておらず、
/// フィールド位置を手で宣言することになるため。ずれても気づかず、しかも
/// 「ごみを 5 と読んで生きたセッションを消す」危険側に倒れる。
/// libproc 側は `libc` が型を持っているので、その risk が無い。
#[cfg(target_os = "macos")]
fn is_zombie(pid: u32) -> bool {
    // SAFETY: すべて整数と `c_char` 配列なので、全ゼロは有効な値。
    let mut info: libc::proc_bsdshortinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_bsdshortinfo>() as libc::c_int;
    // SAFETY: 自分が所有する `info` に、その実サイズを渡して書かせるだけ。
    // 読み取り専用の問い合わせで、対象プロセスには一切触らない。
    let rc = unsafe {
        libc::proc_pidinfo(
            pid as libc::c_int,
            libc::PROC_PIDT_SHORTBSDINFO,
            0,
            (&mut info as *mut libc::proc_bsdshortinfo).cast(),
            size,
        )
    };
    if rc == size {
        // 全部読めた。状態を直接見る。
        return info.pbsi_status == libc::SZOMB;
    }
    // 読めなかった。`ESRCH`（＝生きたタスクが無い）だけがゾンビの合図。
    // ここは `exists` を通った後なので、プロセステーブルにエントリはある。
    std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
}

/// macOS 以外では判定手段を持たないので、常に「ゾンビではない」を返す
/// （＝ ADR 0008 のままの挙動になる）。Linux で必要になったら
/// `/proc/<pid>/stat` の 3 番目のフィールドが `Z` かを見ることになる。
#[cfg(not(target_os = "macos"))]
fn is_zombie(_pid: u32) -> bool {
    false
}

/// この hook プロセスを起動した Claude Code セッションの pid。
///
/// Claude Code は hook のコマンドを**自分の直接の子として**起動する
/// （`sh -c "<cmd>"` 経由でも単純コマンドなら shell は exec で置き換わるので、
/// 親は claude 本体のまま）。実測でも `ccsessions hook` 相当のスクリプトの親は
/// `claude` プロセスそのものだった。
///
/// 親が既に居なくなっていれば里親（launchd = 1）に付け替えられているので、
/// `<= 1` は「持ち主を特定できなかった」として `None` を返す — 記録した pid が
/// 最初から嘘だと、生きているセッションを死んだと誤判定してしまうため。
pub fn owner_pid() -> Option<u32> {
    let ppid = std::os::unix::process::parent_id();
    (ppid > 1).then_some(ppid)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_current_process_is_alive() {
        assert!(is_alive(std::process::id()));
    }

    #[test]
    fn a_reaped_child_is_not_alive() {
        // 生存確認が実際に「死」を検出できることを、本物のプロセスで確かめる。
        // wait まで済ませて回収しないとゾンビのまま生存扱いになるので、
        // `wait()` を通してから判定する。
        let mut child = std::process::Command::new("/usr/bin/true")
            .spawn()
            .expect("/usr/bin/true should be spawnable");
        let pid = child.id();
        child.wait().expect("child should be waitable");
        assert!(!is_alive(pid), "回収済みの子プロセスは死んでいるはず");
    }

    /// 番人: **親が `wait` しない子（ゾンビ）は死んだ判定になる。**
    ///
    /// 実際に踏んだ不具合そのもの（Conductor が `claude` を `wait` せず、
    /// 判断待ちのセッションが TTL の 8 時間ずっと表示され続けた）。`kill(pid, 0)`
    /// だけの判定に戻すとこのテストが落ちる。
    ///
    /// macOS 限定 — 他 OS には判定手段を用意していないので、そちらでは
    /// 意図的に「生きている」を返す（`is_zombie` の doc 参照）。
    #[cfg(target_os = "macos")]
    #[test]
    fn a_child_nobody_waited_for_is_not_alive() {
        // `wait` を呼ばない（Rust の `Child` は drop でも回収しない）ので、
        // 終了した瞬間からゾンビになる。
        let mut child = std::process::Command::new("/usr/bin/true")
            .spawn()
            .expect("/usr/bin/true should be spawnable");
        let pid = child.id();

        // 子が終了するまでは本当に生きているため、ゾンビになるのを待つ。
        let mut became_dead = false;
        for _ in 0..500 {
            if !is_alive(pid) {
                became_dead = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(
            became_dead,
            "wait していない子はゾンビとして死んだ判定になるはず"
        );

        // 同時に、この時点でも `kill(pid, 0)` は成功する＝ゾンビはまだ
        // プロセステーブルに居る、という前提を固定しておく。ここが崩れたら
        // このテストはゾンビではなく別のものを見ていることになる。
        assert!(
            exists(pid),
            "回収前のゾンビはプロセステーブルには居るはず（kill だけでは死と分からない）"
        );

        child.wait().expect("child should be waitable");
    }

    #[test]
    fn special_pids_are_treated_as_unknown_and_kept() {
        // 0 と 1 は「持ち主の pid ではない」ので、消す方向へは倒さない。
        assert!(is_alive(0));
        assert!(is_alive(1));
    }

    #[test]
    fn owner_pid_is_the_parent_process() {
        // テストバイナリの親（cargo test / シェル）は必ず居るので Some になる。
        assert_eq!(owner_pid(), Some(std::os::unix::process::parent_id()));
    }
}
