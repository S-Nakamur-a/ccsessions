[English](README.md) | **日本語**

# ccsessions

Claude Code の走行中セッションを、macOS のメニューバーに生き物の群れとして表示します。
1 セッション = 1 匹です。色と動きで状態が分かり、ホバーすると詳細が出ます。

![作業中・判断待ち・エージェント待ち・アイドル・完了・エラーの 6 状態](docs/assets/states.svg)

## インストール

macOS のみです。画面収録などの特別な権限は要りません。

```sh
brew install S-Nakamur-a/tap/ccsessions
brew services start ccsessions
```

次に Claude Code の中で、状態を送る hook を入れます:

```
/plugin marketplace add S-Nakamur-a/ccsessions
/plugin install ccsessions@ccsessions-marketplace
```

Claude Code を再起動すれば出ます。うまくいかないときは `ccsessions doctor`
を実行してください（何が入っていて何が足りないかを教えます）。

## 設定

```sh
ccsessions ui
```

ブラウザが開きます。言語、メニューバーか画面下か、生き物の見た目、どれくらいで消えるか
等をそこで決められます。顔を自分で作れるのもここです。

設定画面・キャラクタービルダー・ホバーカードは日本語と英語に対応していて、既定では
OS の言語に従います。診断（`ccsessions doctor`）は常に英語です。

<details>
<summary>設定ファイルを直接書く場合</summary>

`~/.config/ccsessions/config.toml` です。変えた瞬間に走っている常駐が数百 ms で拾います。

```toml
language = "auto"        # "auto"（OS に従う）| "ja" | "en"
                         # 設定画面・ビルダー・ホバーカードの文言。
                         # 診断（`ccsessions doctor`）は英語のまま。
placement = "bar"        # "bar"（メニューバー）| "dock"（画面下）
design = "egg"           # 組込みは "egg" | "round" | "squircle" | "bean"
                         # 自作の顔の id も書ける
reduce_motion = false
show_glyphs = true       # 状態記号（› ! ⋯ z ✓ ×）を出す
bar_align = "auto"       # "auto" | "center" | "left-of-notch" | "right-of-notch"
compact_flock = "auto"   # セッションが増えて入り切らなくなったら群れを縮める
                         # "auto"（既定）| "always"（常に縮める）| "never"（縮めない）
done_ttl_secs = 180      # 完了 → アイドルに変わるまで
session_ttl_secs = 28800 # これだけ無更新なら生き物を消す（保険。下記参照）
max_sessions = 12
detect_errors = false    # Stop 時に transcript を見てエラー終了も判定する（補助手段）
```

bar はキーボードフォーカスのある画面のメニューバーに出ます（外部モニタにも追従します）。
顔を TOML で手書きする場合は [`faces/README.md`](faces/README.md) を参照してください。

</details>

## 状態

| 表示 | 状態 | いつ |
|---|---|---|
| `›` シアン・上下に揺れる・瞬きする | 作業中 | プロンプト送信後、Claude が動いています |
| `!` 琥珀・跳ねる | 判断待ち | 許可要求・通知が出て、あなたの入力を待っています |
| `⋯` 紫・横に漂う・横目 | エージェント待ち | サブエージェント（Task）が走っています |
| `z` 灰・静止・薄い | アイドル | 完了して一定時間経過しました |
| `✓` 緑・静止 | 完了 | ターンが終わった直後です（既定 3 分） |
| `×` 赤・ゆっくり明滅 | エラー | 直近のターンがエラーで終わりました |

バッジはそのセッションが走らせているエージェントの数です。

## やめる・消す

| したいこと | コマンド |
|---|---|
| 常駐を止める | `brew services stop ccsessions` |
| hook を外す | Claude Code で `/plugin uninstall ccsessions@ccsessions-marketplace` |
| 丸ごと消す | 上の 2 つ → `brew uninstall ccsessions` |

<details>
<summary>プラグインを使えない環境</summary>

購読しているイベントは `plugins/ccsessions/hooks/hooks.json` にあります（10 個）。
enterprise の managed settings 等でプラグインを入れられない場合は、これを参考に手で
`settings.json` へ書いてください。その場合 command は `${CLAUDE_PLUGIN_ROOT}/...` ではなく
`ccsessions hook` の絶対パスにします。`timeout` を落とさないでください — 省くと Claude Code
側の既定（多くのイベントで 600 秒）が効き、hook が詰まったときにターンがそのぶん止まります。

</details>

<details>
<summary>生き物が消えるとき</summary>

1. セッションが普通に終わったとき（`SessionEnd` hook）。
2. セッションのプロセスが居なくなったとき — 強制終了・端末を閉じた・親のツールに
   殺された等で `SessionEnd` が飛ばなかった場合です。hook が記録した pid の生存を
   常駐が確かめます。
3. `session_ttl_secs` のあいだ 1 度も hook が来なかったとき（1・2 で拾えないときの保険）。

つまり `session_ttl_secs` を長くしても死んだセッションは居座りません。生存確認できない
ときは必ず「生きている」側に倒します。消したものは `~/Library/Logs/ccsessions/ccsessionsd.log`
に `reaped session ... — pid 12345 が居ない` の形で残ります。

</details>

<details>
<summary>既知の制限</summary>

| 症状 | 原因 | 逃げ方 |
|---|---|---|
| ターンを中断（ESC）したときに状態が「作業中」のまま残ります | 中断では `Stop` も `StopFailure` も来ません | 次のプロンプトを送れば戻ります |
| エラー（`×` 赤）がほとんど出ません | API エラーは `StopFailure` で取りますが、それ以外の失敗は hook から見えません | — |
| ホバーカードのエージェント行に役割ラベルが出ません | `agent_id` と Agent ツールの `description` を突き合わせる手段が payload にありません | — |
| バッジは 32 個で頭打ちになります | `event.rs` の `MAX_AGENTS` による意図的な上限です | — |
| `bar_align = "center"` はノッチ機で群れが隠れます | ノッチは画面の水平中央にあるので、中央配置は必ずその下に入ります | 既定の `auto`（ノッチの右→左へ退避します）を使ってください。起動ログと `ccsessions doctor` も警告します |
| メニューエクストラを増やしても群れの位置が追随しない環境があります | ノッチ右の空き幅は実行時に計測して追随します（最大 10 秒の遅れ）。ただし計測できない環境（非ノッチ機・メニューバー自動非表示・フルスクリーン）では見積もりの 225pt に落ちます | `bar_align` を `left-of-notch` か `center` にしてください |
| セッションが 20 匹前後を超えると bar に収まりません | 群れの縮小には下限（0.55 倍）があり、そこから先は判読できなくなるので諦めています | `max_sessions` を下げるか、`placement = "dock"` にしてください |
| enterprise の managed settings に入れた hook は診断で拾えません | 走査するのはユーザ全体・プロジェクト・ローカルの settings ファイルだけです | そこに入れた場合は `doctor` の「NOT installed」を無視して構いません |
| プラグイン経由の hook は「有効になっていること」までしか分かりません | プラグインが配る hook は `settings.json` の `hooks` に現れません。`doctor` が見られるのは `enabledPlugins` だけです | イベント単位で確かめたいときは `plugins/ccsessions/hooks/hooks.json` を直接見てください |

</details>

## CLI

```sh
ccsessions list [--json]        # 生きているセッションの一覧
ccsessions ui                   # 設定 + 顔作りの Web UI
ccsessions config get|set|path  # 設定の表示・変更（UI と同じ検証を通る）
ccsessions doctor               # 診断
ccsessions face list|render     # 顔の一覧・SVG プレビュー
ccsessions hook                 # Claude Code の hook が呼ぶ（stdin から JSON）
```

## 開発

```sh
make check   # fmt --check + clippy -D warnings + test（コミット前の品質ゲート）
make dev     # 走行中の常駐を止めて build → dev バイナリを起動（install 不要）
make demo    # 6 状態のダミーセッションで見た目を確認（実セッション不要）
make help    # ターゲット一覧
```

前提は [rustup](https://rustup.rs/) の Rust ツールチェイン（MSRV 1.89）です。
`make install` で `~/.cargo/bin` へ入れ、`make start` で常駐します。
`brew services` と両方で常駐させると生き物が二重に出るので、どちらか一方にしてください
（`ccsessions doctor` が検出します）。hook は開発中もプラグインで入れます —
チェックアウトをそのまま marketplace として使えるので、`/plugin marketplace add .`
→ `/plugin install ccsessions@ccsessions-marketplace` としてください。

README を直したときは、英語版（[`README.md`](README.md)）と日本語版（このファイル）の
両方を更新してください。

- [`docs/how-it-works.md`](docs/how-it-works.md) — hook からオーバーレイまでの流れ、
  購読しているイベント、なぜ CALayer 直描きなのか
- [`docs/invariants.md`](docs/invariants.md) — 崩してはいけない不変条件
- [`docs/adr/`](docs/adr/README.md) — なぜ他の案を採らなかったのか
- [`faces/README.md`](faces/README.md) — 顔（生き物のデザイン）の作り方。Rust は要りません

## ライセンス

[MIT](LICENSE)

---

この日本語版は [`README.md`](README.md) の翻訳です。内容が食い違っている場合は英語版が正です。
