# 仕組み

```
Claude Code hook ──exec──> ccsessions hook ──atomic write──> ~/.local/state/ccsessions/sessions/<id>.json
                                                                      │ poll 500ms
ccsessionsd ←───────────────────────────────────────────────────────────┘
   └─ Vec<Session> → 差分更新 → CALayer（アニメは CoreAnimation が自走）
```

## なぜ hook か（トランスクリプトの追跡ではなく）

「判断待ち」「エージェント待ち」はトランスクリプトからは事後的にしか読めない。
hook は `Notification` / `PermissionRequest` / `SubagentStart` / `SubagentStop` /
`Stop` / `StopFailure` などが**その瞬間**に発火するので、表示の遅れがポーリング間隔
だけで済む。

例外は**セッションタイトル**（ホバーカードの 2 行目）だけ。これは hook payload に
入っておらず transcript の中にしかないので、ターンの区切りで末尾だけを読む
（[`adr/0023`](adr/0023-session-title-from-transcript.md)）。状態はここに依存しない。

購読しているのは次の 10 イベント（すべて matcher 無し・`timeout` 明示）。

| イベント | 用途 |
|---|---|
| `SessionStart` | セッションの出現 |
| `UserPromptSubmit` | 作業中へ・前ターンの残骸を掃除 |
| `PermissionRequest` | 判断待ち |
| `Notification` | `notification_type` を見て判断待ち／無視を分ける |
| `SubagentStart` | サブエージェントを `agent_id` で追跡し始める |
| `SubagentStop` | 同じ `agent_id` の 1 件だけを外す |
| `PostToolBatch` | 判断待ちからの復帰（ツールが実際に動いた合図） |
| `Stop` | メインスレッドのターンの終わり。`background_tasks` と突き合わせる |
| `StopFailure` | API エラーで終わったターン（種別つき） |
| `SessionEnd` | セッションの消滅 |

ツール 1 回ごとに発火する `PostToolUse` / `PreToolUse` は使わない（プロセス起動コストを
避ける）。`PostToolBatch` は**モデル往復ごとに 1 回**なので、必要な情報を桁違いに少ない
発火数で取れる。

**`Stop` はターンの終わりであって、サブエージェントの終わりではない。** lead が喋り
終えてもバックグラウンドの teammate は走り続けるので、`Stop` が運んでくる
`background_tasks`（＝ Claude Code のタスク台帳。id はサブエージェントの `agent_id` と
同じ）と突き合わせて、まだ走っているものだけを残す
（[`adr/0024`](adr/0024-stop-is-not-the-end-of-subagents.md)）。

hook が守る契約（exit 0・stdout に書かない・timeout）は
[`invariants.md`](invariants.md#hook) にある。**表示ツールがユーザの権限判断や
プロンプトを壊さないための安全装置**なので、イベントを足すときは必ず読むこと。

## なぜ CALayer 直描きか（WebView に HTML を載せるのではなく）

アニメーションは CoreAnimation がレンダーサーバ側で無限ループするので、daemon 本体は
眠っていられる。実測でアイドル時 CPU 0.0–0.1%。ログイン中ずっと生きているものなので、
ここは譲れない。

元のデザインは HTML/CSS なので、CSS の角丸プロファイルやアニメは Rust の定数・純関数へ
写経してある（`ccsessionsd/src/theme.rs` と `ccsessions-core/src/face/`）。

## ウィンドウが 2 枚ある理由

生き物の窓はホバーを取るためマウスイベントを受けるので、**帯のサイズぴったり**に作る
（窓は自分の矩形ぶんだけメニューバーのクリックを奪う）。ホバーカードは帯の外へはみ出す
ので、クリック透過の別窓に出す。

## メニューバー高への適応

bar 配置の群れはメニューバーの中に収まらなければならない。使える高さは機種依存で、
**ノッチ機 33pt / 非ノッチ画面（外部モニタ・MacBook Air・旧機種）24pt**。しかも
`NSScreen::mainScreen` はフォーカス追従なので、外部モニタにフォーカスを移すだけで
後者に切り替わる。

`layout::bar_fit` が 3 段階で吸収する（33pt では状態記号を体の右上に浮かせ、24pt では
体に重ね、それ以下では体も縮める）。どちらでも上端で切れず、動きも同じだけ出る。
メニューバーを自動的に隠す設定で高さが測れない場合は 24pt を仮定する（大きく見積もると
帯がメニューバーの外に残り、その範囲のクリックを奪うため）。

セッションが増えて幅が足りなくなったときは**群れ全体を一様に縮める**（7 匹目から。
下限 0.55 倍）。縮小の一様性と、ノッチの避け方は [`invariants.md`](invariants.md) と
[`adr/0013-uniform-squeeze.md`](adr/0013-uniform-squeeze.md) /
[`adr/0012-notch-avoidance.md`](adr/0012-notch-avoidance.md)。

## 構成

3 crate の Rust workspace。**「macOS FFI を触るコード」と「純粋なロジック」を
crate / モジュール境界で分離する**のが全体の設計原理。

| crate | 役割 |
|---|---|
| `ccsessions-core` | macOS 非依存・全部テスト可能。状態モデル・hook reducer・ストア・設定・顔の定義 |
| `ccsessions` | CLI 兼 hook producer。`settings.json` の追記マージ、設定と顔作りの Web UI |
| `ccsessionsd` | 常駐オーバーレイ。CALayer の構築と AppKit。**設定は読むだけ** |

より細かい内訳は [`../CLAUDE.md`](../CLAUDE.md)、決定の理由は [`adr/`](adr/README.md)。
