# 0024 · `Stop` はサブエージェントの終わりを意味しない

採用 · 2026-07-28

[0005](0005-subscribed-hook-events.md) の続き。あちらの「`Stop` → `Done`、`agents` を
空にする」という遷移が、**サブエージェントが `Stop` より長生きする**という事実を
見落としていたので、そこだけ差し替える。購読するイベントの一覧は変えない。

## 文脈

lead（メインスレッド）がターンを終えても、バックグラウンドのサブエージェントや
teammate は走り続ける。ところが `Stop` を「全部終わった」と読んでいたため、実測で
次のように壊れていた。

実測（2026-07-28、teammate 1 匹が走っているセッション）:

| 経過 | 出来事 | ストアの状態 |
|---|---|---|
| T+0 | `SubagentStart(<agent_id>)` | `wait_agent` / agents=[teammate] ✓ |
| 数分後 | lead のターンが終わる → `Stop` | **`done` / agents=[]** ✗ |
| T+6 分 | `Notification{idle_prompt}` | **`wait_user`** ✗ |
| T+12 分 | teammate が再開され `SubagentStart` が再発火 | `wait_agent` / agents=[teammate] ✓ |

teammate は T+0 から**一度も止まらず**（`subagents/agent-<agent_id>.jsonl` が更新され
続けている）走っていた。「完了」と「判断待ち」がどちらも出て、しかも**再開のたびに
正しい表示へ戻る**ため、「たまにおかしい」という形で現れる。

判断待ちの方は Claude Code の無操作タイマー（`notification_type: "idle_prompt"`、
"Claude is waiting for your input"）で、実装を読むとこの通知は**バックグラウンドの
仕事を見ていない**。メインスレッドが手空きなだけで、ユーザに用があるわけではない。

材料は payload にあった。**`Stop` の payload は `background_tasks` を運んでくる。**
実際に収録したもの（Claude Code 2.1.220）:

```json
"background_tasks":[{"id":"a6b95b3843cb27b86","type":"subagent","status":"running",
                     "description":"probe agent","agent_type":"general-purpose"}]
```

同じ実行の `SubagentStart` は `"agent_id":"a6b95b3843cb27b86"` で、**`id` が一致する**
（Claude Code のタスク台帳が agent id をキーにしているため）。`type` は
`subagent` / `teammate` / `shell` / `workflow` / `monitor` / `MCP task` /
`cloud session` など。Claude Code 側で status が running / pending のものに
絞られている。

## 決定

1. **`Stop` では `agents` を消さず、`background_tasks` と突き合わせて作り直す。**
   残ればセッションは `Done` ではなく `WaitAgent`。
2. 数えるのは `type` が **`subagent` / `teammate`** のものだけ。
3. **`Session` に `main_stopped: bool` を持つ**（`Stop`/`StopFailure` で `true`、
   `UserPromptSubmit`/`PostToolBatch` で `false`）。最後の `SubagentStop` で
   `agents` が空になったときの戻り先を、これで `Working` と `Done` に振り分ける。
4. **`Notification{idle_prompt}` は `agents` が居るあいだ無視する。**
   `permission_prompt` / `agent_needs_input` / `elicitation_dialog` は従来どおり
   判断待ちにする。
5. **`StopFailure` でも `agents` を消さない**（この payload に `background_tasks` は
   無い ＝ 生死が分からない）。状態は `Error` のまま。
6. 突き合わせのついでに、`background_tasks[].description` を**役割ラベル**として
   `Agent::role` に入れる。0005 が「落ちる」と書いた `PreToolUse(Agent)` の
   `description` が、この経路で戻ってくる。

## 理由

- **`agents` が空でないことだけを根拠に待つ案では足りない。** `SubagentStart` の
  取りこぼしが直せず、逆に `SubagentStop` を落とすと残骸のまま `WaitAgent` に
  固まる。`background_tasks` は Claude Code のタスク台帳そのものなので、
  **残骸の除去と取りこぼしの回収が同時にできる**。id が `agent_id` と同じ値で
  あることが効いている。
- **`None`（フィールドごと無い）と `Some(空)` を区別する。** 前者は「教えてくれない
  古い Claude Code」なので、判断材料が無いまま待ち続けず従来どおり全部消して完了に
  倒す。確かめる術が無い状態で `WaitAgent` に固まる方が復帰しにくい。
- **`shell` を数えない。** バックグラウンドの `sleep` を 1 つ投げただけでターンが
  完了しなくなる。しかもそれらはサブエージェントではないので、ホバーカードに
  並べる相手でもない。`monitor` / `MCP task` も同じ理由。
- **`main_stopped` は 1 ビット足す価値がある。** 無いと、`Stop` のあとまで残っていた
  最後のサブエージェントが終わったときに `Working`（作業中）へ戻ってしまう。
  lead はもう喋らないので、これは次の通知が来るまで消えない嘘になる。
- **`idle_prompt` を判断待ちに数え続ける案は採らない。** ユーザ側から見て、
  サブエージェントが走っている間は「自分の番」ではない。0005 の「未知の
  `notification_type` は判断待ちに倒す」（見落とすより余計に光る方がまし）は
  変えない — ここで外すのは**意味が確定している 1 種類だけ**。
- **`TaskCompleted` / `TeammateIdle` を新たに購読する案は採らない。** 購読イベントを
  増やすと [0004](0004-hook-never-blocks.md) の事故面が広がる。既に購読している
  `Stop` の payload で足りる。

## 影響

- **セッションファイルに `main_stopped` が増える。** `#[serde(default)]` なので、
  このフィールドを持たない旧バージョンのファイルは `false`（ターン進行中）として
  読める。逆に新しいファイルを旧 daemon が読んでも未知フィールドとして無視される。
- 0005 の遷移表のうち `Stop` / `StopFailure` / `SubagentStop` / `Notification` の
  4 行は、この ADR が上書きする（0005 は履歴として残す）。
- **番人**（`event.rs` のテスト）:
  `stop_keeps_waiting_while_a_subagent_is_still_running` /
  `stop_drops_the_agents_that_are_no_longer_in_the_task_list` /
  `the_last_subagent_stop_after_the_main_thread_stopped_means_done` /
  `idle_prompt_leaves_a_session_whose_subagents_are_still_working_alone` /
  `background_shell_work_does_not_keep_the_session_out_of_done`。
  payload の形そのものは `a_recorded_stop_payload_is_understood_end_to_end` が
  **収録した実物の JSON** で固定している（想像で書いた payload では、今回のように
  「実際には別の形だった」を検出できない）。
- **`UserPromptSubmit` の `agents.clear()` は残す。** そのイベントには生死の情報が
  無い。teammate が走っている最中に次のプロンプトを打つとバッジが一瞬 0 になるが、
  状態は `Working`（lead が動いている）で正しく、次の `Stop` の突き合わせで実態に
  戻る。
