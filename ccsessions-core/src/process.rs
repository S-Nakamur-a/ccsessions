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

/// 持ち主を探すときに**読み飛ばす**プロセス名（＝シェル）。
///
/// シェルは hook を起動するための足場でしかなく、セッションを所有しない。
/// 逆に Claude Code 本体がこの一覧に載ることはありえないので、
/// 「シェルでない最初の祖先」＝持ち主、と判定できる。
const SHELLS: [&str; 8] = ["sh", "bash", "zsh", "dash", "ksh", "fish", "csh", "tcsh"];

/// 祖先をたどる上限。実際に必要なのは 1〜2 段（ラッパー 1 枚＋念のため）で、
/// これは無限ループとプロセステーブルの循環に対する保険。
const MAX_ANCESTRY_DEPTH: usize = 8;

/// プロセス名がシェルか。ログインシェルの `-zsh` や絶対パスの `/bin/sh` も
/// 同じシェルとして扱えるよう、先頭の `-` と親ディレクトリを落としてから見る。
fn is_shell(comm: &str) -> bool {
    let name = comm.rsplit('/').next().unwrap_or(comm);
    let name = name.strip_prefix('-').unwrap_or(name);
    SHELLS.contains(&name)
}

/// 祖先 1 段ぶんの情報。
struct Ancestor {
    ppid: u32,
    comm: String,
}

/// この hook プロセスを起動した Claude Code セッションの pid。
///
/// **直接の親とは限らない。** 直接の親を持ち主にしていた頃、プラグインが配る
/// `ccsessions-hook.sh` を経由すると全セッションが表示されなくなった:
///
/// ```text
/// claude → sh ccsessions-hook.sh → ccsessions hook
///                  ↑ 直接の親。ccsessions が終わった直後に死ぬ
/// ```
///
/// ラッパーは `exec` を**使わない**（exec に失敗すると sh が非 0 で抜けうるため。
/// 「何があっても exit 0」の契約が優先する）ので、この 1 段は消せない。記録した
/// pid が数ミリ秒で死ぬため、書いた直後に死んだセッションとして一掃されていた。
///
/// そこで**シェルを読み飛ばして最初の非シェルの祖先**を持ち主とする。ラッパーが
/// 何段挟まっても、配布形態が変わっても壊れない。
///
/// 分からないときは常に安全側（＝ `None`）に倒す。`None` は「持ち主を特定できな
/// かった」で、pid による回収の対象外になる（TTL だけが受け皿になる）。祖先が
/// シェルばかりで尽きた場合に直接の親を返さないのはそのため — 消える方向の
/// 誤判定は生き物が突然消える形で目に見えるが、少し長く残る方は実害が小さい。
pub fn owner_pid() -> Option<u32> {
    resolve_owner(std::os::unix::process::parent_id(), ancestor)
}

/// `owner_pid` の探索そのもの。プロセステーブルの読み方（FFI）を引数に外に出して
/// あるので、この関数は純粋で、作り物の系図で全経路をテストできる。
///
/// `ancestor` が `None` を返す（＝素性が読めない）祖先は、そこで採用する。
/// 読めないものをシェル扱いして飛ばすと、持ち主を見失う側に倒れてしまう。
fn resolve_owner(start: u32, ancestor: impl Fn(u32) -> Option<Ancestor>) -> Option<u32> {
    let mut pid = start;
    for _ in 0..MAX_ANCESTRY_DEPTH {
        // 里親（launchd = 1）に付け替えられている＝本当の親はもう居ない。
        if pid <= 1 {
            return None;
        }
        let Some(a) = ancestor(pid) else {
            return Some(pid);
        };
        if !is_shell(&a.comm) {
            return Some(pid);
        }
        pid = a.ppid;
    }
    None
}

/// `pid` の親と実行ファイル名。読めなければ `None`。
///
/// `is_zombie` と同じ libproc の flavor を使う（`libc` が型を持っていて、
/// フィールド位置を手で宣言せずに済む）。
#[cfg(target_os = "macos")]
fn ancestor(pid: u32) -> Option<Ancestor> {
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
    if rc != size {
        return None;
    }
    // `pbsi_comm` は `MAXCOMLEN` 丁度だと NUL で終わらないので、長さで区切る。
    let comm: Vec<u8> = info
        .pbsi_comm
        .iter()
        .take_while(|&&c| c != 0)
        .map(|&c| c as u8)
        .collect();
    Some(Ancestor {
        ppid: info.pbsi_ppid,
        comm: String::from_utf8_lossy(&comm).into_owned(),
    })
}

/// macOS 以外では祖先をたどる手段を持たないので、直接の親をそのまま持ち主に
/// する（＝この変更前と同じ挙動）。プラグイン経由の hook は macOS でしか
/// 動かないため、実害があるのは macOS だけ。
#[cfg(not(target_os = "macos"))]
fn ancestor(_pid: u32) -> Option<Ancestor> {
    None
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

    /// 作り物の系図で `resolve_owner` を試すための表。
    /// `(pid, ppid, comm)` の並びをそのまま引ける形にしておく。
    fn family<'a>(rows: &'a [(u32, u32, &'a str)]) -> impl Fn(u32) -> Option<Ancestor> + 'a {
        move |pid| {
            rows.iter()
                .find(|(p, _, _)| *p == pid)
                .map(|(_, ppid, comm)| Ancestor {
                    ppid: *ppid,
                    comm: (*comm).to_string(),
                })
        }
    }

    #[test]
    fn the_direct_parent_is_the_owner_when_it_is_not_a_shell() {
        // settings.json に直接書く配線（shell が exec で置き換わる）の形。
        let rows = [(100, 50, "claude"), (50, 1, "zsh")];
        assert_eq!(resolve_owner(100, family(&rows)), Some(100));
    }

    /// 番人: **プラグインのラッパー越しでも claude 本体を持ち主にする。**
    ///
    /// 実際に踏んだ不具合そのもの（ラッパーの `sh` を持ち主にしたため、
    /// hook が書いた全セッションが 0.5 秒以内に死んだ判定で一掃され、
    /// メニューバーに何も出なかった）。直接の親を返す実装に戻すと落ちる。
    #[test]
    fn the_plugin_wrapper_shell_is_skipped_in_favour_of_the_session_owner() {
        // claude(200) → sh ccsessions-hook.sh(221) → ccsessions hook
        let rows = [(221, 200, "sh"), (200, 150, "claude"), (150, 1, "zsh")];
        assert_eq!(resolve_owner(221, family(&rows)), Some(200));
    }

    #[test]
    fn several_stacked_shells_are_all_skipped() {
        let rows = [
            (303, 302, "sh"),
            (302, 301, "bash"),
            (301, 300, "zsh"),
            (300, 1, "claude"),
        ];
        assert_eq!(resolve_owner(303, family(&rows)), Some(300));
    }

    #[test]
    fn a_login_shell_written_with_a_leading_dash_is_still_a_shell() {
        // ログインシェルは argv[0] が `-zsh` になる。名前の形で取り逃さないこと。
        assert!(is_shell("-zsh"));
        assert!(is_shell("/bin/sh"));
        assert!(!is_shell("claude"));
        // 名前にシェルを含むだけの別物を巻き込まない。
        assert!(!is_shell("shell-helper"));
        assert!(!is_shell("fishd"));
    }

    #[test]
    fn an_ancestor_whose_identity_cannot_be_read_is_taken_as_the_owner() {
        // 素性が読めないものをシェル扱いして飛ばすと持ち主を見失う。
        // 読めなかった時点で打ち切り、安全側（持ち主が居る）に倒す。
        let rows = [(401, 400, "sh")];
        assert_eq!(resolve_owner(401, family(&rows)), Some(400));
    }

    #[test]
    fn an_ancestry_of_only_shells_reports_no_owner() {
        // 直接の親（＝すぐ死ぬシェル）に落とすと、書いた直後に死んだ判定に
        // なってしまう。特定できないなら `None`（＝ pid では回収しない）。
        let rows = [(501, 502, "sh"), (502, 503, "sh"), (503, 1, "zsh")];
        assert_eq!(resolve_owner(501, family(&rows)), None);
    }

    #[test]
    fn a_process_reparented_to_launchd_has_no_identifiable_owner() {
        assert_eq!(resolve_owner(1, family(&[])), None);
        assert_eq!(resolve_owner(0, family(&[])), None);
    }

    #[test]
    fn a_cycle_in_the_ancestry_does_not_hang() {
        let rows = [(601, 602, "sh"), (602, 601, "sh")];
        assert_eq!(resolve_owner(601, family(&rows)), None);
    }

    /// 本物のプロセスに対しても持ち主が取れること。作り物の系図だけだと
    /// `ancestor`（FFI）が壊れていても気づけないので、実プロセスで 1 本通す。
    #[test]
    fn the_owner_of_this_test_process_is_a_live_process() {
        let owner = owner_pid().expect("テストプロセスの持ち主は特定できるはず");
        assert!(is_alive(owner), "持ち主として記録する pid は生きているはず");
    }

    /// **持ち主にシェルを選ばない。** hook を起動したシェルは数ミリ秒で消えるので、
    /// そこを持ち主にすると生きたセッションが即座に死んだ扱いになる。
    #[cfg(target_os = "macos")]
    #[test]
    fn the_owner_is_never_a_shell_process() {
        let owner = owner_pid().unwrap();
        let comm = ancestor(owner)
            .expect("生きている持ち主の素性は読めるはず")
            .comm;
        assert!(!is_shell(&comm), "持ち主にシェルを選んではいけない: {comm}");
    }
}
