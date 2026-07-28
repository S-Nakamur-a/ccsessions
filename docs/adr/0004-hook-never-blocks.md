# 0004 · `ccsessions hook` は常に exit 0・stdout に何も書かない

採用 · 2026-07-25

## 文脈

Claude Code は hook の終了コードと標準出力を**イベントごとに違う意味で**解釈する。
Claude Code 2.1.220 の実装で確認した意味は次のとおり。

| 挙動 | 影響 |
|---|---|
| `PreToolUse` で exit 2 | ツールをブロックする |
| `PermissionRequest` で exit 2 | **権限を拒否する** |
| `UserPromptSubmit` で exit 2 | **ユーザのプロンプトを消去する** |
| `PostToolBatch` で exit 2 | エージェントのループを停止する |
| `UserPromptSubmit` / `SessionStart` で exit 0 | **stdout がそのまま Claude に渡る** |
| exit 1 | ブロックしない（Unix の慣習と逆） |

## 決定

`ccsessions hook` は**何があっても exit 0 で終わり、stdout に 1 バイトも書かない**。
失敗はすべて stderr の 1 行に留める。

## 理由

行儀の問題ではなく安全装置。破ると、表示ツールでしかないものがユーザの権限判断や
プロンプトを壊す。しかも**購読するイベントを増やすたびに被害面が広がる**ので、
イベントを足すときにこそ効く不変条件になる。

Rust の panic は exit 101 なので、パニックしてもブロックにはならない（安全側）。

## 影響

- 番人は `ccsessions/tests/cli.rs::hook_always_exits_zero_and_writes_nothing_to_stdout`
  （空 stdin・壊れた JSON・値の無いフラグ・書けないパス・巨大 payload を通す）。
- プラグイン経由で配るラッパースクリプトも同じ契約を守ること
  （`exec >/dev/null` してから本体を呼ぶ）。
- `--record` は payload をそのままファイルに落とすので**本番の設定に残さない**。
  ユーザが打った生のプロンプトが入る。`ccsessions doctor` が付けっぱなしを警告する。
