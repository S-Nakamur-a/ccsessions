# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

（コード内のコメント・doc comment・ドキュメントはすべて日本語で書かれている。追記するときも日本語で揃える。
例外はリポジトリ直下の `README.md` — ここだけ英語が正で、日本語版は `README.ja.md` に置く。
README を直したら必ず両方を更新する）

**ユーザに出力する文字列は別のルールに従う**（[ADR 0025](docs/adr/0025-ui-is-bilingual-diagnostics-are-english.md)）。
画面の文言（設定・ビルダー・ホバーカード）は `lang::L { ja, en }` の対訳で持ち、診断
（`doctor`・CLI のヘルプとエラー・顔の検証メッセージ・`config.toml` のコメント）は英語で固定。
顔パーツのバリエーション名は画面に出さないので日本語のままでよい。

## コマンド

`make help` にターゲット一覧がある。よく使うのは `make check`（コミット前の品質ゲート:
fmt --check + clippy -D warnings + test）・`make test` / `make integration-test`・
`make dev`・`make demo`・`make config`。

- **`ccsessionsd` は `cargo run` で起動しない。必ず `make dev`**（走行中インスタンスの
  停止が要る macOS GUI なので）。dev のログは `/tmp/ccsessionsd-dev.log`。
- 見た目のイテレーションは `ccsessionsd/src/theme.rs` の定数を編集 → `make dev` の繰り返し。

## アーキテクチャ

3 crate の Rust workspace。**「macOS FFI を触るコード」と「純粋なロジック」を
crate / モジュール境界で分離する**のが全体の設計原理。データの流れは
[`docs/how-it-works.md`](docs/how-it-works.md)。

**ccsessions-core** — macOS 非依存・全部テスト可能。

| module | 役割 |
|---|---|
| `session.rs` | 状態モデルと表示派生 |
| `event.rs` | hook payload → 状態遷移の**純関数 reducer**（I/O 無し） |
| `store.rs` / `lock.rs` | 1 セッション 1 ファイル・atomic write / 並列 hook 間の `flock` |
| `process.rs` | 持ち主 pid の生存確認（`kill` は POSIX、ゾンビ判定は macOS 固有） |
| `config.rs` | 設定。**キー・型・選択肢・検証を `fields()` / `set_field()` に集約** |
| `ignore/` | 一覧から外す条件（`parse` 検証 / `glob` 照合）。**表示のフィルタで、生死ではない** |
| `face/` | 顔のデータモデル（`spec` / `parse` / `validate` / `registry` / `svg` / `golden`） |
| `face/builder/` | キャラクタービルダー（`parts` パーツの表 / `shape` 幾何 / `emit` TOML 生成） |
| `transcript.rs` | transcript の tail 読み。エラー判定（補助手段・既定 off）とセッションタイトル |

**ccsessions** — CLI 兼 hook producer。`hook.rs` が stdin の JSON を `reduce` に通して
store を更新する。`settings_json.rs` は `~/.claude/settings.json` を**読むだけ**
（`doctor` の診断用。書き込みコードは持たない — hook 配線は Claude Code プラグイン
`plugins/ccsessions/` の仕事）。`ui_cmd.rs` + `ui/` が `ccsessions ui`（＝ `make config`。
依存なしの 127.0.0.1 HTTP サーバ + 単一 HTML）で、**設定と顔作りの唯一の GUI**。

**ccsessionsd** — 常駐オーバーレイ。`main.rs`（tao イベントループ + poller スレッド。
可変状態は `Overlay` にまとめてあり、イベント 1 種 = メソッド 1 つ）/ `theme.rs`
（**全部の顔に共通の**デザイン定数。顔ごとの形は `faces/*.toml`）/ `layout.rs`・
`geometry.rs`（**純関数のみ**）/ `ffi.rs`（FFI 定型のみ、ロジック無し）/
`creature.rs`・`card.rs`・`flock.rs`（CALayer 構築）/ `window.rs`・`screen.rs`（AppKit）。
**設定は読むだけ**（書くのは dock のドラッグ位置のみ）。

## 崩してはいけない不変条件

**コードを変える前に [`docs/invariants.md`](docs/invariants.md) を読む。** いずれも実際に
踏んだ問題の再発防止で、破ると**静かに壊れる**（テストは通り、その場では正しく見え、
あとで別の場所が壊れる）。索引:

| 領域 | 破ると何が起きるか |
|---|---|
| **hook は必ず exit 0・stdout に書かない** | 権限を拒否する／ユーザのプロンプトを消す／エージェントのループを止める |
| **`settings.json` は読むだけ（配線はプラグイン）・matcher 無し** | 他ツールの hook を壊す／そのイベントが一切届かなくなる |
| **`Stop` でサブエージェントを消さない（`background_tasks` と突き合わせる）** | teammate が働いている間ずっと「完了」→「判断待ち」に見える |
| **ストアの書き手は `lock_exclusive` を保持する** | 並列 hook の read-modify-write が後勝ちで消える |
| **死活は pid で決める（ゾンビは死）。TTL は保険** | 死んだセッションが 8 時間居座り、枠を食う |
| **`ignore` は `take(max)` の前で外す。`sweep` には効かせない** | 隠したセッションが枠を食って生きているセッションを押し出す／表示のフィルタのはずがファイルを消す |
| **bar はメニューバー高に収まる（33pt / 24pt）** | 帯の矩形ぶんメニューバーのクリックを奪う |
| **群れの縮小は一様** | 縮めたのにはみ出す／体だけ縮んで目が飛び出す |
| **アニメは CoreAnimation に自走させる／レイヤを作り直さない** | 常時 CPU を食う／群れが不自然に同期する |
| **設定の入口は Web UI だけ。スキーマは `fields()` の 1 か所** | 片方が腐る（設定が嘘をつく） |
| **顔は `faces/*.toml` が唯一の定義** | `theme.rs` に顔ごとの分岐が戻り、顔を足すのに Rust が要る |

「なぜ他の案を採らなかったのか」は [`docs/adr/`](docs/adr/README.md)。

## テストの流儀

- 環境変数（`CCSESSIONS_STATE_DIR` / `CCSESSIONS_CONFIG`）に依存させない。`store.rs`・
  `config.rs` はディレクトリを明示的に受け取る `*_in` 系の内部関数を持ち、テストは
  そちらを叩く（並列実行で干渉しないため）。
- **本物の `~/.claude/settings.json` と `~/.config/ccsessions/` に絶対に触らない。**
  tempdir に埋め込みフィクスチャを書き、パスを明示的に渡す。
- `ccsessionsd` のテストは FFI を含まない層（`theme` / `layout` / `geometry`）に閉じる。
  配置のロジックは、AppKit から読んだ値を `ScreenMetrics` として受け取る純関数側に置く。
- テスト名は「何が保証されるか」が読める英語の文にする
  （`bar_layout_fits_inside_a_notched_menu_bar` のように）。
- 画面収録権限が無い環境では目視検証ができない。`ccsessionsd` は画面ジオメトリとホバー対象を
  stderr に出すので、配置と当たり判定は `/tmp/ccsessionsd-dev.log` の数値で確かめる。
  顔は `ccsessions face render` の SVG で見る。

## 未実装

**配布は 2026-07-29 に完了した**（[`docs/adr/0021-distribution.md`](docs/adr/0021-distribution.md)）。
リポジトリ公開・`v0.1.0` タグ・tap（`S-Nakamur-a/homebrew-tap`）への formula 配置・
`sha256` 埋めまで済んでいて、`brew install S-Nakamur-a/tap/ccsessions` と
`/plugin marketplace add S-Nakamur-a/ccsessions` の両方が実際に通る。

リリースのたびに要るのは、タグを切る → tarball の `sha256` を取る →
`packaging/homebrew/ccsessions.rb`（原本）の `url` / `sha256` を差し替える →
tap の `Formula/ccsessions.rb` へコピー、の 4 手。**原本と tap の 2 か所に同じ
formula がある**ので、片方だけ直すとずれる。

イベント一覧と timeout は Rust の定数と `hooks.json` の 2 か所にある。**真実は
`settings_json.rs` の `SIMPLE_EVENTS` / `hook_timeout_secs` の 1 か所**で、
ずれたらテストが落ちる。

macOS 以外でのビルド（`ccsessionsd` の objc2 依存を target 修飾していない）は残っているが、
formula 側を `depends_on :macos` で閉じるので brew 配布のブロッカーではない。
