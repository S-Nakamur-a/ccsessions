# 0005 · 購読する hook イベントと、その状態遷移

採用 · 2026-07-26

## 文脈

初版は hook が 8 種だという前提で書かれていたが、Claude Code 2.1.220 には
**31 種**あり、そのうち 4 つは初版が抱えていた不正確さを直接解消するものだった。
実際に次の 3 つが本番で壊れていた。

- **エラー状態にほぼ到達できない** — API エラーで終わったターンは `Stop` ではなく
  **`StopFailure` が代わりに**飛ぶ。`Stop` だけを見ていたので、rate limit で落ちた
  セッションは作業中のまま TTL まで固まっていた。
- **サブエージェントの対応付けが実質 FIFO** — `SubagentStop` の payload に
  `tool_use_id` は**存在しない**。照合が常に失敗し「先頭を消す」フォールバックに
  落ちていたので、並列実行時にホバーカードの一覧が実態と食い違っていた。
- **判断待ちから戻れない** — 復帰を `PreToolUse` に頼っていたが、matcher で
  `Task` に絞ってあったため Bash や Edit では届かなかった。一度権限ダイアログが
  出ると、許可したあともターンが終わるまで琥珀色のままだった。

## 決定

matcher 無しで次の 10 種だけを購読する（`ccsessions/src/settings_json.rs::SIMPLE_EVENTS`）。

| イベント | 遷移 |
|---|---|
| `SessionStart` | セッション作成（state 据え置き） |
| `UserPromptSubmit` | `Working`、`agents` を空にする |
| `PermissionRequest` | `WaitUser` |
| `Notification` | `notification_type` で分岐（下記） |
| `SubagentStart` | `agents` へ `agent_id` で push、`WaitAgent` |
| `SubagentStop` | `agent_id` 一致で除去。空になったら `Working` |
| `PostToolBatch` | `WaitUser` なら `Working` へ復帰。他は `updated` だけ |
| `Stop` | `Done`、`agents` を空にする |
| `StopFailure` | `Error`（`error` 種別を保存）、`agents` を空にする |
| `SessionEnd` | セッションファイルを削除 |

**このうち `Stop` / `StopFailure` / `SubagentStop` / `Notification` の 4 行は
[0024](0024-stop-is-not-the-end-of-subagents.md) が上書きしている**（`Stop` は
サブエージェントの終わりを意味しないので `agents` を消さない）。購読するイベントの
一覧は変わっていない。

付随する決定:

- **エラー検出は `StopFailure` に一本化する。** transcript の tail 読みは
  「`Stop` は来たが直近の assistant 行がエラー」という未実証のケースの補助手段へ
  降格し、既定 `detect_errors = false`。
- **サブエージェントの追跡は `SubagentStart` / `SubagentStop` の `agent_id` で行う。**
  id ベースなので冪等で、ネストや並列でも壊れない。`PreToolUse(Agent|Task)` は
  購読しない — 併存させると 1 回の起動で `agents` を二重に push する。
  副作用としてホバーカードの役割ラベル（Agent ツールの `description`）は落ちる。
  `PreToolUse.tool_use_id` と `SubagentStart.agent_id` を突き合わせる手段が
  payload に無いため。（この副作用は [0024](0024-stop-is-not-the-end-of-subagents.md)
  で解消した — `Stop` の `background_tasks[].description` が同じものを運んでくる）
- **判断待ちからの復帰は `PostToolBatch`。** 「バッチ内の全ツールが解決した後、
  次のモデル呼び出し前に 1 回」なので、発火回数はモデル往復数と同じ。`PostToolUse`
  を matcher 無しで購読するより桁で少なく、[0001](0001-hook-as-the-state-source.md) の
  「高頻度イベントを避ける」と両立する。
- **`notification_type` の分岐は payload 側で行う**（install の matcher で絞らない）。
  matcher で絞ると、既に導入済みユーザの `settings.json` を書き換えないと挙動を
  直せなくなる。matcher は「頻度を減らす」目的にだけ使う。判断待ちでないと
  確定している `auth_success` / `elicitation_complete` / `elicitation_response` /
  `agent_completed` だけを除外し、**未知の値は判断待ち側に倒す**。
- **`agent_id` が付いたイベントは親セッションの状態を変えない。** `agent_id` は
  「サブエージェント内から発火した」ことを意味する共通フィールド。例外は
  `SubagentStart` / `SubagentStop` で、そこでの `agent_id` は「発火元」ではなく
  **対象のエージェント**を指す。

## 影響

- matcher は使わない。**matcher で絞るとそのイベントの他の payload が一切届かない**。
- イベントを足すときは [0004](0004-hook-never-blocks.md) と
  [0006](0006-hook-timeout.md) を必ず読むこと。
- 既に導入済みのユーザには `ccsessions doctor` が「hook の構成が古い」と知らせ、
  プラグインの更新で届く（どのファイルに入っているかは doctor が
  出す。書き込み先は推測しない ―― [0007](0007-settings-json-merge-only.md)）。
