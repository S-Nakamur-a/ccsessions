//! hook payload → 状態遷移の純関数 reducer。
//!
//! Claude Code の hook は stdin に JSON を渡してくる。`reduce` はそれと現在の
//! セッション（あれば）から次の状態を決めるだけの純関数にしてある — I/O は
//! すべて呼び出し側（`ccsessions hook`）の責務にすることで、この核心ロジックを
//! ファイルシステムなしでテストできるようにするため。
//!
//! 購読しているイベントと遷移は Claude Code 2.1.220 の実バイナリから抽出した
//! zod スキーマに合わせてある。特に次の 3 つは「実装の想像」と実際の payload が
//! ズレていて、本番で静かに壊れていた箇所:
//!
//! - `StopFailure` は **`Stop` の代わりに**発火する。API エラーで終わったターンでは
//!   `Stop` が来ないので、`Stop` だけを見ているとエラー状態に到達できない。
//! - `SubagentStop` の payload に `tool_use_id` は**無い**。照合は `agent_id` で行う。
//! - `PostToolBatch` はバッチごとに 1 回だけ発火する。「判断待ち」からの復帰は
//!   これで行う（`PreToolUse` は matcher で絞られていて届かない）。
//! - `Stop` は「メインスレッドのターンの終わり」であって**サブエージェントの
//!   終わりではない**。走り続けているものは `Stop` の `background_tasks` に載って
//!   いるので、`agents` を消さずにそれと突き合わせる（`reconcile_agents`）。
//!
//! どのイベントをなぜ購読しているかは `docs/adr/0005-subscribed-hook-events.md`。

use serde::Deserialize;

use crate::session::{Agent, Session, SessionState};

/// Claude Code hook の stdin JSON。
///
/// イベント別の追加フィールドはすべて `Option` にしてある。欠けていても壊れない
/// こと（`#[serde(default)]` をコンテナに付け、Deserialize 導出可能にするため
/// `Default` も導出）。未知フィールドは `deny_unknown_fields` を付けず黙って
/// 無視する（Claude Code 側がフィールドを追加しても hook が落ちないように）。
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct HookPayload {
    pub session_id: Option<String>,
    pub transcript_path: Option<String>,
    pub cwd: Option<String>,
    pub hook_event_name: Option<String>,
    /// 全イベント共通。**サブエージェント内から発火したときだけ存在する**
    /// （メインスレッドでは `--agent` 起動でも absent）。したがって「サブ
    /// エージェント由来か」の判定はこのフィールドで行う — `agent_type` は
    /// `--agent` 起動のメインスレッドにも付くので判定に使えない。
    ///
    /// **例外**: `SubagentStart` / `SubagentStop` ではこれが「発火元」ではなく
    /// **対象のエージェント**を指す（スキーマが共通フィールドを明示的に上書き
    /// している）。意味が違うので取り違えないこと。
    pub agent_id: Option<String>,
    /// `SubagentStart` / `SubagentStop` の対象種別（例 `"general-purpose"`）。
    /// 共通フィールドとしても来る（サブエージェント内発火・`--agent` 起動時）。
    pub agent_type: Option<String>,
    /// `Notification` の種別。`permission_prompt` / `idle_prompt` /
    /// `auth_success` / `elicitation_dialog` / `elicitation_complete` /
    /// `elicitation_response` / `agent_needs_input` / `agent_completed`。
    /// **スキーマ上は enum ではなく素の string** なので未知の値が来うる。
    pub notification_type: Option<String>,
    /// `StopFailure` の API エラー種別（`rate_limit` / `overloaded` /
    /// `authentication_failed` など 10 種）。
    pub error: Option<String>,
    /// `PreToolUse` の `"Bash"` / `"Agent"` など。現在は購読していないが、
    /// payload の形として残しておく（`--record` した実物を読むときの目印）。
    pub tool_name: Option<String>,
    /// `PreToolUse` の tool_input。形は Claude Code のバージョンに依存するため
    /// `serde_json::Value` のまま持つ。
    pub tool_input: Option<serde_json::Value>,
    /// ツール呼び出しの一意 id。**`SubagentStop` には付かない**（照合は `agent_id`）。
    pub tool_use_id: Option<String>,
    /// `SessionEnd` の終了理由。
    pub reason: Option<String>,
    /// `SessionStart` の起動元。
    pub source: Option<String>,
    /// **`Stop`（と `SubagentStop`）だけ**が運んでくる「まだ走っている
    /// バックグラウンドの仕事」の一覧。Claude Code 側で status が
    /// running / pending のものに絞られている。
    ///
    /// `Stop` は「メインスレッドのターンが終わった」であって「サブエージェントも
    /// 終わった」ではない。生死を知る手掛かりはこのフィールドしかないので、
    /// **`None`（フィールドごと無い）と `Some(空)` を区別する** — 前者は
    /// 「教えてくれない古い Claude Code」、後者は「本当に何も走っていない」。
    pub background_tasks: Option<Vec<BackgroundTask>>,
}

/// `Stop` payload の `background_tasks` の 1 件。
///
/// 実物（Claude Code 2.1.220 で収録）:
///
/// ```json
/// {"id":"a6b95b3843cb27b86","type":"subagent","status":"running",
///  "description":"probe agent","agent_type":"general-purpose"}
/// ```
///
/// **`id` は `SubagentStart` / `SubagentStop` の `agent_id` と同じ値**
/// （Claude Code のタスク台帳が agent id をキーにしているため）。これで
/// 「hook で追跡してきた `agents`」と「実際に走っているもの」を突き合わせられる。
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct BackgroundTask {
    /// `SubagentStart.agent_id` と同じ値（エージェント系のタスクの場合）。
    pub id: Option<String>,
    /// `"subagent"` / `"teammate"` / `"shell"` / `"workflow"` / `"monitor"` /
    /// `"MCP task"` / `"cloud session"` など。`type` は Rust の予約語なので改名する。
    #[serde(rename = "type")]
    pub kind: Option<String>,
    /// `"running"` / `"pending"`。
    pub status: Option<String>,
    /// Agent ツールの `description`（＝ホバーカードの役割ラベル）。
    /// teammate ではタスクの説明が入る。
    pub description: Option<String>,
    /// `"subagent"` のときの種別（例 `"general-purpose"`）。teammate には付かない。
    pub agent_type: Option<String>,
}

/// reducer の出力。呼び出し側（CLI）がこれを見てストアを更新する。
pub enum Outcome {
    /// このセッションファイルを書く。
    Upsert(Session),
    /// このセッションファイルを消す。
    Remove(String),
    /// 何もしない（未知イベント・session_id 欠落・状態に関係ない通知）。
    Ignore,
}

/// 1 セッションが同時に保持できる agent の上限。
///
/// バッジの値は `agents.len()` をそのまま使うので、上限で打ち止めると表示が
/// 実態より小さく見える歪みが生じる。32 は実運用のサブエージェント同時実行数を
/// 大きく超える値を選び、「歪みが発生する上限」と「無限に push され続けるのを
/// 防ぐ」の両立を図った（ネストしたサブエージェントの起動が続いても Vec が
/// 際限なく肥大しないようにする安全弁）。
const MAX_AGENTS: usize = 32;

/// `agent_type` が取れなかったときの表示名。
const FALLBACK_AGENT_NAME: &str = "agent";

/// 無操作タイマーの `Notification`（"Claude is waiting for your input"）。
const IDLE_PROMPT: &str = "idle_prompt";

/// `background_tasks` の `type` のうち、**生き物の「エージェント待ち」に数えるもの**。
///
/// `SubagentStart` / `SubagentStop` が飛んでくる（＝ `agent_id` で追跡できる）種別だけ
/// を入れてある。`shell`（バックグラウンドの Bash）・`monitor`・`MCP task` は数えない
/// — `sleep` を 1 つ投げただけでターンが完了しなくなってしまい、しかもそれらは
/// サブエージェントではないのでホバーカードに並べる相手でもない。
const AGENT_TASK_KINDS: [&str; 2] = ["subagent", "teammate"];

/// 現在のセッション（無ければ `None`）と hook payload から次の状態を決める。
///
/// `now` は epoch ms。`error` は `transcript::last_turn_errored` で判定した
/// エラーフラグ（`Stop` 以外では呼び出し側が常に `false` を渡す想定）。
pub fn reduce(prev: Option<Session>, p: &HookPayload, now: u64, error: bool) -> Outcome {
    let session_id = match p.session_id.as_deref() {
        Some(id) if !id.is_empty() => id.to_string(),
        _ => return Outcome::Ignore,
    };
    let event = match p.hook_event_name.as_deref() {
        Some(e) => e,
        None => return Outcome::Ignore,
    };

    // サブエージェント内から発火したイベントは親セッションに何もしない。
    // 例: サブエージェントが Bash を呼べば、その PostToolBatch が親の session_id
    // で飛んでくる。これで親を Working に戻すと、親が本当は判断待ちでも塗り
    // つぶされる。
    //
    // `updated` すら進めず完全に無視する理由が 2 つある:
    //
    // - **書き込みを増やさない**。hook は「マッチしたものすべてが並列プロセス」
    //   で走る一方、`load → reduce → save` 全体はアトミックでない（`store` の
    //   doc コメント参照）。`updated` を進めるためだけに書くと、その 1 回ぶん
    //   lost update の窓が増える — 得るものが TTL の鮮度だけなのに割に合わない。
    // - **TTL は親のイベントで十分保たれる**。セッション TTL は既定 8 時間で、
    //   1 ターンの間に必ず `UserPromptSubmit` と `Stop`/`StopFailure` が親から
    //   来るため、サブエージェントの活動で延命する必要がない。
    //
    // 例外は SubagentStart / SubagentStop — そこでの `agent_id` は「発火元」では
    // なく「対象のエージェント」なので、この判定に使ってはいけない。
    if fired_inside_subagent(p) && !matches!(event, "SubagentStart" | "SubagentStop") {
        return Outcome::Ignore;
    }

    match event {
        "SessionStart" => {
            // 既存があれば state 据え置き・updated だけ更新。無ければ新規作成
            // （state=Idle, agents=[]）。base_session がそのまま両方を満たす。
            let mut s = base_session(&prev, &session_id, p, now);
            s.updated = now;
            Outcome::Upsert(s)
        }
        "UserPromptSubmit" => {
            let mut s = base_session(&prev, &session_id, p, now);
            // 新しいターンの開始なので前ターンの agent は残骸。通常は Stop の
            // 突き合わせが消しているが、中断（ESC）では拾えないことがある。
            s.agents.clear();
            s.main_stopped = false;
            s.set_state(SessionState::Working, now);
            Outcome::Upsert(s)
        }
        "PermissionRequest" => {
            let mut s = base_session(&prev, &session_id, p, now);
            s.set_state(SessionState::WaitUser, now);
            Outcome::Upsert(s)
        }
        "Notification" => {
            if !notification_means_wait_user(p.notification_type.as_deref()) {
                // 完了通知（認証成功・エージェント完了など）は状態と無関係。
                return Outcome::Ignore;
            }
            let mut s = base_session(&prev, &session_id, p, now);
            // `idle_prompt`（"Claude is waiting for your input"）は**無操作が
            // 続いたことを知らせるタイマー**であって、ユーザに用があるわけではない。
            // Claude Code はこれを出すときにバックグラウンドの仕事を見ていないので、
            // サブエージェントが走っている間も飛んでくる。ここで判断待ちに倒すと
            // 「メインは終わったがサブエージェントは作業中」のセッションが
            // 琥珀色になり、ユーザは自分の番だと誤解する。
            //
            // 本当にユーザの操作が要る `permission_prompt` / `agent_needs_input` は
            // 対象外なので、サブエージェント実行中でもこれまでどおり判断待ちにする。
            if p.notification_type.as_deref() == Some(IDLE_PROMPT) && !s.agents.is_empty() {
                return Outcome::Ignore;
            }
            s.set_state(SessionState::WaitUser, now);
            Outcome::Upsert(s)
        }
        "SubagentStart" => Outcome::Upsert(reduce_subagent_start(&prev, &session_id, p, now)),
        "SubagentStop" => Outcome::Upsert(reduce_subagent_stop(&prev, &session_id, p, now)),
        "PostToolBatch" => {
            let mut s = base_session(&prev, &session_id, p, now);
            // ツールが動いた＝メインスレッドがターンの途中に居る。`UserPromptSubmit`
            // を伴わない再開（teammate からの差し戻しなど）でも `main_stopped` が
            // 立ちっぱなしにならないように、ここでも倒しておく。
            s.main_stopped = false;
            if s.state == SessionState::WaitUser {
                // 許可が下りてツールが実際に動いた合図なので Working に戻す。
                // 「ダイアログが出たあと許可しても、ターンが終わるまで琥珀色の
                // まま」だった問題の修正はここ。
                s.set_state(SessionState::Working, now);
            } else {
                // WaitAgent 等は据え置く（set_state を通すと since が動く）。
                s.updated = now;
            }
            Outcome::Upsert(s)
        }
        "Stop" => {
            let mut s = base_session(&prev, &session_id, p, now);
            s.main_stopped = true;
            let still_running = reconcile_agents(&mut s.agents, p.background_tasks.as_deref());
            if error {
                // transcript から拾ったエラー。API エラーは StopFailure で来るので
                // こちらは「Stop は来たが直近の assistant 行がエラー」の補助経路。
                // 種別は分からないので None。
                s.set_error(None, now);
            } else if still_running {
                // メインは終わったがサブエージェントは走り続けている。ここで完了に
                // すると、サブエージェントが黙々と働いている間ずっと「✓」が出る。
                s.set_state(SessionState::WaitAgent, now);
            } else {
                s.set_state(SessionState::Done, now);
            }
            Outcome::Upsert(s)
        }
        "StopFailure" => {
            let mut s = base_session(&prev, &session_id, p, now);
            s.main_stopped = true;
            // **`agents` は消さない。** この payload には `background_tasks` が無く
            // 生死が分からないので、消すと生きているサブエージェントを見失う
            // （lead が rate limit で落ちても teammate は走り続ける）。掃除は
            // `SubagentStop` と次のターンの `Stop` の突き合わせに任せる。
            s.set_error(non_empty(p.error.as_deref()), now);
            Outcome::Upsert(s)
        }
        "SessionEnd" => Outcome::Remove(session_id),
        _ => Outcome::Ignore,
    }
}

/// この payload がサブエージェント内から発火したものか。
///
/// 判定は `agent_id` の有無のみで行う（`agent_type` は `--agent` 起動の
/// メインスレッドにも付くため判定に使えない）。
fn fired_inside_subagent(p: &HookPayload) -> bool {
    non_empty(p.agent_id.as_deref()).is_some()
}

/// `notification_type` が「ユーザの判断・入力を待っている」ことを意味するか。
///
/// 判断待ちでないと**確定している**種別だけを明示的に除外し、それ以外
/// （未知の値・フィールド欠落）は判断待ちとして扱う。理由は 2 つ:
///
/// - payload のスキーマは `notification_type: string` で enum ではないため、
///   将来 Claude Code が新しい入力待ち通知を足しうる。見落として生き物が
///   固まるより、余計に光る方が害が小さい。
/// - `notification_type` を持たない古い Claude Code の payload が、従来どおり
///   判断待ちになる（後方互換）。
fn notification_means_wait_user(notification_type: Option<&str>) -> bool {
    !matches!(
        notification_type,
        // 認証が通っただけ。
        Some("auth_success")
            // MCP elicitation の完了通知（ダイアログは elicitation_dialog）。
            | Some("elicitation_complete")
            | Some("elicitation_response")
            // バックグラウンドエージェントの完了通知。
            | Some("agent_completed")
    )
}

/// `SessionStart` 以外の全イベント共通: 既存セッションがあればそれを土台にし、
/// 無ければ新規セッションを組み立てる。`cwd` は payload にあれば
/// `name`/`cwd` を導出し直し、無ければ既存値（新規なら `"?"`/空）を維持する。
///
/// `pid` は**ここでは決めない**（新規は `None`・既存は据え置き）。持ち主の
/// pid は stdin の JSON ではなく実行環境から取る事実なので、`reduce` を純関数の
/// まま保つために I/O 層（`ccsessions hook`）が保存直前に押す。
fn base_session(prev: &Option<Session>, session_id: &str, p: &HookPayload, now: u64) -> Session {
    match (&p.cwd, prev) {
        (Some(cwd), Some(prev)) => Session {
            cwd: cwd.clone(),
            name: Session::name_from_cwd(cwd),
            ..prev.clone()
        },
        (Some(cwd), None) => new_session(session_id, cwd, now),
        (None, Some(prev)) => prev.clone(),
        (None, None) => new_session(session_id, "", now),
    }
}

fn new_session(session_id: &str, cwd: &str, now: u64) -> Session {
    Session {
        id: session_id.to_string(),
        name: Session::name_from_cwd(cwd),
        // タイトルは transcript にしか無く、reducer は I/O を持たない。
        // 取ってくるのは hook 側（`ccsessions hook` が保存直前に押す）。
        title: None,
        cwd: cwd.to_string(),
        state: SessionState::Idle,
        since: now,
        updated: now,
        agents: Vec::new(),
        main_stopped: false,
        error_kind: None,
        // 持ち主の pid は reducer では決めない（`base_session` の doc 参照）。
        pid: None,
    }
}

fn reduce_subagent_start(
    prev: &Option<Session>,
    session_id: &str,
    p: &HookPayload,
    now: u64,
) -> Session {
    let mut s = base_session(prev, session_id, p, now);
    let id = non_empty(p.agent_id.as_deref()).unwrap_or_default();
    // 同じ agent_id での再発火では増やさない（id ベースなので冪等になる）。
    let already = !id.is_empty() && s.agents.iter().any(|a| a.id == id);
    if !already && s.agents.len() < MAX_AGENTS {
        s.agents.push(Agent {
            name: non_empty(p.agent_type.as_deref())
                .unwrap_or_else(|| FALLBACK_AGENT_NAME.to_string()),
            // description を持つ PreToolUse(Agent) と SubagentStart を突き合わせる
            // 手段が payload に無いので、hook 経由では常に空（`Agent::role` 参照）。
            role: String::new(),
            state: SessionState::Working,
            id,
        });
    }
    s.set_state(SessionState::WaitAgent, now);
    s
}

fn reduce_subagent_stop(
    prev: &Option<Session>,
    session_id: &str,
    p: &HookPayload,
    now: u64,
) -> Session {
    let mut s = base_session(prev, session_id, p, now);
    remove_agent_by_id(&mut s.agents, p.agent_id.as_deref());
    if s.agents.is_empty() && s.state == SessionState::WaitAgent {
        // 最後の 1 匹が終わった。**戻り先はメインスレッドの状況で決まる** —
        // ターンの途中なら lead が続きを喋るので `Working`、既に `Stop` を
        // 受けているなら残っていた仕事も片付いたので `Done`。
        let next = if s.main_stopped {
            SessionState::Done
        } else {
            SessionState::Working
        };
        s.set_state(next, now);
    } else {
        // 「WaitAgent 維持」および「WaitUser / Done を塗りつぶさない」。
        // state は変えず updated だけ進める（set_state で強制すると、万一 state が
        // 既に別値だった場合に since が意図せずリセットされてしまう）。
        s.updated = now;
    }
    s
}

/// `Stop` の `background_tasks` を正として `agents` を作り直し、**サブエージェントが
/// まだ走っているか**を返す。
///
/// hook で積み上げてきた `agents` は取りこぼしうる（`SubagentStart` が届かない・
/// `SubagentStop` が落ちる・中断で両方来ない）。`background_tasks` は Claude Code の
/// タスク台帳そのものなので、ここで突き合わせれば**残骸の除去と取りこぼしの回収が
/// 同時に**できる。id は `SubagentStart.agent_id` と同じ値。
///
/// `tasks` が `None`（フィールドごと無い ＝ 教えてくれない古い Claude Code）のときは
/// 判断材料が無いので、従来どおり全部消して「もう走っていない」とみなす。ここで
/// 逆に残す側へ倒すと、生死を確かめる術が無いまま `WaitAgent` に固まりうる。
fn reconcile_agents(agents: &mut Vec<Agent>, tasks: Option<&[BackgroundTask]>) -> bool {
    let tasks = match tasks {
        Some(t) => t,
        None => {
            agents.clear();
            return false;
        }
    };
    let mut next: Vec<Agent> = Vec::new();
    for t in tasks.iter().filter(|t| is_running_agent_task(t)) {
        let id = match non_empty(t.id.as_deref()) {
            Some(id) => id,
            // id が無ければ既存の agent と対応付けられない。押し込むと `Stop` の
            // たびに重複して増えるので、追跡できないものとして落とす。
            None => continue,
        };
        if next.len() >= MAX_AGENTS {
            break;
        }
        let known = agents.iter().find(|a| a.id == id);
        next.push(Agent {
            // 表示名は `SubagentStart.agent_type` で付いたものを優先する
            // （teammate はそこでしか名前が分からない。`background_tasks` の
            // `agent_type` は subagent にしか付かず、teammate では `type` の
            // "teammate" が精一杯）。
            name: known
                .map(|a| a.name.clone())
                .or_else(|| non_empty(t.agent_type.as_deref()))
                .or_else(|| non_empty(t.kind.as_deref()))
                .unwrap_or_else(|| FALLBACK_AGENT_NAME.to_string()),
            // 役割ラベルはここでしか手に入らない（`SubagentStart` は
            // `{agent_id, agent_type}` しか持たない ＝ ADR 0005 で「落ちる」と
            // 書いた `description` が、`Stop` を経由して戻ってくる）。
            role: non_empty(t.description.as_deref())
                .or_else(|| known.map(|a| a.role.clone()).filter(|r| !r.is_empty()))
                .unwrap_or_default(),
            state: known.map(|a| a.state).unwrap_or(SessionState::Working),
            id,
        });
    }
    *agents = next;
    !agents.is_empty()
}

/// この `background_tasks` の 1 件が「まだ走っているサブエージェント」か。
///
/// `status` は Claude Code 側で running / pending に絞ってあるが、値が増えたときに
/// **終わったものを走っていると誤認しない**よう、知っている 2 値だけを通す
/// （フィールドが無い場合は絞り込み済みの前提で通す）。
fn is_running_agent_task(t: &BackgroundTask) -> bool {
    let kind_is_agent = t
        .kind
        .as_deref()
        .is_some_and(|k| AGENT_TASK_KINDS.contains(&k));
    let running = match t.status.as_deref() {
        Some(s) => s == "running" || s == "pending",
        None => true,
    };
    kind_is_agent && running
}

/// `agent_id` が一致する agent を 1 件だけ除去する。**一致しなければ何もしない。**
///
/// 以前は「一致しなければ先頭を除去」というフォールバックがあった。しかし
/// `SubagentStop` の payload に `tool_use_id` は無いので照合は常に失敗し、実質
/// 「常に先頭を消す」= FIFO になっていた。並列でサブエージェントを走らせると
/// 「先に終わった方ではなく先頭が消える」ため、ホバーカードの内容が実態と
/// 食い違っていた。
///
/// ネストしたサブエージェントの `SubagentStop` も「対象の `agent_id`」を持って
/// 親セッションに届くので、一致しない stop で無関係な agent を消してはいけない。
fn remove_agent_by_id(agents: &mut Vec<Agent>, agent_id: Option<&str>) {
    let id = match non_empty(agent_id) {
        Some(id) => id,
        None => return,
    };
    if let Some(pos) = agents.iter().position(|a| a.id == id) {
        agents.remove(pos);
    }
}

/// 空文字を `None` に畳む。payload の文字列フィールドは「キーはあるが空」が
/// あり得るため、`Option` の有無だけでは判定できない。
fn non_empty(s: Option<&str>) -> Option<String> {
    s.filter(|v| !v.is_empty()).map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn payload(event: &str) -> HookPayload {
        HookPayload {
            session_id: Some("sess-1".into()),
            hook_event_name: Some(event.into()),
            cwd: Some("/Users/x/proj".into()),
            ..Default::default()
        }
    }

    fn existing(state: SessionState, since: u64, updated: u64, agents: Vec<Agent>) -> Session {
        Session {
            id: "sess-1".into(),
            name: "proj".into(),
            title: None,
            cwd: "/Users/x/proj".into(),
            state,
            since,
            updated,
            agents,
            main_stopped: false,
            error_kind: None,
            pid: Some(4242),
        }
    }

    fn agent(name: &str, id: &str) -> Agent {
        Agent {
            name: name.into(),
            role: String::new(),
            state: SessionState::Working,
            id: id.into(),
        }
    }

    /// `Stop` payload の `background_tasks` の 1 件（既定は「走っている subagent」）。
    fn task(id: &str, kind: &str, description: &str) -> BackgroundTask {
        BackgroundTask {
            id: Some(id.into()),
            kind: Some(kind.into()),
            status: Some("running".into()),
            description: Some(description.into()),
            agent_type: None,
        }
    }

    fn unwrap_upsert(o: Outcome) -> Session {
        match o {
            Outcome::Upsert(s) => s,
            Outcome::Remove(_) => panic!("expected Upsert, got Remove"),
            Outcome::Ignore => panic!("expected Upsert, got Ignore"),
        }
    }

    fn assert_ignored(o: Outcome) {
        match o {
            Outcome::Ignore => {}
            Outcome::Upsert(s) => panic!("expected Ignore, got Upsert({:?})", s.state),
            Outcome::Remove(_) => panic!("expected Ignore, got Remove"),
        }
    }

    // ---- session_id / event missing --------------------------------------------

    #[test]
    fn missing_session_id_is_ignored() {
        let mut p = payload("UserPromptSubmit");
        p.session_id = None;
        assert_ignored(reduce(None, &p, 1000, false));
    }

    #[test]
    fn empty_session_id_is_ignored() {
        let mut p = payload("UserPromptSubmit");
        p.session_id = Some("".into());
        assert_ignored(reduce(None, &p, 1000, false));
    }

    #[test]
    fn unknown_event_is_ignored() {
        let p = payload("SomeFutureEvent");
        assert_ignored(reduce(None, &p, 1000, false));
    }

    // ---- SessionStart -----------------------------------------------------------

    #[test]
    fn session_start_creates_new_session_idle() {
        let p = payload("SessionStart");
        let s = unwrap_upsert(reduce(None, &p, 1000, false));
        assert_eq!(s.state, SessionState::Idle);
        assert_eq!(s.since, 1000);
        assert_eq!(s.updated, 1000);
        assert!(s.agents.is_empty());
        assert_eq!(s.name, "proj");
        assert_eq!(s.error_kind, None);
    }

    #[test]
    fn session_start_existing_keeps_state_updates_updated() {
        let prev = existing(SessionState::Working, 500, 500, vec![]);
        let p = payload("SessionStart");
        let s = unwrap_upsert(reduce(Some(prev), &p, 2000, false));
        assert_eq!(s.state, SessionState::Working);
        assert_eq!(
            s.since, 500,
            "since must not change on SessionStart re-notify"
        );
        assert_eq!(s.updated, 2000);
    }

    // ---- UserPromptSubmit ---------------------------------------------------------

    #[test]
    fn user_prompt_submit_sets_working_no_prev() {
        let p = payload("UserPromptSubmit");
        let s = unwrap_upsert(reduce(None, &p, 1000, false));
        assert_eq!(s.state, SessionState::Working);
        assert_eq!(s.since, 1000);
    }

    #[test]
    fn user_prompt_submit_clears_leftover_agents() {
        // 新しいターンの開始で前ターンの残骸を掃除する。中断（ESC）では Stop も
        // StopFailure も来ないので、ここが唯一の掃除機会。
        let prev = existing(
            SessionState::WaitAgent,
            500,
            500,
            vec![agent("greta", "ag-1")],
        );
        let p = payload("UserPromptSubmit");
        let s = unwrap_upsert(reduce(Some(prev), &p, 2000, false));
        assert_eq!(s.state, SessionState::Working);
        assert!(
            s.agents.is_empty(),
            "leftover agents from the previous turn must be cleared"
        );
    }

    // ---- Notification ------------------------------------------------------------

    #[test]
    fn notification_without_type_sets_wait_user() {
        // notification_type を持たない古い payload は従来どおり判断待ち。
        let p = payload("Notification");
        let s = unwrap_upsert(reduce(None, &p, 1000, false));
        assert_eq!(s.state, SessionState::WaitUser);
    }

    #[test]
    fn notification_wait_user_types_set_wait_user() {
        for t in [
            "permission_prompt",
            "idle_prompt",
            "elicitation_dialog",
            "agent_needs_input",
        ] {
            let mut p = payload("Notification");
            p.notification_type = Some(t.into());
            let s = unwrap_upsert(reduce(None, &p, 1000, false));
            assert_eq!(s.state, SessionState::WaitUser, "{t} must mean wait_user");
        }
    }

    #[test]
    fn notification_completion_types_are_ignored() {
        for t in [
            "auth_success",
            "elicitation_complete",
            "elicitation_response",
            "agent_completed",
        ] {
            let prev = existing(SessionState::Working, 500, 500, vec![]);
            let mut p = payload("Notification");
            p.notification_type = Some(t.into());
            assert_ignored(reduce(Some(prev), &p, 2000, false));
        }
    }

    #[test]
    fn idle_prompt_leaves_a_session_whose_subagents_are_still_working_alone() {
        // 本題の後半。メインが終わったあと 60 秒黙っていると Claude Code は
        // 「Claude is waiting for your input」を出すが、これはバックグラウンドの
        // 仕事を見ていない。サブエージェントが走っている間は判断待ちにしない。
        let prev = existing(
            SessionState::WaitAgent,
            500,
            500,
            vec![agent("greta", "ag-1")],
        );
        let mut p = payload("Notification");
        p.notification_type = Some("idle_prompt".into());
        assert_ignored(reduce(Some(prev), &p, 2000, false));
    }

    #[test]
    fn idle_prompt_still_means_wait_user_once_no_subagent_is_left() {
        let prev = existing(SessionState::Done, 500, 500, vec![]);
        let mut p = payload("Notification");
        p.notification_type = Some("idle_prompt".into());
        let s = unwrap_upsert(reduce(Some(prev), &p, 2000, false));
        assert_eq!(s.state, SessionState::WaitUser);
    }

    #[test]
    fn a_prompt_that_really_needs_the_user_wins_over_running_subagents() {
        // 「サブエージェント実行中は判断待ちにしない」を広げすぎない。許可
        // ダイアログとエージェントからの問い合わせは、実際にユーザの操作が要る。
        for t in [
            "permission_prompt",
            "agent_needs_input",
            "elicitation_dialog",
        ] {
            let prev = existing(
                SessionState::WaitAgent,
                500,
                500,
                vec![agent("greta", "ag-1")],
            );
            let mut p = payload("Notification");
            p.notification_type = Some(t.into());
            let s = unwrap_upsert(reduce(Some(prev), &p, 2000, false));
            assert_eq!(s.state, SessionState::WaitUser, "{t} はユーザの操作が要る");
        }
    }

    #[test]
    fn unknown_notification_type_falls_back_to_wait_user() {
        // スキーマは enum ではなく string なので未知の値が来うる。判断待ちを
        // 見落とすより余計に光る方が害が小さい（fail-safe の向き）。
        let mut p = payload("Notification");
        p.notification_type = Some("some_future_notification".into());
        let s = unwrap_upsert(reduce(None, &p, 1000, false));
        assert_eq!(s.state, SessionState::WaitUser);
    }

    // ---- PermissionRequest ---------------------------------------------------------

    #[test]
    fn permission_request_sets_wait_user() {
        let p = payload("PermissionRequest");
        let s = unwrap_upsert(reduce(None, &p, 1000, false));
        assert_eq!(s.state, SessionState::WaitUser);
    }

    // ---- SubagentStart -------------------------------------------------------------

    #[test]
    fn subagent_start_pushes_agent_and_sets_wait_agent() {
        let mut p = payload("SubagentStart");
        p.agent_id = Some("ag-1".into());
        p.agent_type = Some("general-purpose".into());
        let s = unwrap_upsert(reduce(None, &p, 1000, false));
        assert_eq!(s.state, SessionState::WaitAgent);
        assert_eq!(s.agents.len(), 1);
        assert_eq!(s.agents[0].name, "general-purpose");
        assert_eq!(s.agents[0].id, "ag-1");
        assert_eq!(
            s.agents[0].role, "",
            "role cannot be recovered from SubagentStart"
        );
    }

    #[test]
    fn subagent_start_without_agent_type_falls_back_to_agent() {
        let mut p = payload("SubagentStart");
        p.agent_id = Some("ag-1".into());
        let s = unwrap_upsert(reduce(None, &p, 1000, false));
        assert_eq!(s.agents[0].name, "agent");
    }

    #[test]
    fn subagent_start_is_idempotent_for_the_same_agent_id() {
        // 他のツールが SubagentStart をブロックして再発火した場合でも増えない。
        let prev = existing(
            SessionState::WaitAgent,
            500,
            500,
            vec![agent("general-purpose", "ag-1")],
        );
        let mut p = payload("SubagentStart");
        p.agent_id = Some("ag-1".into());
        p.agent_type = Some("general-purpose".into());
        let s = unwrap_upsert(reduce(Some(prev), &p, 2000, false));
        assert_eq!(s.agents.len(), 1, "same agent_id must not be pushed twice");
    }

    #[test]
    fn subagent_start_tracks_distinct_ids_separately() {
        let prev = existing(SessionState::WaitAgent, 500, 500, vec![agent("a", "ag-1")]);
        let mut p = payload("SubagentStart");
        p.agent_id = Some("ag-2".into());
        p.agent_type = Some("b".into());
        let s = unwrap_upsert(reduce(Some(prev), &p, 2000, false));
        assert_eq!(s.agents.len(), 2);
    }

    #[test]
    fn agents_capped_at_32() {
        let agents: Vec<Agent> = (0..32).map(|i| agent("a", &format!("ag-{i}"))).collect();
        let prev = existing(SessionState::WaitAgent, 500, 500, agents);
        let mut p = payload("SubagentStart");
        p.agent_id = Some("overflow".into());
        let s = unwrap_upsert(reduce(Some(prev), &p, 1000, false));
        assert_eq!(s.agents.len(), 32, "must not exceed MAX_AGENTS");
    }

    // ---- SubagentStop ---------------------------------------------------------------

    #[test]
    fn subagent_stop_removes_the_matching_agent_id_not_the_first() {
        // agent_id 照合の回帰テスト。FIFO に戻ると先頭（greta）が消えてこれが落ちる。
        let prev = existing(
            SessionState::WaitAgent,
            500,
            500,
            vec![agent("greta", "ag-1"), agent("izzy", "ag-2")],
        );
        let mut p = payload("SubagentStop");
        p.agent_id = Some("ag-2".into());
        let s = unwrap_upsert(reduce(Some(prev), &p, 2000, false));
        assert_eq!(s.agents.len(), 1);
        assert_eq!(
            s.agents[0].name, "greta",
            "the agent whose id matched must be the one removed"
        );
    }

    #[test]
    fn subagent_stop_with_unknown_id_removes_nothing() {
        // ネストしたサブエージェントの stop や、旧形式のセッションファイルが
        // 残っている場合。無関係な agent を消してはいけない。
        let prev = existing(
            SessionState::WaitAgent,
            500,
            500,
            vec![agent("greta", "ag-1"), agent("izzy", "ag-2")],
        );
        let mut p = payload("SubagentStop");
        p.agent_id = Some("no-such-id".into());
        let s = unwrap_upsert(reduce(Some(prev), &p, 2000, false));
        assert_eq!(s.agents.len(), 2, "must not remove an unrelated agent");
        assert_eq!(s.state, SessionState::WaitAgent);
    }

    #[test]
    fn subagent_stop_emptying_agents_sets_working() {
        let prev = existing(
            SessionState::WaitAgent,
            500,
            500,
            vec![agent("greta", "ag-1")],
        );
        let mut p = payload("SubagentStop");
        p.agent_id = Some("ag-1".into());
        let s = unwrap_upsert(reduce(Some(prev), &p, 2000, false));
        assert!(s.agents.is_empty());
        assert_eq!(s.state, SessionState::Working);
    }

    #[test]
    fn the_last_subagent_stop_after_the_main_thread_stopped_means_done() {
        // メインが `Stop` を受けたあとまで走っていたサブエージェントが終わった。
        // lead はもう喋らないので、戻り先は「作業中」ではなく「完了」。
        let mut prev = existing(
            SessionState::WaitAgent,
            500,
            500,
            vec![agent("greta", "ag-1")],
        );
        prev.main_stopped = true;
        let mut p = payload("SubagentStop");
        p.agent_id = Some("ag-1".into());
        let s = unwrap_upsert(reduce(Some(prev), &p, 2000, false));
        assert!(s.agents.is_empty());
        assert_eq!(s.state, SessionState::Done);
    }

    #[test]
    fn subagent_stop_does_not_clobber_wait_user() {
        // エージェント実行中に許可ダイアログが出た場合。エージェントが終わっても
        // ユーザはまだ聞かれているので、判断待ちを Working で塗りつぶさない。
        let prev = existing(
            SessionState::WaitUser,
            500,
            500,
            vec![agent("greta", "ag-1")],
        );
        let mut p = payload("SubagentStop");
        p.agent_id = Some("ag-1".into());
        let s = unwrap_upsert(reduce(Some(prev), &p, 2000, false));
        assert!(s.agents.is_empty());
        assert_eq!(s.state, SessionState::WaitUser);
        assert_eq!(s.since, 500, "since must not move");
        assert_eq!(s.updated, 2000);
    }

    #[test]
    fn subagent_stop_remaining_agents_keeps_wait_agent() {
        let prev = existing(
            SessionState::WaitAgent,
            500,
            500,
            vec![agent("greta", "ag-1"), agent("izzy", "ag-2")],
        );
        let mut p = payload("SubagentStop");
        p.agent_id = Some("ag-1".into());
        let s = unwrap_upsert(reduce(Some(prev), &p, 2000, false));
        assert_eq!(s.state, SessionState::WaitAgent);
        assert_eq!(s.since, 500);
    }

    // ---- PostToolBatch ---------------------------------------------------------------

    #[test]
    fn post_tool_batch_reverts_wait_user_to_working() {
        // 判断待ちからの復帰の回帰テスト: 許可ダイアログが下りたあと琥珀色のまま固まらない。
        let prev = existing(SessionState::WaitUser, 500, 500, vec![]);
        let p = payload("PostToolBatch");
        let s = unwrap_upsert(reduce(Some(prev), &p, 2000, false));
        assert_eq!(s.state, SessionState::Working);
        assert_eq!(s.since, 2000);
    }

    #[test]
    fn post_tool_batch_leaves_wait_agent_untouched() {
        let prev = existing(
            SessionState::WaitAgent,
            500,
            500,
            vec![agent("greta", "ag-1")],
        );
        let p = payload("PostToolBatch");
        let s = unwrap_upsert(reduce(Some(prev), &p, 2000, false));
        assert_eq!(s.state, SessionState::WaitAgent);
        assert_eq!(s.since, 500, "since must not change; only updated advances");
        assert_eq!(s.updated, 2000);
        assert_eq!(s.agents.len(), 1);
    }

    #[test]
    fn post_tool_batch_leaves_working_since_alone() {
        let prev = existing(SessionState::Working, 500, 500, vec![]);
        let p = payload("PostToolBatch");
        let s = unwrap_upsert(reduce(Some(prev), &p, 2000, false));
        assert_eq!(s.state, SessionState::Working);
        assert_eq!(s.since, 500);
        assert_eq!(s.updated, 2000);
    }

    // ---- agent_id によるサブエージェント発イベントの遮断 ----------------------------

    #[test]
    fn event_fired_inside_a_subagent_is_ignored_entirely() {
        // サブエージェントが Bash を呼ぶと、その PostToolBatch が親の session_id で
        // 飛んでくる。親が判断待ちでもそれを塗りつぶしてはいけない。書き込み自体を
        // 行わない（並列 hook の lost update の窓を増やさないため）。
        let prev = existing(SessionState::WaitUser, 500, 500, vec![]);
        let mut p = payload("PostToolBatch");
        p.agent_id = Some("ag-1".into());
        assert_ignored(reduce(Some(prev), &p, 2000, false));
    }

    #[test]
    fn stop_fired_inside_a_subagent_does_not_finish_the_parent() {
        let prev = existing(SessionState::Working, 500, 500, vec![]);
        let mut p = payload("Stop");
        p.agent_id = Some("ag-1".into());
        assert_ignored(reduce(Some(prev), &p, 2000, false));
    }

    #[test]
    fn notification_never_carries_agent_id_so_the_filter_cannot_hide_it() {
        // Claude Code の Notification は toolUseContext 無しで組み立てられるため
        // `agent_id` が付かない（バイナリ 2.1.220: `{...Kf(void 0), ... "Notification"}`）。
        // つまりこのフィルタがサブエージェント発の「入力が必要」通知を
        // 握りつぶすことはない。仮に将来 agent_id が付くようになったら、
        // このテストではなく実際の挙動が変わるので、そのときに再考する。
        let mut p = payload("Notification");
        p.notification_type = Some("agent_needs_input".into());
        let s = unwrap_upsert(reduce(None, &p, 1000, false));
        assert_eq!(s.state, SessionState::WaitUser);
    }

    #[test]
    fn subagent_lifecycle_events_are_exempt_from_the_agent_id_filter() {
        // SubagentStart/Stop の agent_id は「対象」であって「発火元」ではないので、
        // agent_id が付いていても処理しなければならない。
        let mut p = payload("SubagentStart");
        p.agent_id = Some("ag-1".into());
        p.agent_type = Some("general-purpose".into());
        let s = unwrap_upsert(reduce(None, &p, 1000, false));
        assert_eq!(s.state, SessionState::WaitAgent);
        assert_eq!(s.agents.len(), 1);
    }

    #[test]
    fn agent_type_alone_does_not_mark_an_event_as_subagent_fired() {
        // `--agent` で起動したセッションのメインスレッドは agent_type を持つが
        // agent_id を持たない。判定は agent_id だけで行う。
        let prev = existing(SessionState::WaitUser, 500, 500, vec![]);
        let mut p = payload("PostToolBatch");
        p.agent_type = Some("general-purpose".into());
        let s = unwrap_upsert(reduce(Some(prev), &p, 2000, false));
        assert_eq!(
            s.state,
            SessionState::Working,
            "main thread must be handled"
        );
    }

    // ---- Stop / StopFailure -------------------------------------------------------------

    #[test]
    fn stop_clears_agents_and_sets_done() {
        let prev = existing(
            SessionState::WaitAgent,
            500,
            500,
            vec![agent("greta", "ag-1")],
        );
        let p = payload("Stop");
        let s = unwrap_upsert(reduce(Some(prev), &p, 2000, false));
        assert!(s.agents.is_empty(), "Stop must clear agents");
        assert_eq!(s.state, SessionState::Done);
        assert_eq!(s.error_kind, None);
    }

    #[test]
    fn stop_with_transcript_error_sets_error_without_a_kind() {
        let prev = existing(SessionState::Working, 500, 500, vec![]);
        let p = payload("Stop");
        let s = unwrap_upsert(reduce(Some(prev), &p, 2000, true));
        assert_eq!(s.state, SessionState::Error);
        assert_eq!(
            s.error_kind, None,
            "transcript detection cannot tell the API error kind"
        );
    }

    #[test]
    fn stop_failure_sets_error_with_kind() {
        // StopFailure の回帰テスト: API エラーで終わったターンは Stop が来ないので、
        // StopFailure を見なければ Working のまま固まる。
        // （`agents` を残す方の保証は
        // `stop_failure_keeps_the_subagents_that_the_lead_left_running`）
        let prev = existing(SessionState::WaitAgent, 500, 500, vec![]);
        let mut p = payload("StopFailure");
        p.error = Some("rate_limit".into());
        let s = unwrap_upsert(reduce(Some(prev), &p, 2000, false));
        assert_eq!(s.state, SessionState::Error);
        assert_eq!(s.error_kind.as_deref(), Some("rate_limit"));
        assert_eq!(s.since, 2000);
    }

    #[test]
    fn stop_failure_without_error_field_still_sets_error() {
        let p = payload("StopFailure");
        let s = unwrap_upsert(reduce(None, &p, 1000, false));
        assert_eq!(s.state, SessionState::Error);
        assert_eq!(s.error_kind, None);
    }

    #[test]
    fn error_kind_is_cleared_when_leaving_the_error_state() {
        // 一度 rate limit で落ちたセッションが次のターンで成功したときに
        // "rate limit" が残らないこと（error_kind は Error のときだけ意味を持つ）。
        let mut prev = existing(SessionState::Error, 500, 500, vec![]);
        prev.error_kind = Some("rate_limit".into());
        let p = payload("UserPromptSubmit");
        let s = unwrap_upsert(reduce(Some(prev), &p, 2000, false));
        assert_eq!(s.state, SessionState::Working);
        assert_eq!(s.error_kind, None, "stale error kind must not survive");
    }

    #[test]
    fn error_kind_survives_a_state_neutral_event() {
        // PostToolBatch は Error を触らないので種別も残る。
        let mut prev = existing(SessionState::Error, 500, 500, vec![]);
        prev.error_kind = Some("overloaded".into());
        let p = payload("PostToolBatch");
        let s = unwrap_upsert(reduce(Some(prev), &p, 2000, false));
        assert_eq!(s.state, SessionState::Error);
        assert_eq!(s.error_kind.as_deref(), Some("overloaded"));
    }

    // ---- Stop × background_tasks（メインが終わってもサブエージェントは走っている）----

    #[test]
    fn stop_keeps_waiting_while_a_subagent_is_still_running() {
        // 本題の前半。lead のターンが終わっただけで teammate は走り続けているのに、
        // 以前はここで agents を消して「完了」にしていた（そのあと idle_prompt で
        // 「判断待ち」になる）。
        let prev = existing(
            SessionState::WaitAgent,
            500,
            500,
            vec![agent("greta", "ag-1")],
        );
        let mut p = payload("Stop");
        p.background_tasks = Some(vec![task("ag-1", "teammate", "CGWindowList の接地")]);
        let s = unwrap_upsert(reduce(Some(prev), &p, 2000, false));
        assert_eq!(s.state, SessionState::WaitAgent);
        assert_eq!(s.agents.len(), 1);
        assert_eq!(s.agents[0].name, "greta", "起動時に付いた名前を保つ");
        assert!(s.main_stopped, "メインのターンは終わっている");
    }

    #[test]
    fn stop_drops_the_agents_that_are_no_longer_in_the_task_list() {
        // 突き合わせは残骸の掃除も兼ねる（`SubagentStop` を取りこぼしても、次の
        // `Stop` で必ず実態に戻る）。
        let prev = existing(
            SessionState::WaitAgent,
            500,
            500,
            vec![agent("greta", "ag-1"), agent("izzy", "ag-2")],
        );
        let mut p = payload("Stop");
        p.background_tasks = Some(vec![task("ag-2", "subagent", "調査")]);
        let s = unwrap_upsert(reduce(Some(prev), &p, 2000, false));
        assert_eq!(s.agents.len(), 1);
        assert_eq!(s.agents[0].name, "izzy");
        assert_eq!(s.state, SessionState::WaitAgent);
    }

    #[test]
    fn stop_adopts_a_subagent_whose_start_was_never_seen() {
        // 逆向きの回収。`SubagentStart` が届かなかった（hook が落ちた・古い
        // セッションファイル）場合でも、台帳に載っていれば拾い直す。
        let prev = existing(SessionState::Working, 500, 500, vec![]);
        let mut p = payload("Stop");
        let mut t = task("ag-9", "subagent", "probe agent");
        t.agent_type = Some("general-purpose".into());
        p.background_tasks = Some(vec![t]);
        let s = unwrap_upsert(reduce(Some(prev), &p, 2000, false));
        assert_eq!(s.state, SessionState::WaitAgent);
        assert_eq!(s.agents.len(), 1);
        assert_eq!(s.agents[0].name, "general-purpose");
        assert_eq!(s.agents[0].id, "ag-9");
    }

    #[test]
    fn stop_recovers_the_role_label_that_subagent_start_cannot_carry() {
        // `SubagentStart` は `{agent_id, agent_type}` しか持たないので役割ラベルが
        // 空のままだった（ADR 0005）。`background_tasks` の `description` がそれ。
        let prev = existing(
            SessionState::WaitAgent,
            500,
            500,
            vec![agent("greta", "ag-1")],
        );
        assert_eq!(prev.agents[0].role, "");
        let mut p = payload("Stop");
        p.background_tasks = Some(vec![task("ag-1", "teammate", "CGWindowList の接地")]);
        let s = unwrap_upsert(reduce(Some(prev), &p, 2000, false));
        assert_eq!(s.agents[0].role, "CGWindowList の接地");
    }

    #[test]
    fn background_shell_work_does_not_keep_the_session_out_of_done() {
        // `sleep` を 1 つバックグラウンドに投げただけでターンが完了しなくなる、
        // という壊れ方を防ぐ。数えるのはサブエージェント系だけ。
        let prev = existing(SessionState::Working, 500, 500, vec![]);
        let mut p = payload("Stop");
        p.background_tasks = Some(vec![
            task("sh-1", "shell", "sleep 300"),
            task("mon-1", "monitor", "watch the log"),
        ]);
        let s = unwrap_upsert(reduce(Some(prev), &p, 2000, false));
        assert_eq!(s.state, SessionState::Done);
        assert!(s.agents.is_empty());
    }

    #[test]
    fn a_task_that_is_no_longer_running_is_not_counted() {
        // Claude Code 側で running/pending に絞られているが、値が増えたときに
        // 終わったものを走っていると誤認しないこと。
        let prev = existing(SessionState::Working, 500, 500, vec![]);
        let mut p = payload("Stop");
        let mut t = task("ag-1", "subagent", "終わったやつ");
        t.status = Some("completed".into());
        p.background_tasks = Some(vec![t]);
        let s = unwrap_upsert(reduce(Some(prev), &p, 2000, false));
        assert_eq!(s.state, SessionState::Done);
        assert!(s.agents.is_empty());
    }

    #[test]
    fn a_stop_that_lists_nothing_is_plain_done() {
        let prev = existing(
            SessionState::WaitAgent,
            500,
            500,
            vec![agent("greta", "ag-1")],
        );
        let mut p = payload("Stop");
        p.background_tasks = Some(vec![]);
        let s = unwrap_upsert(reduce(Some(prev), &p, 2000, false));
        assert_eq!(s.state, SessionState::Done);
        assert!(s.agents.is_empty());
    }

    #[test]
    fn a_stop_without_the_field_at_all_behaves_like_before() {
        // `background_tasks` を持たない古い Claude Code。生死の判断材料が無いので
        // 従来どおり全部消して完了にする（確かめられないまま待ち続けない）。
        let prev = existing(
            SessionState::WaitAgent,
            500,
            500,
            vec![agent("greta", "ag-1")],
        );
        let p = payload("Stop");
        assert!(p.background_tasks.is_none());
        let s = unwrap_upsert(reduce(Some(prev), &p, 2000, false));
        assert_eq!(s.state, SessionState::Done);
        assert!(s.agents.is_empty());
    }

    #[test]
    fn a_task_without_an_id_cannot_be_tracked_and_is_skipped() {
        // id が無いと既存の agent と対応付けられず、`Stop` のたびに重複して増える。
        let prev = existing(SessionState::Working, 500, 500, vec![]);
        let mut p = payload("Stop");
        let mut t = task("x", "subagent", "id なし");
        t.id = None;
        p.background_tasks = Some(vec![t]);
        let s = unwrap_upsert(reduce(Some(prev), &p, 2000, false));
        assert!(s.agents.is_empty());
        assert_eq!(s.state, SessionState::Done);
    }

    #[test]
    fn reconciled_agents_are_capped_at_32() {
        let prev = existing(SessionState::Working, 500, 500, vec![]);
        let mut p = payload("Stop");
        p.background_tasks = Some(
            (0..40)
                .map(|i| task(&format!("ag-{i}"), "subagent", "x"))
                .collect(),
        );
        let s = unwrap_upsert(reduce(Some(prev), &p, 2000, false));
        assert_eq!(s.agents.len(), MAX_AGENTS);
    }

    #[test]
    fn stop_failure_keeps_the_subagents_that_the_lead_left_running() {
        // `StopFailure` の payload に `background_tasks` は無い。生死が分からない
        // ものを消すと、rate limit で lead が落ちただけで teammate を見失う。
        let prev = existing(
            SessionState::WaitAgent,
            500,
            500,
            vec![agent("greta", "ag-1")],
        );
        let mut p = payload("StopFailure");
        p.error = Some("rate_limit".into());
        let s = unwrap_upsert(reduce(Some(prev), &p, 2000, false));
        assert_eq!(s.state, SessionState::Error);
        assert_eq!(s.agents.len(), 1, "生死不明の agent を消さない");
        assert!(s.main_stopped);
    }

    #[test]
    fn a_new_prompt_puts_the_main_thread_back_in_its_turn() {
        let mut prev = existing(SessionState::Done, 500, 500, vec![]);
        prev.main_stopped = true;
        let s = unwrap_upsert(reduce(
            Some(prev),
            &payload("UserPromptSubmit"),
            2000,
            false,
        ));
        assert!(!s.main_stopped);
    }

    #[test]
    fn a_tool_running_puts_the_main_thread_back_in_its_turn() {
        // `UserPromptSubmit` を伴わない再開でも立ちっぱなしにならないこと。
        let mut prev = existing(SessionState::WaitAgent, 500, 500, vec![]);
        prev.main_stopped = true;
        let s = unwrap_upsert(reduce(Some(prev), &payload("PostToolBatch"), 2000, false));
        assert!(!s.main_stopped);
    }

    /// Claude Code 2.1.220 が実際に送ってきた `Stop` payload（収録したもの）。
    /// **ただし id とパスは伏せてある** — 収録時の session_id / prompt_id / cwd は
    /// 同じ形の別の値に置き換えた（この payload で固定したいのは形であって、
    /// 実行ごとに変わる値ではない）。`background_tasks` の構造は実物のまま。
    ///
    /// 同じ実行の `SubagentStart` は `"agent_id":"a6b95b3843cb27b86"` で、
    /// **`background_tasks[].id` と一致する**。突き合わせが成り立つ根拠がこれ。
    #[test]
    fn a_recorded_stop_payload_is_understood_end_to_end() {
        let raw = r#"{"session_id":"1f7c9a04-3b62-4d18-9e55-6a0c2d47f8b1",
          "transcript_path":"/Users/x/.claude/projects/p/1f7c9a04.jsonl",
          "cwd":"/Users/x/proj","prompt_id":"8d3e5b21-0c74-4af9-b6d2-19e7c4a05f36",
          "permission_mode":"default","hook_event_name":"Stop","stop_hook_active":false,
          "last_assistant_message":"DONE",
          "background_tasks":[{"id":"a6b95b3843cb27b86","type":"subagent","status":"running",
            "description":"probe agent","agent_type":"general-purpose"}],
          "session_crons":[]}"#;
        let p: HookPayload = serde_json::from_str(raw).unwrap();

        let s = unwrap_upsert(reduce(None, &p, 2000, false));
        assert_eq!(s.state, SessionState::WaitAgent);
        assert_eq!(s.agents.len(), 1);
        assert_eq!(s.agents[0].id, "a6b95b3843cb27b86");
        assert_eq!(s.agents[0].name, "general-purpose");
        assert_eq!(s.agents[0].role, "probe agent");
        assert!(s.main_stopped);
    }

    // ---- SessionEnd -------------------------------------------------------------------

    #[test]
    fn session_end_removes_session() {
        let p = payload("SessionEnd");
        match reduce(None, &p, 1000, false) {
            Outcome::Remove(id) => assert_eq!(id, "sess-1"),
            _ => panic!("expected Remove"),
        }
    }

    // ---- pid は reducer では触らない -----------------------------------------------

    /// 持ち主の pid は実行環境から取る事実なので、reducer は**既存値を素通し**
    /// させるだけでよい（保存直前に `ccsessions hook` が現在の親 pid を押す）。
    /// ここで勝手に消すと、押し直される前の一瞬だけ持ち主不明のセッションが
    /// ファイルに現れることになる。
    #[test]
    fn reduce_carries_the_existing_pid_through() {
        let prev = existing(SessionState::Working, 500, 500, vec![]);
        assert_eq!(prev.pid, Some(4242));
        for event in ["SessionStart", "UserPromptSubmit", "Notification", "Stop"] {
            let s = unwrap_upsert(reduce(Some(prev.clone()), &payload(event), 2000, false));
            assert_eq!(s.pid, Some(4242), "{event} が pid を落としている");
        }
    }

    #[test]
    fn a_brand_new_session_has_no_pid_yet() {
        let s = unwrap_upsert(reduce(None, &payload("SessionStart"), 1000, false));
        assert_eq!(s.pid, None);
    }

    // ---- cwd fallback when absent -----------------------------------------------------

    #[test]
    fn missing_cwd_keeps_existing_name_and_cwd() {
        let prev = existing(SessionState::Working, 500, 500, vec![]);
        let mut p = payload("UserPromptSubmit");
        p.cwd = None;
        let s = unwrap_upsert(reduce(Some(prev), &p, 2000, false));
        assert_eq!(s.name, "proj");
        assert_eq!(s.cwd, "/Users/x/proj");
    }
}
