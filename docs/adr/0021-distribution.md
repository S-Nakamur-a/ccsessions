# 0021 · 配布は Homebrew の source formula と Claude Code プラグイン

採用 · 2026-07-25 — 改訂 2026-07-29
（**2026-07-29 に配布まで完了**。リポジトリ公開・`v0.1.0` タグ・tap への配置・
`sha256` 埋めまで実施し、brew とプラグインの両導線が通ることを実機で確認した。
詳細は[下記](#配り終えた2026-07-29)）

## 文脈

配る対象は 3 つあり、それぞれ独立に配れる。

| | 実体 |
|---|---|
| バイナリ | `ccsessions`（CLI / hook producer）と `ccsessionsd`（GUI 常駐） |
| hook 配線 | `~/.claude/settings.json` のエントリ |
| 常駐化 | LaunchAgent |

1 つのインストーラで全部やろうとすると、**他人の `settings.json` を勝手に書き換える**
という一番やってはいけないことになる。

## 決定

- **バイナリは自前 tap の source formula + `brew services`。**
- **hook 配線は Claude Code プラグインだけで行う。** 当初は `install-hooks` を
  退路として残す判断だったが、2026-07-29 に削除した（下記）。
- **エンドユーザ向け導線と開発者向け導線を README で分ける。** `make` は開発者用。

## 理由

**source formula（または `cargo install`）で配る限り、Apple Developer Program は要らない。**
`com.apple.quarantine` はダウンロードしたアプリに付く拡張属性で、`brew install`
（source / bottle 問わず）でも `cargo install` でも付かない。付くのは
`brew install --cask` とブラウザで dmg/zip を落とした場合だけ。`.app` を cask で配る道を
選んだ瞬間に公証（＝有料アカウント）が必要になる。

Apple Silicon で必須の ad-hoc 署名は、**macOS ホスト上で `cargo build` すれば
リンカが自動的に付ける**ので追加作業が要らない。これも source build を推す理由。

プラグイン方式の利点:

- `settings.json` に入るのは `enabledPlugins` の 1 行だけ。**hooks セクションを
  触らない**ので、バックアップ・冪等・完全復元の責務が丸ごと Claude Code 側へ移る。
- **hook 構成の更新を配れる。** 購読イベントを変えたときに「もう一度
  `install-hooks` を実行してください」と README でお願いする必要がなくなる
  （[0005](0005-subscribed-hook-events.md) のような変更は実際に起きる）。
- ラッパースクリプトを `${CLAUDE_PLUGIN_ROOT}` で指せるので、**バイナリの絶対パスを
  `settings.json` に焼く問題が消える**。Homebrew は `bin/` を Cellar への symlink に
  するので、`current_exe()` が Cellar 側を返すと `brew upgrade` のたびに hook が壊れる。

## 影響

- **formula に `~/.claude/settings.json` を触らせない。** Homebrew の formula は
  prefix の外を変更してはいけないし、他人のエディタ設定を勝手に書き換えるツールは
  信用されない。hook 配線は必ずユーザの明示的な操作にする。
- **同じ理由で、formula に LaunchAgent の plist も書かせない。** `~/Library/LaunchAgents/`
  も prefix の外である以上、扱いは `settings.json` と同じ。`brew services` という別
  コマンドが存在するのは、まさに「常駐化はユーザが明示的に起こす操作」を install から
  分離するため。**plist を書く主体は `brew services`（エンドユーザ）か `make plist`
  （開発者）のどちらかだけで、formula は関与しない。**
- **`brew services start` を自動実行しない。** `caveats` で案内するに留める。
- **`brew services` 側でもログは `~/Library/Logs/ccsessions/` に向ける**（`service do`
  ブロックの `log_path` / `error_log_path`）。`/tmp` は全ユーザ共有で、他人が先に同名の
  symlink を置けば launchd が**ユーザ権限で**その先へ追記する。再起動で消え 3 日で
  掃除される点も、常駐の障害調査には向かない。`Makefile` の `LOGDIR` と揃える。
- プラグインだけ入れてバイナリが無い人を壊さないこと。**その状態が静かで安全である**
  ことがラッパーの要件（`command -v` で分岐して黙って exit 0）。
  実装は `plugins/ccsessions/hooks/ccsessions-hook.sh`。
- 逆に「バイナリだけ入れてプラグインを入れない」人も出るので、`ccsessions doctor` は
  プラグイン経由の配線も検出できる必要がある。**実装済み** —
  `enabled_ccsessions_plugins` が `enabledPlugins` を見る。プラグインが配る hook は
  `settings.json` の `hooks` に現れないので、`MARKER` の走査では原理的に見つからない。
  そのため分かるのは「有効になっている」ところまでで、**イベント単位の欠落までは
  分からない**という非対称が残る。
- **イベント一覧と timeout は Rust の定数と `hooks.json` の 2 か所にある。**
  真実は `settings_json.rs` の `SIMPLE_EVENTS` / `hook_timeout_secs` の 1 か所と決め、
  `hooks.json` がそこからずれたらテストで落とす
  （`the_plugin_subscribes_to_exactly_the_declared_events` ほか 2 本）。
- **両方から配線しても状態は壊れない。** `reduce_subagent_start` は `agent_id` で
  冪等なので、同じ payload が 2 回来てもエージェントは二重に積まれない。壊れない
  ぶん気づきにくいので、doctor が「無駄である」ことだけ告げる。
- `brew services` のラベルは現行の LaunchAgent とは別物。**両方走ると生き物が
  二重に出る**ので、doctor に検出を足す（`doctor.rs` の `RESIDENCIES` に実装済み。
  `~/Library/LaunchAgents/<label>.plist` の有無と、走っている daemon の数の両方を見る）。
- **formula の原本は [`packaging/homebrew/ccsessions.rb`](../../packaging/homebrew/ccsessions.rb)。**
  tap（`S-Nakamur-a/homebrew-tap`）へはリリースのたびに `url` / `sha256` を差し替えて
  コピーする。原本をこのリポジトリに置くのは、formula の変更をこの ADR と一緒に
  レビューできるようにするため。
  → **[0027](0027-release-automation.md) が上書きした。** 実体は tap 側で、こちらは
  レビュー用の**写し**。`url` / `sha256` を書くのは CI で、人間は手で直さない。
- **ログの親ディレクトリは `brew services start` が作る。** Homebrew の
  `Service#path_dirs` が `log_path` / `error_log_path` の親を集め、
  `services/cli.rb` が `mkpath` する。formula 自身が prefix の外へ `mkdir` する
  必要はないので、「formula は prefix の外を触らない」と両立する。

## 配る前に片づけること

LICENSE（MIT）と `Cargo.toml` のメタデータは入れた（共通項目は `[workspace.package]`
に置き、各 crate が `.workspace = true` で引き継ぐ）。残っているのは 1 つ。

- **macOS 以外でのビルド** — `ccsessionsd` の objc2 依存を target 修飾していないので、
  Linux での `cargo install` は objc2 のコンパイルエラーで死ぬ。**brew 配布の
  ブロッカーではない**（formula 側は `depends_on :macos` で閉じられる）。効くのは
  `cargo install` の導線を残す場合だけ。
- ~~**名前**~~ — **2026-07-28 に `ccstatus` → `ccsessions` へ改名して解消**（下記）。

## 配り終えた（2026-07-29）

この ADR が設計した 3 本の導線を全部通した。

| | 実体 | 確認方法 |
|---|---|---|
| リポジトリ | `S-Nakamur-a/ccsessions`（public・`v0.1.0`） | CI（macos-15）が両ジョブ green |
| tap | `S-Nakamur-a/homebrew-tap` の `Formula/ccsessions.rb` | `brew install` / `brew test` / `brew audit --strict --online` が全部通る |
| プラグイン | リポジトリ自身が marketplace | `/plugin marketplace add S-Nakamur-a/ccsessions` を実機で実行 |

実機で確かめて分かったことを 2 つ残す。

- **`publish = false` は `cargo install --path` を妨げない。** formula は
  `std_cargo_args(path:)` 経由で `cargo install --locked --path` を打つだけなので、
  crates.io に出さない判断と source formula は両立する（懸念していたが問題なかった）。
- **ログの親ディレクトリは本当に `brew services` が作った。** 上で「formula が
  prefix の外へ mkdir する必要はない」と書いた読みは正しく、`brew services start`
  の直後に `~/Library/Logs/ccsessions/` が生えた。

**リリース手順は 4 手**（タグ → tarball の `sha256` → 原本の `url`/`sha256` 差し替え
→ tap へコピー）。原本と tap に同じ formula が 2 つある構造なので、片方だけ直すと
静かにずれる。原本をこのリポジトリに置く判断（レビュー可能性）とのトレードオフで、
承知のうえで受けている。

→ **この 4 手は [0027](0027-release-automation.md) が機械に渡した**（2026-07-29）。
いまは `make release VERSION=x.y.z` で Release PR を出し、それをマージすると CI が
タグを打って公開する。ずれの検出も PR 時の CI が持つ。**この節の手順を手でやらない。**

## `install-hooks` を消した（2026-07-29）

プラグインが hook 配線を完全に覆ったので、**`install-hooks` / `uninstall-hooks` を
実装ごと削除した**（`install_hooks.rs` 525 行 + `uninstall_hooks.rs` 283 行 +
統合テスト 530 行）。当初は「プラグインを使えない人の退路」として残す判断だったが、
入口を 2 つ維持し続けるコストの方が大きいと見て降ろした。

**得たもの**: ccsessions は他人の `settings.json` を書くコードを 1 行も持たなくなった。
[0007](0007-settings-json-merge-only.md) が守っていた 5 つの制約（推測しない・追記のみ・
バックアップ・同意・冪等）は、破りようが無いので不変条件ですらなくなった。

**失ったもの**（承知のうえ）:

- **貼るべき JSON 断片を出す案内。** プラグインを使えない環境（enterprise の
  managed settings 等）で手書きするときの唯一の導線だった。README に
  「`hooks.json` を参考に、command を絶対パスにし、timeout を落とさない」と
  書いて代替する。
- **イベント単位の診断。** プラグイン経由だと `doctor` に見えるのは
  `enabledPlugins` だけで、「`SubagentStart` だけ入っていない」は分からない。

`settings_json.rs` は**読み手として残る** — `doctor` が手書き配線の検出と
`enabledPlugins` の確認に使う。timeout の定数だけは実行時の参照者が居なくなったので
`#[cfg(test)]` にし、プラグインの `hooks.json` を検証する仕様として生かしてある。

## 名前を `ccsessions` にした（2026-07-28）

`ccstatus` は既に別プロジェクト（Claude Code の**ステータスライン**を出すツール）の
名前で、この界隈の "ccstatus*" はその意味で使われている。改名のコストは配る前が最小で、
配ったあとに変えると全員の設定を壊すので、ここで済ませた。

**crates.io は改名の理由ではない。** tap の source formula は GitHub の tarball を
`cargo build --locked` するだけなので、publish されているかは関係ない。効いたのは
brew 側の 2 つ。

- **`bin/ccstatus` が PATH で衝突する。** 先行ツールを `cargo install` で入れている
  人は、どちらかが上書きされる。
- **formula 名は後から変えると全員が入れ直しになる。** `brew services` のラベルにも
  効くので、配布後の改名は一番高い。

候補は比喩系（`perch` / `roost` / `gaggle`）と説明系で当てたが、**`brew install` の
一行を初見で見た人が何のツールか分かること**を優先して説明系を採った。`ccmenu` は
CCMenu（CI 状況を出す macOS メニューバーアプリ）と同カテゴリで衝突するので除外。
`ccflock` は flock がまさに比喩なので同じ理由で落とした。

**改名は `~/.claude/settings.json` の hook を自動では引き継がない。** 自分の hook を
見分ける目印が `settings_json.rs` の `MARKER`（＝ `"ccsessions hook"`）である以上、
旧名で入ったエントリは新しいバイナリからは「他人の hook」に見える（見えてはいけない
ものを消さない、という [0007](0007-settings-json-merge-only.md) の帰結として正しい）。
**旧バイナリで hook を外してから入れ替える**のが唯一の順序。設定と顔
（`~/.config/ccstatus/`）も同様に自動では移らない。
