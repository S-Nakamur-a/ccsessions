# ADR — この実装がなぜこうなっているか

一度決めて、破ると静かに壊れる判断だけをここに置く。調査ログ・実測の生データ・
やらなかったことの一覧は残していない（決定に必要だった分だけ各 ADR の「理由」に畳んである）。

**コードのコメントはそれ単体で読めるように書いてある。** ここを参照するのは、
「なぜ他の案を採らなかったか」まで知りたいときだけでよい。

| # | 決定 |
|---|---|
| [0001](0001-hook-as-the-state-source.md) | 状態のソースは Claude Code の hook にする |
| [0002](0002-one-file-per-session.md) | セッションは 1 ファイル。書き手はストア全体を排他する |
| [0003](0003-calayer-not-webview.md) | 描画は CALayer 直描き。WebView は使わない |
| [0004](0004-hook-never-blocks.md) | `ccsessions hook` は常に exit 0・stdout に何も書かない |
| [0005](0005-subscribed-hook-events.md) | 購読する hook イベントと、その状態遷移 |
| [0006](0006-hook-timeout.md) | hook の `timeout` はそのイベントの既定を上回らない |
| [0007](0007-settings-json-merge-only.md) | `settings.json` は追記マージだけ。完全に戻せること |
| [0008](0008-liveness-by-pid.md) | セッションの死活は持ち主の pid で決める。TTL は保険 |
| [0009](0009-stable-display-order.md) | 表示順は名前 → id の安定順 |
| [0010](0010-narrow-band-folds-inward.md) | 狭い帯では要素を体に重ねて集約する |
| [0011](0011-menu-bar-height-fallback.md) | メニューバー高が測れないときは 24pt を仮定する |
| [0012](0012-notch-avoidance.md) | ノッチは横方向に避ける |
| [0013](0013-uniform-squeeze.md) | 群れの縮小は一様でなければならない |
| [0014](0014-faces-as-data.md) | 顔は `faces/*.toml` のデータで定義する |
| [0015](0015-state-palette-is-global.md) | 色・アニメ・グリフは顔ごとに変えられない |
| [0016](0016-design-resolved-late.md) | `design` の実在検証はレジストリ解決時に行う |
| [0017](0017-face-bar-height-limit.md) | 顔の bar の体の高さは 22pt 以下 |
| [0018](0018-svg-preview.md) | SVG プレビューを目視の代わりにする |
| [0019](0019-builder-emits-toml.md) | ビルダーは TOML テキストを唯一の中間形にする |
| [0020](0020-single-settings-entrance.md) | 設定の入口は Web UI ひとつだけ |
| [0021](0021-distribution.md) | 配布は Homebrew の source formula と Claude Code プラグイン |
| [0022](0022-zombie-is-not-alive.md) | ゾンビプロセスは「死んでいる」と判定する（0008 の続き） |
| [0023](0023-session-title-from-transcript.md) | セッションタイトルは transcript から読む |
| [0024](0024-stop-is-not-the-end-of-subagents.md) | `Stop` はサブエージェントの終わりを意味しない（0005 の続き） |
| [0025](0025-ui-is-bilingual-diagnostics-are-english.md) | 画面の文言は日英、診断は英語。顔パーツ名は出さない |
| [0026](0026-ignore-is-a-display-filter.md) | `ignore` は表示のフィルタ。枠を数える前に外す |
| [0027](0027-release-automation.md) | リリースは Release PR のマージが引き金。タグは CI が打つ（0021 の続き） |
