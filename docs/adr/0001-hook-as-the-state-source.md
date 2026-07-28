# 0001 · 状態のソースは Claude Code の hook にする

採用 · 2026-07-25

## 文脈

走行中セッションの状態（作業中・判断待ち・エージェント待ち）を知る方法は 2 つある。

1. Claude Code の hook を仕込み、イベントの瞬間に呼んでもらう
2. transcript の JSONL を tail して推測する

## 決定

hook を使う。`ccsessions hook` が stdin の payload を受け、セッションファイルを書く。

## 理由

- **判断待ちとエージェント待ちは JSONL からは事後的にしか読めない。** hook なら
  権限ダイアログが出た瞬間・サブエージェントが起動した瞬間に飛ぶので、可視化の
  遅れがほぼゼロになる。
- セットアップが要る点はプラグイン導入の 1 手で足りる。

## 影響

- **高頻度イベントは購読しない。** hook はイベントごとにプロセスを起動するので、
  ツール呼び出しごとに飛ぶ `PostToolUse` / `PreToolUse` を購読するとその頻度ぶん
  プロセスが立つ。購読するイベントは [0005](0005-subscribed-hook-events.md) に絞ってある。
- hook は他人の `settings.json` を触るので、その扱いは
  [0004](0004-hook-never-blocks.md) と [0007](0007-settings-json-merge-only.md) で縛る。

## 撤回条件

hook のプロセス起動が体感で重い（1 イベントあたり 50ms を超える）なら、
JSONL tail との併用を再検討する。
