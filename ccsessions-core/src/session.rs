//! セッション状態モデル。
//!
//! `Session` は 1 つの Claude Code セッション（= 1 匹の生き物）を表す。
//! ここに置くのは macOS 非依存の純粋なデータとその派生ロジックのみ。
//! ファイルへの読み書きは `store`、hook payload からの状態遷移は `event` が担う。

use serde::{Deserialize, Serialize};

use crate::lang::{l, Lang};

/// セッションの見た目上の状態。元デザインの 6 状態にそのまま対応する。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
    Working,
    WaitUser,
    WaitAgent,
    Done,
    Error,
    /// 未知の文字列から deserialize したときのフォールバック先でもある
    /// （`from_str` のコメント参照）。`#[serde(other)]` は enum の**最後の
    /// variant** にしか付けられない制約があるため宣言順はここが最後になる
    /// （表示順は別に持つ `ORDER` 定数の並びには影響しない）。
    #[serde(other)]
    Idle,
}

impl SessionState {
    /// 表示・パネル操作 CLI (`ccsessions set --state ...`) 向けの文字列表現。
    pub fn as_str(&self) -> &'static str {
        match self {
            SessionState::Working => "working",
            SessionState::WaitUser => "wait_user",
            SessionState::WaitAgent => "wait_agent",
            SessionState::Idle => "idle",
            SessionState::Done => "done",
            SessionState::Error => "error",
        }
    }

    /// `as_str` の逆変換。未知の文字列は `None`
    /// （こちらは呼び出し側にエラー扱いさせたい CLI 入力用なので `Idle` へは
    /// フォールバックしない。deserialize 時のフォールバックとは用途が異なる:
    /// JSON ファイルが古いバージョンのフィールドを持っていて daemon がクラッシュ
    /// してはいけない場面と、ユーザが `--state` に typo を打った場面は区別すべき）。
    ///
    /// `std::str::FromStr` は実装しない: そちらは `Result<Self, Self::Err>` を
    /// 要求するが、ここでは失敗理由を持たない単純な `Option` で十分なため
    /// （clippy の `should_implement_trait` は意図的に無視する）。
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "working" => Some(SessionState::Working),
            "wait_user" => Some(SessionState::WaitUser),
            "wait_agent" => Some(SessionState::WaitAgent),
            "idle" => Some(SessionState::Idle),
            "done" => Some(SessionState::Done),
            "error" => Some(SessionState::Error),
            _ => None,
        }
    }

    /// 画面に出す状態名。ホバーカード・`ccsessions list`・設定画面が同じものを使う。
    ///
    /// 桁揃えする側（`ccsessions list`）が幅を決められるよう、英語は
    /// 一番長いものでも 14 文字（"Agents running"）に収めてある。
    ///
    /// 英語は README の状態図（`docs/assets/states.svg`）と同じ語にしてある
    /// （図を見てから設定画面を開いた人が、同じ語を探せるように）。
    pub fn label(&self, lang: Lang) -> &'static str {
        match self {
            SessionState::Working => l("作業中", "Working"),
            SessionState::WaitUser => l("判断待ち", "Needs you"),
            SessionState::WaitAgent => l("エージェント待ち", "Agents running"),
            SessionState::Idle => l("アイドル", "Idle"),
            SessionState::Done => l("完了", "Done"),
            SessionState::Error => l("エラー", "Error"),
        }
        .get(lang)
    }

    /// 右上に出すグリフ文字。
    pub fn glyph(&self) -> &'static str {
        match self {
            SessionState::Working => "›",
            SessionState::WaitUser => "!",
            SessionState::WaitAgent => "⋯",
            SessionState::Idle => "z",
            SessionState::Done => "✓",
            SessionState::Error => "×",
        }
    }

    /// メニュー・デモ表示等で決まった順に並べるための一覧。
    pub const ORDER: [SessionState; 6] = [
        SessionState::Working,
        SessionState::WaitUser,
        SessionState::WaitAgent,
        SessionState::Idle,
        SessionState::Done,
        SessionState::Error,
    ];
}

/// 走っているサブエージェント 1 件。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Agent {
    /// 例 "greta", "general-purpose"。`SubagentStart` の `agent_type`。
    pub name: String,
    /// 例 "接地" — 表示用の役割ラベル。
    ///
    /// **hook からは常に空になる**。サブエージェントの追跡を `SubagentStart` に
    /// 移した結果、`description` を持つ `PreToolUse(Agent)` と起動を意味する
    /// `SubagentStart` を突き合わせる手段が payload に無くなったため
    /// （`SubagentStart` は `{agent_id, agent_type}` しか持たない）。時系列で
    /// 近似すると並列起動で別のエージェントに役割が付くので、嘘のラベルを出す
    /// より出さない方を採った。`ccsessions set` とデモ表示のためにフィールドは残す。
    pub role: String,
    pub state: SessionState,
    /// サブエージェントを一意に識別する id（`SubagentStart`/`SubagentStop` の
    /// `agent_id`）。`SubagentStop` で消すときの照合キー。取れなければ空文字。
    #[serde(default)]
    pub id: String,
}

/// 1 セッション = 1 匹の生き物。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Session {
    /// Claude Code の session_id。
    pub id: String,
    /// 表示名（cwd の basename）。
    pub name: String,
    /// Claude Code が付けているセッションタイトル。**同じディレクトリで複数
    /// セッションを走らせたときに、`name` だけでは区別がつかない**のを埋める。
    ///
    /// hook payload には無く、transcript の中にしか無い（`transcript::session_title`）
    /// ので、取れないことがある — 生成前の短いセッション・tail に届かない場合・
    /// Claude Code 側の形式変更。いずれも `None` になるだけで、表示は 1 行に戻る。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub cwd: String,
    pub state: SessionState,
    /// 現在の state になった時刻（epoch ms）。経過時間の起点。
    pub since: u64,
    /// 最後に何かイベントを受けた時刻（epoch ms）。TTL 掃除の基準。
    pub updated: u64,
    #[serde(default)]
    pub agents: Vec<Agent>,
    /// メインスレッドのターンが終わっているか（`Stop` / `StopFailure` で `true`、
    /// `UserPromptSubmit` / `PostToolBatch` で `false`）。
    ///
    /// **`Stop` のあとも走り続けるサブエージェントが居る**ために必要になった
    /// 1 ビット。最後の `SubagentStop` で `agents` が空になったとき、lead がまだ
    /// ターンの途中なら `Working` へ戻すのが正しく、既に終わっているなら `Done` が
    /// 正しい。これが無いと後者を「作業中」と嘘をつき、次の通知が来るまで戻らない。
    ///
    /// `#[serde(default)]` があるので、このフィールドを持たない旧バージョンの
    /// セッションファイルは `false`（＝ターン進行中）として読める。
    #[serde(default)]
    pub main_stopped: bool,
    /// `StopFailure` が運んできた API エラーの種別（`"rate_limit"` /
    /// `"overloaded"` など）。**`Error` 状態のときだけ意味を持つ**。
    ///
    /// `#[serde(default)]` があるので、このフィールドを持たない旧バージョンの
    /// セッションファイルもそのまま読める（逆に新形式を旧 daemon が読んでも
    /// serde は未知フィールドを無視するので、混在しても壊れない）。
    #[serde(default)]
    pub error_kind: Option<String>,
    /// このセッションを持っている Claude Code プロセスの pid。
    ///
    /// hook が毎回書き直す（`ccsessions hook` の親プロセス）。生存確認に使い、
    /// **プロセスが消えていればセッションも死んだとみなす** — `SessionEnd` が
    /// 飛ばない終わり方（強制終了・端末を閉じる・親デーモンの停止）を回収する
    /// ための唯一の手掛かり。
    ///
    /// `None` は「持ち主が分からない」であって「死んでいる」ではない。
    /// 旧バージョンが書いたファイル・`ccsessions set` で外から作ったセッション・
    /// 親 pid を特定できなかった場合が該当し、いずれも TTL だけで判断する。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
}

/// セッションが「生きていない」と判断された理由。
///
/// 消したことを説明できるようにするためにある。生きているセッションを誤って
/// 消してしまったときに、ログから原因（TTL か pid か）を切り分けられないと
/// 手も足も出ないため。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeadReason {
    /// `session_ttl` のあいだ 1 度も hook が来なかった。
    Expired,
    /// 持ち主のプロセスが居なくなった（`SessionEnd` を受け取れなかった終わり方）。
    ProcessGone(u32),
}

impl Session {
    /// 状態を更新する。**状態が変わったときだけ** `since` を更新する — 同じ
    /// 状態への再通知（例: 連続する Notification）で経過時間の起点がリセット
    /// されてしまうと、表示される経過時間が実態より短く見えてしまう。
    /// `updated` は再通知の有無に関わらず常に進める。
    ///
    /// あわせて `Error` 以外へ遷移したら `error_kind` を落とす。「`error_kind`
    /// は `Error` のときだけ意味を持つ」を呼び出し側の規律に任せると、一度
    /// `StopFailure` を受けたセッションが次のターンで成功しても "rate limit"
    /// が残る。不変条件をここ 1 箇所で守る。
    pub fn set_state(&mut self, s: SessionState, now: u64) {
        if s != SessionState::Error {
            self.error_kind = None;
        }
        if self.state != s {
            self.state = s;
            self.since = now;
        }
        self.updated = now;
    }

    /// `Error` 状態にし、種別を記録する。`kind` が `None` なら種別不明のエラー
    /// （transcript から拾ったケース）。`set_state` が `Error` への遷移では
    /// `error_kind` を触らないので、ここで明示的に上書きする。
    pub fn set_error(&mut self, kind: Option<String>, now: u64) {
        self.set_state(SessionState::Error, now);
        self.error_kind = kind;
    }

    /// 生きていないと判断した理由。生きていれば `None`。
    ///
    /// 判定は 2 段構え。**pid の方が TTL より強い証拠**なので、更新が新しくても
    /// プロセスが消えていれば死んだとみなす（`working` のまま殺されたセッションは
    /// TTL 側では 8 時間ずっと「作業中」に見えてしまう）。
    ///
    /// `alive` を引数で受け取るのは、この判断を副作用なしにテストできるように
    /// するため（本番では `crate::process::is_alive` を渡す）。
    pub fn dead_reason(
        &self,
        now: u64,
        session_ttl_ms: u64,
        alive: impl Fn(u32) -> bool,
    ) -> Option<DeadReason> {
        if let Some(pid) = self.pid {
            if !alive(pid) {
                return Some(DeadReason::ProcessGone(pid));
            }
        }
        if now.saturating_sub(self.updated) >= session_ttl_ms {
            return Some(DeadReason::Expired);
        }
        None
    }

    /// `dead_reason` が付かない、つまり表示してよいセッションか。
    pub fn is_live(&self, now: u64, session_ttl_ms: u64, alive: impl Fn(u32) -> bool) -> bool {
        self.dead_reason(now, session_ttl_ms, alive).is_none()
    }

    /// 表示用の状態。`Done` のまま `done_ttl_ms` 以上経過したセッションは
    /// `Idle` として見せる（完了直後は目立たせたいが、いつまでも "✓" のまま
    /// 居座ると生きているセッションと区別がつかなくなるため）。
    pub fn display_state(&self, now: u64, done_ttl_ms: u64) -> SessionState {
        if self.state == SessionState::Done && now.saturating_sub(self.since) >= done_ttl_ms {
            SessionState::Idle
        } else {
            self.state
        }
    }

    /// `name` の最初の `-` までの部分（デザイン仕様: `name.split('-')[0]`）。
    pub fn short_name(&self) -> &str {
        self.name.split('-').next().unwrap_or(&self.name)
    }

    /// 経過時間の整形（デザイン仕様の `fmtDur` の写経）。
    /// 分未満は `0m`、60 分未満は `{m}m`、以上は `{h}h{mm}m`（分は 2 桁ゼロ埋め）。
    pub fn fmt_dur(ms: u64) -> String {
        let total_min = ms / 60_000;
        if total_min < 60 {
            format!("{}m", total_min)
        } else {
            let h = total_min / 60;
            let m = total_min % 60;
            format!("{}h{:02}m", h, m)
        }
    }

    /// cwd から表示名（basename）を導出する。空なら `"?"`。
    pub fn name_from_cwd(cwd: &str) -> String {
        let trimmed = cwd.trim_end_matches('/');
        match trimmed.rsplit('/').next() {
            Some(s) if !s.is_empty() => s.to_string(),
            _ => "?".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- SessionState ------------------------------------------------------

    #[test]
    fn as_str_round_trips_through_from_str() {
        for s in SessionState::ORDER {
            assert_eq!(SessionState::from_str(s.as_str()), Some(s));
        }
    }

    #[test]
    fn every_state_is_named_in_both_languages() {
        for s in SessionState::ORDER {
            for lang in [Lang::Ja, Lang::En] {
                assert!(!s.label(lang).is_empty(), "{s:?} に {lang:?} の名前が無い");
            }
        }
    }

    /// `ccsessions list` は状態名の列を固定幅で揃える。**一番長い英語の名前が
    /// その幅に収まっていること** — はみ出すと右隣のセッション名の列が押し出され、
    /// 行ごとに桁がずれる（日本語だけ見て幅を決めると必ず踏む）。
    #[test]
    fn no_state_name_overflows_the_list_column() {
        const LIST_COLUMN: usize = 14;
        for s in SessionState::ORDER {
            for lang in [Lang::Ja, Lang::En] {
                let n = s.label(lang).chars().count();
                assert!(
                    n <= LIST_COLUMN,
                    "{:?} は {n} 文字で、list の {LIST_COLUMN} 桁に収まらない",
                    s.label(lang)
                );
            }
        }
    }

    #[test]
    fn from_str_rejects_unknown() {
        assert_eq!(SessionState::from_str("bogus"), None);
    }

    #[test]
    fn unknown_json_string_deserializes_to_idle() {
        // 将来バージョンが未知の state 値を書き込んでも、古い daemon はクラッシュ
        // せず Idle にフォールバックする（fail-safe）。
        let v: SessionState = serde_json::from_str("\"future_state\"").unwrap();
        assert_eq!(v, SessionState::Idle);
    }

    #[test]
    fn known_json_strings_deserialize_correctly() {
        for s in SessionState::ORDER {
            let json = format!("\"{}\"", s.as_str());
            let v: SessionState = serde_json::from_str(&json).unwrap();
            assert_eq!(v, s);
        }
    }

    // ---- Session::set_state -------------------------------------------------

    fn session(state: SessionState, since: u64, updated: u64) -> Session {
        Session {
            id: "s1".into(),
            name: "proj".into(),
            title: None,
            cwd: "/tmp/proj".into(),
            state,
            since,
            updated,
            agents: vec![],
            main_stopped: false,
            error_kind: None,
            pid: None,
        }
    }

    #[test]
    fn set_state_updates_since_on_transition() {
        let mut s = session(SessionState::Working, 1000, 1000);
        s.set_state(SessionState::WaitUser, 5000);
        assert_eq!(s.state, SessionState::WaitUser);
        assert_eq!(s.since, 5000);
        assert_eq!(s.updated, 5000);
    }

    // ---- error_kind の後方/前方互換 ------------------------------------------

    #[test]
    fn session_file_without_error_kind_still_loads() {
        // `error_kind` を持たない旧バージョンのセッションファイルが読めること。
        let json = r#"{"id":"s1","name":"proj","cwd":"/tmp/proj",
                       "state":"working","since":1000,"updated":2000,"agents":[]}"#;
        let s: Session = serde_json::from_str(json).unwrap();
        assert_eq!(s.error_kind, None);
        assert_eq!(s.state, SessionState::Working);
    }

    #[test]
    fn unknown_fields_are_ignored_so_new_files_load_in_old_builds() {
        // 逆向きの互換: serde は未知フィールドを黙って無視するので、新しい
        // フィールドを足した daemon が書いたファイルも古い読み手で壊れない。
        let json = r#"{"id":"s1","name":"proj","cwd":"/tmp/proj","state":"error",
                       "since":1,"updated":2,"agents":[],"error_kind":"rate_limit",
                       "some_future_field":42}"#;
        let s: Session = serde_json::from_str(json).unwrap();
        assert_eq!(s.error_kind.as_deref(), Some("rate_limit"));
    }

    #[test]
    fn set_state_clears_error_kind_when_leaving_error() {
        let mut s = session(SessionState::Error, 1000, 1000);
        s.error_kind = Some("rate_limit".into());
        s.set_state(SessionState::Working, 5000);
        assert_eq!(
            s.error_kind, None,
            "error_kind only means anything in Error"
        );
    }

    #[test]
    fn set_error_records_the_kind() {
        let mut s = session(SessionState::Working, 1000, 1000);
        s.set_error(Some("overloaded".into()), 5000);
        assert_eq!(s.state, SessionState::Error);
        assert_eq!(s.error_kind.as_deref(), Some("overloaded"));
        assert_eq!(s.since, 5000);
    }

    #[test]
    fn set_error_twice_keeps_since_but_updates_kind() {
        // 同じ Error 状態への再通知では経過時間の起点を戻さないが、
        // 種別は新しいものに差し替わる。
        let mut s = session(SessionState::Working, 1000, 1000);
        s.set_error(Some("rate_limit".into()), 5000);
        s.set_error(Some("overloaded".into()), 9000);
        assert_eq!(s.since, 5000, "same-state renotify must not reset since");
        assert_eq!(s.error_kind.as_deref(), Some("overloaded"));
        assert_eq!(s.updated, 9000);
    }

    #[test]
    fn set_state_same_state_keeps_since_but_bumps_updated() {
        // 再通知だけでは経過時間の起点がリセットされない。
        let mut s = session(SessionState::Working, 1000, 1000);
        s.set_state(SessionState::Working, 9000);
        assert_eq!(s.since, 1000, "same-state renotify must not reset since");
        assert_eq!(s.updated, 9000, "updated must always advance");
    }

    // ---- dead_reason / is_live -----------------------------------------------

    /// pid を持たない（＝持ち主が分からない）セッションは TTL だけで判断する。
    /// 旧バージョンが書いたファイルや `ccsessions set` 由来のセッションを、
    /// 生存確認できないという理由だけで消してはいけない。
    #[test]
    fn a_session_without_a_pid_is_judged_by_ttl_alone() {
        let s = session(SessionState::Working, 0, 1000);
        assert!(s.is_live(2000, 5000, |_| panic!("pid が無ければ生存確認は呼ばない")));
        assert_eq!(
            s.dead_reason(6001, 5000, |_| panic!("同上")),
            Some(DeadReason::Expired)
        );
    }

    #[test]
    fn a_session_whose_process_is_gone_is_dead_even_if_freshly_updated() {
        // 本題: `working` のまま強制終了されたセッション。更新は 1 秒前なので
        // TTL では 8 時間ずっと生き残ってしまうが、pid で即座に死と分かる。
        let mut s = session(SessionState::Working, 0, 1000);
        s.pid = Some(4242);
        assert_eq!(
            s.dead_reason(2000, 8 * 3600 * 1000, |_| false),
            Some(DeadReason::ProcessGone(4242))
        );
    }

    #[test]
    fn a_session_whose_process_is_alive_survives_long_silence_until_ttl() {
        // 逆向きの保証: プロセスが生きている限り、黙っていても TTL までは残す。
        let mut s = session(SessionState::Idle, 0, 1000);
        s.pid = Some(4242);
        assert!(s.is_live(1000 + 4999, 5000, |_| true));
        // ただし TTL を超えたら（プロセスが生きていても）掃除の対象にする。
        assert_eq!(
            s.dead_reason(1000 + 5000, 5000, |_| true),
            Some(DeadReason::Expired)
        );
    }

    /// 生存確認が判断できないとき（`is_alive` が安全側の `true` を返すとき）は
    /// 消さない、という契約を型の上でも固定しておく。
    #[test]
    fn an_unverifiable_process_is_kept() {
        let mut s = session(SessionState::Working, 0, 1000);
        s.pid = Some(1);
        assert!(s.is_live(2000, 5000, crate::process::is_alive));
    }

    // ---- serde 互換 -------------------------------------------------------------

    #[test]
    fn a_session_file_without_pid_still_loads() {
        // 旧バージョンが書いた JSON（pid フィールドが無い）を読めること。
        let json = r#"{"id":"s1","name":"proj","cwd":"/tmp/proj","state":"idle",
                       "since":1,"updated":2,"agents":[]}"#;
        let s: Session = serde_json::from_str(json).unwrap();
        assert_eq!(s.pid, None);
    }

    #[test]
    fn a_session_file_without_title_still_loads() {
        // タイトル対応前のバージョンが書いた JSON も読めること。
        let json = r#"{"id":"s1","name":"proj","cwd":"/tmp/proj","state":"idle",
                       "since":1,"updated":2,"agents":[]}"#;
        let s: Session = serde_json::from_str(json).unwrap();
        assert_eq!(s.title, None);
    }

    #[test]
    fn title_is_omitted_from_json_when_absent() {
        let s = session(SessionState::Idle, 0, 0);
        assert!(!serde_json::to_string(&s).unwrap().contains("title"));
        let mut titled = s.clone();
        titled.title = Some("直近のバグ調査".into());
        assert!(serde_json::to_string(&titled)
            .unwrap()
            .contains("\"title\":\"直近のバグ調査\""));
    }

    #[test]
    fn pid_is_omitted_from_json_when_absent() {
        // 旧 daemon が読んでも困らないよう、無いときはキーごと出さない。
        let s = session(SessionState::Idle, 0, 0);
        assert!(!serde_json::to_string(&s).unwrap().contains("pid"));
        let mut with_pid = s.clone();
        with_pid.pid = Some(99);
        assert!(serde_json::to_string(&with_pid)
            .unwrap()
            .contains("\"pid\":99"));
    }

    // ---- display_state -------------------------------------------------------

    #[test]
    fn display_state_shows_done_before_ttl() {
        let s = session(SessionState::Done, 1000, 1000);
        assert_eq!(s.display_state(1000 + 179_000, 180_000), SessionState::Done);
    }

    #[test]
    fn display_state_falls_back_to_idle_after_ttl() {
        let s = session(SessionState::Done, 1000, 1000);
        assert_eq!(s.display_state(1000 + 180_000, 180_000), SessionState::Idle);
    }

    #[test]
    fn display_state_non_done_is_unaffected_by_ttl() {
        let s = session(SessionState::Working, 1000, 1000);
        assert_eq!(
            s.display_state(1000 + 999_999_999, 180_000),
            SessionState::Working
        );
    }

    // ---- short_name ----------------------------------------------------------

    #[test]
    fn short_name_splits_on_first_hyphen() {
        let mut s = session(SessionState::Idle, 0, 0);
        s.name = "ccsessions-core".into();
        assert_eq!(s.short_name(), "ccsessions");
    }

    #[test]
    fn short_name_no_hyphen_returns_whole_name() {
        let mut s = session(SessionState::Idle, 0, 0);
        s.name = "overlay".into();
        assert_eq!(s.short_name(), "overlay");
    }

    // ---- fmt_dur ---------------------------------------------------------------

    #[test]
    fn fmt_dur_matches_design_spec_fixtures() {
        assert_eq!(Session::fmt_dur(0), "0m");
        assert_eq!(Session::fmt_dur(59_999), "0m");
        assert_eq!(Session::fmt_dur(60_000), "1m");
        assert_eq!(Session::fmt_dur(3_599_999), "59m");
        assert_eq!(Session::fmt_dur(3_600_000), "1h00m");
        assert_eq!(Session::fmt_dur(4_500_000), "1h15m");
    }

    // ---- name_from_cwd ---------------------------------------------------------

    #[test]
    fn name_from_cwd_basename() {
        assert_eq!(Session::name_from_cwd("/Users/x/ghq/proj"), "proj");
    }

    #[test]
    fn name_from_cwd_trailing_slash() {
        assert_eq!(Session::name_from_cwd("/Users/x/proj/"), "proj");
    }

    #[test]
    fn name_from_cwd_empty_is_question_mark() {
        assert_eq!(Session::name_from_cwd(""), "?");
        assert_eq!(Session::name_from_cwd("/"), "?");
    }
}
