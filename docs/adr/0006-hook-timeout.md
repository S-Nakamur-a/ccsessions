# 0006 · hook の `timeout` はそのイベントの既定を上回らない

採用 · 2026-07-26

## 文脈

`settings.json` の hook エントリに `timeout` を書かないと、Claude Code の既定が
効く。実測した既定値は次のとおりで、桁が揃っていない。

| イベント | 既定 |
|---|---|
| 全般 | **600 秒** |
| `UserPromptSubmit` | 30 秒 |
| `MessageDisplay` | 10 秒 |
| `SessionEnd` | **1.5 秒** |

`ccsessions hook` は数 ms で終わるが、万一ストア書き込みでブロックすると、既定のまま
ではユーザのターンが最大 10 分止まりうる。

## 決定

install が書くエントリには必ず `timeout` を明示する。値は
**そのイベントの既定を決して上回らない**。現状は `SessionEnd` が 1 秒、他が 5 秒
（`ccsessions/src/settings_json.rs::hook_timeout_secs`）。

`SessionEnd` に一律 5 を書くと予算を 1.5 秒から 5 秒へ**引き上げる**ことになり、
「表示ツールがユーザの待ち時間を延ばさない」という意図と逆になる。

## 影響

- 新しく購読するイベントを足すときは、そのイベントの既定が 5 秒より短くないかを
  確認する。
- **既存エントリのうち `command` に `ccsessions hook` を含むものには後追いで
  `timeout` を足す。** [0007](0007-settings-json-merge-only.md) の「既存エントリを
  変更しない」は他ツールの hook を壊さないための規律であって、自分のエントリまで
  例外にすると、既に導入済みのユーザに timeout が永久に届かない。
