# ccsessions — dev / release helpers
#
# 設定・顔作り:
#   `make config`  … Web UI（127.0.0.1:8787）。設定の入口はここだけ
#                    （メニューバーの status item は廃止した）
#
# 見た目イテレーション（一番よく使う）:
#   ccsessionsd/src/theme.rs の定数をいじる → `make dev` → 画面で確認 → 繰り返す
#
# リリース（この 2 つで回す）:
#   `make preview`             … main のコードを production と同じ形で起動して目視する
#   `make release VERSION=x.y.z` … preview を畳んで Release PR を出す
#
# 常駐（production）は brew のものだけ。`brew services start ccsessions` で起こし、
# 更新は `brew upgrade ccsessions` で降ってくる。Makefile から常駐させる導線
# （install / plist / start / restart / deploy / uninstall）は消した — 入口が 2 つ
# あること自体が「生き物が二重に出る」の原因で、手元のコードを見る用は
# `make dev`（debug）と `make preview`（release）で足りる。docs/adr/0028 を参照。
#
# その brew 常駐との重複は Makefile が面倒を見る。手元のコードを起こす側
# （dev / demo / preview）が brew 常駐を退避し、畳む側（stop / release）が戻す。
# 退避は `launchctl bootout` で、plist は消さない。だから戻し忘れても次のログインで
# launchd が読み直して production が自ら復活する。恒久的に消えたままになる経路を
# 作らないための選択。

CARGO      ?= cargo
UID        := $(shell id -u)
# かつて `make start` が置いていた LaunchAgent。導線は消したが、置き土産が残っている
# 環境があるので、手元の daemon を起こす前に落とす（残っていると二重に出る）。
# `ccsessions doctor` も検出する。
LABEL      := dev.ccsessions.ccsessionsd
AGENT      := gui/$(UID)/$(LABEL)
DAEMON_DEV := target/debug/ccsessionsd
# `make preview` が起こすもの。release ビルドで、brew で配るものと同じ最適化。
DAEMON_PRE := target/release/ccsessionsd
# brew 由来の常駐。formula は `keep_alive true` なので、プロセスを殺しても launchd が
# 蘇らせる。止める手段は launchd に外させることだけ。
#
# `brew services kill` は使えない。`keep_alive true` の service を Homebrew は拒む
# （"is set to automatically restart and can't be killed"）うえ、exit 0 で返すので、
# 呼んでも黙って何も起きない（実際に踏んだ）。
# `brew services stop` も使わない。あれは plist ごと消すので、退避したのかユーザが
# 自分で止めたのかが区別できなくなる（そして再ログインでも戻らない）。
BREW_AGENT  := gui/$(UID)/homebrew.mxcl.ccsessions
BREW_PLIST  := $(HOME)/Library/LaunchAgents/homebrew.mxcl.ccsessions.plist
# **hook の配線ターゲットはここには無い。** settings.json に書くのは Claude Code
# プラグイン（plugins/ccsessions/）の仕事で、ccsessions は他人の設定ファイルを
# 一切書き換えない（docs/adr/0021-distribution.md）。開発中に配線するなら
# Claude Code の中で `/plugin marketplace add .` を使う。

.DEFAULT_GOAL := help

# ---------------------------------------------------------------------------
# brew 常駐の退避と復帰（手元のコードを起こす全部から使う）
# ---------------------------------------------------------------------------
#
# ヘルプに出さない（`##` を付けない）。人間が直接叩くものではなく、
# dev / demo / preview / stop / release が内部で呼ぶ対。単体で呼ばれても壊れない
# ように、どちらも「いま退避が要る／戻す先があるか」を自分で見てから動く。

# 走っていなければ何もしない。bootout は非同期（`restart` の注記と同じ）なので、
# 本当に消えるまで待つ。待たずに自分の daemon を起こすと、その間だけ二重に出る。
.PHONY: brew-pause
brew-pause:
	@if launchctl print $(BREW_AGENT) >/dev/null 2>&1; then \
	  echo "brew 常駐を退避する（plist は残すので、戻し忘れても次のログインで復活する）"; \
	  launchctl bootout $(BREW_AGENT) 2>/dev/null || true; \
	  for i in 1 2 3 4 5 6 7 8 9 10; do \
	    launchctl print $(BREW_AGENT) >/dev/null 2>&1 || break; \
	    sleep 0.3; \
	  done; \
	fi

# 「plist は在るのに service が居ない」＝退避中、という判定。ユーザが自分で
# `brew services stop` した場合は plist ごと消えるので、ここは何もしない
# （＝ユーザが明示的に止めたものを勝手に起こさない。ADR 0021 の流儀）。
.PHONY: brew-resume
brew-resume:
	@if [ -f "$(BREW_PLIST)" ] && ! launchctl print $(BREW_AGENT) >/dev/null 2>&1; then \
	  echo "brew 常駐を戻す"; \
	  launchctl bootstrap gui/$(UID) "$(BREW_PLIST)" 2>/dev/null || true; \
	fi

# ---------------------------------------------------------------------------
# 見た目の高速ループ
# ---------------------------------------------------------------------------

.PHONY: dev
dev: ## 走行中の daemon を止め→build→dev binary を起動（install 不要）
	-@pkill -f '$(DAEMON_DEV)' 2>/dev/null || true
	-@pkill -f '$(DAEMON_PRE)' 2>/dev/null || true
	-@launchctl bootout $(AGENT) 2>/dev/null || true
	@$(MAKE) --no-print-directory brew-pause
	@$(CARGO) build -q -p ccsessionsd
	@$(DAEMON_DEV) >/tmp/ccsessionsd-dev.log 2>&1 &
	@echo "ccsessionsd(dev) 起動。theme.rs を直して 'make dev' を繰り返す。log: /tmp/ccsessionsd-dev.log"

.PHONY: demo
demo: ## 6 状態のダミーセッションを出して見た目を確認する（実セッション不要）
	-@pkill -f '$(DAEMON_DEV)' 2>/dev/null || true
	-@pkill -f '$(DAEMON_PRE)' 2>/dev/null || true
	-@launchctl bootout $(AGENT) 2>/dev/null || true
	@$(MAKE) --no-print-directory brew-pause
	@$(CARGO) build -q -p ccsessionsd
	@$(DAEMON_DEV) --demo >/tmp/ccsessionsd-dev.log 2>&1 &
	@echo "ccsessionsd(demo) 起動。log: /tmp/ccsessionsd-dev.log"

.PHONY: config
config: ## 設定と顔作りの Web UI を立ち上げる（127.0.0.1:8787）。設定の入口はここだけ
	@$(CARGO) build -q -p ccsessions
	@target/debug/ccsessions ui

.PHONY: stop
stop: ## 手元の ccsessionsd を全部止めて、退避してある brew 常駐を戻す
	-@pkill -f '$(DAEMON_DEV)' 2>/dev/null || true
	-@pkill -f '$(DAEMON_PRE)' 2>/dev/null || true
	-@launchctl bootout $(AGENT) 2>/dev/null || true
	@$(MAKE) --no-print-directory brew-resume
	@echo "stopped"

# ---------------------------------------------------------------------------
# 品質ゲート
# ---------------------------------------------------------------------------

.PHONY: check
check: ## fmt --check + clippy -D warnings + test
	$(CARGO) fmt --all -- --check
	$(CARGO) clippy --all-targets -- -D warnings
	$(CARGO) test

.PHONY: test
test: ## 全テスト（unit + 統合）
	$(CARGO) test

# 実プロセスを起動して hook の契約（exit 0・stdout 無言・並列書き込み）を見るテスト。
# ライブラリの単体テストでは再現できない性質なので分けてある。
.PHONY: integration-test
integration-test: ## CLI 統合テスト（ccsessions/tests/cli.rs。実プロセスを起動する）
	$(CARGO) test -p ccsessions --test cli

# ---------------------------------------------------------------------------
# リリース
# ---------------------------------------------------------------------------

# 版を持っているファイルはこの 2 つだけ。**`release.yml` の guard ジョブが
# 「この 2 つが同じ版であること」を毎リリース検証する**ので、増やすときは
# あちらも一緒に動かす。
CARGO_TOML  := Cargo.toml
PLUGIN_JSON := plugins/ccsessions/.claude-plugin/plugin.json

# `preview` と `release` が共有する前提。「見たもの＝出るもの」を保証する 1 か所で、
# ここがずれていると、未レビューの変更を目視してリリースしたつもりになる。
# ヘルプには出さない（人間が直接叩くものではない）。
.PHONY: main-sync
main-sync:
	@test -z "$$(git status --porcelain)" \
	  || { echo "作業ツリーが汚れている。commit か stash をしてから実行する"; exit 1; }
	@git fetch --quiet origin main
	@test "$$(git rev-parse HEAD)" = "$$(git rev-parse origin/main)" \
	  || { echo "HEAD が origin/main と違う。main を最新にしてから実行する（手元の枝を見るなら 'make dev'）"; exit 1; }

# 「そろそろリリースするか」の 1 コマンド。brew で入るものと同じ release ビルドを
# 起こす（`make dev` の debug ビルドは速さのためのもので、配るものではない）。
# brew 常駐は `brew-pause` が退避するので二重に出ない。
.PHONY: preview
preview: main-sync ## main のコードを production と同じ形で起動して目視する（brew 常駐は退避）
	-@pkill -f '$(DAEMON_DEV)' 2>/dev/null || true
	-@pkill -f '$(DAEMON_PRE)' 2>/dev/null || true
	-@launchctl bootout $(AGENT) 2>/dev/null || true
	@$(MAKE) --no-print-directory brew-pause
	@echo "release ビルド中（初回は数分）…"
	@$(CARGO) build -q --release -p ccsessionsd -p ccsessions
	@$(DAEMON_PRE) >/tmp/ccsessionsd-preview.log 2>&1 &
	@echo ""
	@echo "  $$(git log -1 --format='%h %s') を production と同じ形で起動した。"
	@echo "  設定と顔:  target/release/ccsessions ui"
	@echo "  導入状況:  target/release/ccsessions doctor"
	@echo "  log:       /tmp/ccsessionsd-preview.log"
	@echo ""
	@echo "  OK なら:   make release VERSION=x.y.z （preview を畳んで Release PR を出す）"
	@echo "  やめる:    make stop                  （brew 常駐が戻る）"

# `make release` は **PR を出すところで止まる**。タグを打つのも tap を更新するのも
# `.github/workflows/release.yml` で、引き金は「この PR がマージされたこと」。
# 人間が `main` へ直接書く操作をリリース手順から無くすため（docs/adr/0025）。
#
# sed は BSD 版（`-i ''`）。このリポジトリは macOS 専用なので揃えてある。
.PHONY: release
release: ## Release PR を出す（make release VERSION=x.y.z）。タグは CI が打つ
	@test -n "$(VERSION)" \
	  || { echo "VERSION が要る（例: make release VERSION=0.1.2）"; exit 1; }
	@printf '%s' '$(VERSION)' | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+$$' \
	  || { echo "VERSION は x.y.z の 3 つ組で書く（'v' は付けない）: '$(VERSION)'"; exit 1; }
	@command -v gh >/dev/null 2>&1 || { echo "gh が無い: brew install gh"; exit 1; }
	@# preview を畳んで production を戻してからリリースに入る。ここでやるのは
	@# 「`make preview` して OK だったので出す」が 1 本の流れになるようにするため。
	@# 版の書式が間違っているときは畳む前に落としたいので、この位置に置いてある。
	-@pkill -f '$(DAEMON_PRE)' 2>/dev/null || true
	@$(MAKE) --no-print-directory brew-resume
	@# リリースブランチは origin/main から生やす。ここがずれていると、まだ
	@# レビューされていない変更をリリースに巻き込む（`preview` と同じ前提）。
	@$(MAKE) --no-print-directory main-sync
	@if git ls-remote --exit-code --tags origin 'refs/tags/v$(VERSION)' >/dev/null 2>&1; then \
	  echo "タグ v$(VERSION) は既にある。上げる版を指定する"; exit 1; \
	fi
	@if grep -q '^version = "$(VERSION)"$$' $(CARGO_TOML); then \
	  echo "$(CARGO_TOML) は既に $(VERSION)。上げる版を指定する"; exit 1; \
	fi
	@git switch -q -c release/v$(VERSION)
	@sed -i '' -E 's/^version = "[0-9]+\.[0-9]+\.[0-9]+"$$/version = "$(VERSION)"/' $(CARGO_TOML)
	@sed -i '' -E 's/"version": "[0-9]+\.[0-9]+\.[0-9]+"/"version": "$(VERSION)"/' $(PLUGIN_JSON)
	@# 置換できたことを読み直して確かめる。ファイルの構造が変わったのに sed が
	@# 黙って何もせず、版が上がらないまま Release PR が出る事故を防ぐ。
	@grep -q '^version = "$(VERSION)"$$' $(CARGO_TOML) \
	  || { echo "$(CARGO_TOML) の version を書き換えられなかった（[workspace.package] の形が変わった？）"; exit 1; }
	@grep -q '"version": "$(VERSION)"' $(PLUGIN_JSON) \
	  || { echo "$(PLUGIN_JSON) の version を書き換えられなかった"; exit 1; }
	@# workspace の 3 エントリを Cargo.lock に追従させる。**`--locked` を付けない
	@# 唯一の場所**（ci.yml も release.yml の guard も付ける側）。
	@$(CARGO) metadata --format-version 1 >/dev/null
	@git add $(CARGO_TOML) $(PLUGIN_JSON) Cargo.lock
	@git commit -q -m "release: v$(VERSION)"
	@git push -q -u origin release/v$(VERSION)
	@gh pr create --title "release: v$(VERSION)" --body "$$(printf '%s\n' \
	  'この PR をマージするとリリースが走ります（.github/workflows/release.yml）。' \
	  '' \
	  '1. CI がタグ v$(VERSION) を打つ' \
	  '2. tarball の sha256 を取って formula をレンダリングし、実際に brew install と brew test を通す' \
	  '3. 緑のときだけ tap（S-Nakamur-a/homebrew-tap）を更新して GitHub Release を作る' \
	  '' \
	  '人間がタグを打つ操作はありません。途中で失敗したらタグは消えるので、直して同じ版でやり直せます。')"
	@echo "Release PR を出した。CI が緑なのを見てマージすると v$(VERSION) が公開される。"

# ---------------------------------------------------------------------------

.PHONY: help
help: ## このヘルプ
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) \
	  | awk 'BEGIN {FS = ":.*?## "}; {printf "  \033[36m%-10s\033[0m %s\n", $$1, $$2}'
