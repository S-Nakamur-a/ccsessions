# ccsessions

Claude Code の走行中セッションを、macOS のメニューバー（または画面下のドック）に
**生き物の群れ**として常時表示する常駐オーバーレイ。

1 セッション = 1 匹。体の色と動きで状態が分かり、バッジはそのセッションが走らせている
エージェントの数、ホバーするとセッション名・状態・経過時間・エージェント一覧が出る。

## 状態

| 表示 | 状態 | いつ |
|---|---|---|
| `›` シアン・上下に揺れる・瞬きする | 作業中 | プロンプト送信後、Claude が動いている |
| `!` 琥珀・跳ねる | 判断待ち | 許可要求・通知が出て、あなたの入力を待っている |
| `⋯` 紫・横に漂う・横目 | エージェント待ち | サブエージェント（Task）が走っている |
| `z` 灰・静止・薄い | アイドル | 完了して一定時間経過 |
| `✓` 緑・静止 | 完了 | ターンが終わった直後（既定 3 分） |
| `×` 赤・ゆっくり明滅 | エラー | 直近のターンがエラーで終わった |

エラーは**グリッチではなくゆっくりした呼吸**にしてある（常時視界に入るものなので、
点滅で目を刺さないことを優先した）。

## インストール

前提は **macOS** だけ（`ccsessionsd` は AppKit / CoreAnimation を直に叩くので macOS 専用）。
特別な権限（画面収録など）は要らない。

```sh
brew install S-Nakamur-a/tap/ccsessions
```

> **tap はまだ公開していない。** それまではソースから入れる →
> [開発者向けの入れ方](#開発者向けの入れ方)。

ソースからビルドする formula なので、Rust ツールチェインは Homebrew が用意する。
ダウンロードした `.app` ではないため `com.apple.quarantine` が付かず、Gatekeeper の
確認も公証も要らない（[ADR 0021](docs/adr/0021-distribution.md)）。

**入れただけでは何も起きない。** hook の配線と常駐の開始は、どちらも
`ccsessions` が勝手にやらない — あなたの `settings.json` と LaunchAgents は、
あなたが明示的に操作したときだけ変わる。

```sh
brew services start ccsessions   # 常駐開始（以降はログイン時に自動起動）
```

hook は **Claude Code のプラグイン**で入れる。Claude Code の中で:

```
/plugin marketplace add S-Nakamur-a/ccsessions
/plugin install ccsessions@ccsessions-marketplace
```

こちらだと `settings.json` に入るのは `enabledPlugins` の 1 行だけで、**`hooks`
セクションには一切触らない**。購読イベントを変えたときの更新もプラグインの更新で
届く。プラグインを使えない環境での逃げ方は[下記](#hook-の入れ方)。

| したいこと | コマンド |
|---|---|
| 導入状況を確認する | `ccsessions doctor` |
| 常駐を止める | `brew services stop ccsessions` |
| hook を外す | Claude Code で `/plugin uninstall ccsessions@ccsessions-marketplace` |
| 丸ごと消す | 上の 2 つ → `brew uninstall ccsessions` |

### hook の入れ方

**ccsessions は Claude Code の設定ファイルを書き換えない。** 配線するのは
プラグインで、`settings.json` に入るのは `enabledPlugins` の 1 行だけ。
`hooks` セクションには一切触らないので、他のツールの hook を壊しようがない。

```
/plugin marketplace add S-Nakamur-a/ccsessions
/plugin install ccsessions@ccsessions-marketplace
```

購読するイベントは `plugins/ccsessions/hooks/hooks.json` にある（10 個）。
イベント構成を変えたときの更新も、プラグインの更新として届く。

外すのは `/plugin uninstall ccsessions@ccsessions-marketplace`。
導入状況の確認は `ccsessions doctor`（分割された設定ファイルも走査する）。

プラグインを使えない環境（enterprise の managed settings 等）では、
`hooks.json` を参考に手で `settings.json` へ書く。その場合 command は
`${CLAUDE_PLUGIN_ROOT}/...` ではなく `ccsessions hook` の絶対パスにする。
**`timeout` を落とさないこと** — 省くと Claude Code 側の既定（多くのイベントで
600 秒）が効き、hook が詰まったときにターンがそのぶん止まる。

## 設定

やり方は 2 つあり、どちらも同じ `~/.config/ccsessions/config.toml` を書く。
変えた瞬間に走っている `ccsessionsd` が数百 ms で拾う。

```sh
make config          # Web UI（http://127.0.0.1:8787/）。生き物の見た目もここで作れる
$EDITOR ~/.config/ccsessions/config.toml   # 直接編集でもよい（ccsessions config set も同じ）
```

```toml
placement = "bar"        # "bar"（メニューバー）| "dock"（画面下）
design = "egg"           # 組込みは "egg" | "round" | "squircle" | "bean"
                         # 自作の顔の id も書ける
reduce_motion = false
show_glyphs = true       # 状態記号（› ! ⋯ z ✓ ×）を出す
bar_align = "auto"       # "auto" | "center" | "left-of-notch" | "right-of-notch"
compact_flock = "auto"   # セッションが増えて入り切らなくなったら群れを縮める
                         # "auto"（既定）| "always"（常に縮める）| "never"（縮めない）
done_ttl_secs = 180      # 完了 → アイドルに変わるまで
session_ttl_secs = 28800 # これだけ無更新なら生き物を消す（下記のとおり保険）
max_sessions = 12
detect_errors = false    # Stop 時に transcript を見てエラー終了も判定する（補助手段）
```

bar はキーボードフォーカスのある画面のメニューバーに出る（外部モニタにも追従する）。
顔を TOML で自分で書く場合は [`faces/README.md`](faces/README.md)。

## 生き物が消えるとき

1. セッションが普通に終わった（`SessionEnd` hook）。
2. **セッションのプロセスが居なくなった** — 強制終了・端末を閉じた・親のツールに
   殺された等で `SessionEnd` が飛ばなかった場合。hook が記録した pid の生存を
   `ccsessionsd` が確かめる。
3. `session_ttl_secs` のあいだ 1 度も hook が来なかった（1・2 で拾えないときの保険）。

つまり **`session_ttl_secs` を長くしても死んだセッションは居座らない**。生存確認できない
ときは必ず「生きている」側に倒す。消したものは `~/Library/Logs/ccsessions/ccsessionsd.log` に
`reaped session ... — pid 12345 が居ない` の形で残る。

## 既知の制限

| 症状 | 原因 | 逃げ方 |
|---|---|---|
| ターンを中断（ESC）したときに状態が「作業中」のまま残る | 中断では `Stop` も `StopFailure` も来ない | 次のプロンプトを送れば戻る |
| エラー（`×` 赤）がほとんど出ない | API エラーは `StopFailure` で取るが、それ以外の失敗は hook から見えない | — |
| ホバーカードのエージェント行に役割ラベルが出ない | `agent_id` と Agent ツールの `description` を突き合わせる手段が payload に無い | — |
| バッジは 32 個で頭打ちになる | `event.rs` の `MAX_AGENTS` による意図的な上限 | — |
| **`bar_align = "center"` はノッチ機で群れが隠れる** | ノッチは画面の水平中央にあるので、中央配置は必ずその下に入る | 既定の `auto`（ノッチの右→左へ退避する）を使う。起動ログと `ccsessions doctor` も警告する |
| メニューエクストラを増やしても群れの位置が追随しない環境がある | ノッチ右の空き幅は**実行時に計測**して追随する（最大 10 秒の遅れ）。ただし計測できない環境（非ノッチ機・メニューバー自動非表示・フルスクリーン）では**見積もりの 225pt** に落ちる | `bar_align` を `left-of-notch` か `center` にする |
| セッションが 20 匹前後を超えると bar に収まらない | 群れの縮小には下限（0.55 倍）があり、そこから先は判読できなくなるので諦めている | `max_sessions` を下げる／`placement = "dock"` にする |
| enterprise の managed settings に入れた hook は診断で拾えない | 走査するのはユーザ全体・プロジェクト・ローカルの settings ファイルだけ | そこに入れた場合は `doctor` の「NOT installed」を無視してよい |
| プラグイン経由の hook は「有効になっていること」までしか分からない | プラグインが配る hook は `settings.json` の `hooks` に現れない。`doctor` が見られるのは `enabledPlugins` だけ | イベント単位で確かめたいときは `plugins/ccsessions/hooks/hooks.json` を直接見る |

## CLI

```sh
ccsessions list [--json]        # 生きているセッションの一覧
ccsessions ui                   # 設定 + 顔作りの Web UI ＝ make config
ccsessions config get|set|path  # 設定の表示・変更（UI と同じ検証を通る）
ccsessions doctor               # 診断
ccsessions hook                 # Claude Code の hook が呼ぶ（stdin から JSON）
```

## 開発

`make` は開発者向けで、エンドユーザの導線（`brew` + `brew services`）とは独立している。
**両方で常駐させると生き物が二重に出る**ので、どちらか一方にすること
（`ccsessions doctor` が検出する）。

### 開発者向けの入れ方

前提は [rustup](https://rustup.rs/) の Rust ツールチェイン（MSRV 1.89）と、
`~/.cargo/bin` が `PATH` に入っていること。

```sh
make install   # release ビルドして ~/.cargo/bin へ入れる
make start     # LaunchAgent に登録して常駐開始（以降はログイン時に自動起動）
```

hook は開発中もプラグインで入れる。チェックアウトをそのまま marketplace として
使えるので、Claude Code の中で `/plugin marketplace add .`（リポジトリのルート）
→ `/plugin install ccsessions@ccsessions-marketplace`。

`make start` はビルドしない — 更新を取り込むのは `make deploy`（ビルドして入れ直し、
常駐を入れ替える）。止めるのは `make stop`、丸ごと外すのは `make uninstall`
（**hook はプラグイン側なので触らない**）。

### よく使うターゲット

```sh
make check   # fmt --check + clippy -D warnings + test（コミット前の品質ゲート）
make dev     # 走行中の daemon を止めて build → dev バイナリを起動（install 不要）
make demo    # 6 状態のダミーセッションで見た目を確認（実セッション不要）
make help    # ターゲット一覧
```

- **[`docs/how-it-works.md`](docs/how-it-works.md)** — hook からオーバーレイまでの流れ、
  購読しているイベント、なぜ CALayer 直描きなのか
- **[`docs/invariants.md`](docs/invariants.md)** — 崩してはいけない不変条件
- **[`docs/adr/`](docs/adr/README.md)** — なぜ他の案を採らなかったのか
- **[`faces/README.md`](faces/README.md)** — 顔（生き物のデザイン）の作り方。Rust は要らない

## ライセンス

[MIT](LICENSE)
