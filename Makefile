# ccsessions — dev / deploy helpers
#
# 設定・顔作り:
#   `make config`  … Web UI（127.0.0.1:8787）。**設定の入口はここだけ**
#                    （メニューバーの status item は廃止した）
#
# 見た目イテレーション（一番よく使う）:
#   ccsessionsd/src/theme.rs の定数をいじる → `make dev` → 画面で確認 → 繰り返す
#
# 使う（開発ではなく常用する）:
#   `make start`   … 導入済みの ccsessionsd を常駐開始する。**ビルドしない**
#   `make stop`    … 止める
#   `make deploy`  … ビルドして入れ直し、常駐を入れ替える（更新の反映）

CARGO      ?= cargo
UID        := $(shell id -u)
# LaunchAgent のラベル。plist のファイル名（$(PLIST)）もこれで決まる。
# **変えるなら「古いラベルで stop → plist を消す → 新ラベルで start」の順**。
# 先に書き換えると `make stop` が新ラベルを探しにいき、古いエージェントが
# 生き残って生き物が二重に出る。
LABEL      := dev.ccsessions.ccsessionsd
AGENT      := gui/$(UID)/$(LABEL)
PLIST      := $(HOME)/Library/LaunchAgents/$(LABEL).plist
# 常駐（LaunchAgent）のログの置き場。**`/tmp` には置かない**。macOS の `/tmp` は
# 全ユーザ共有で、他人が先に同名の symlink を作っておけば launchd が**こちらの
# 権限で**その先へ追記してしまう。加えて再起動で消え、3 日で掃除され、Console.app
# からも辿れない。`~/Library/Logs/<name>/` が macOS の標準的な期待。
# （`make dev` の `/tmp/ccsessionsd-dev.log` は開発用の使い捨てなのでそのまま）
LOGDIR     := $(HOME)/Library/Logs/ccsessions
DAEMON     := $(HOME)/.cargo/bin/ccsessionsd
DAEMON_DEV := target/debug/ccsessionsd
CLI        := $(HOME)/.cargo/bin/ccsessions
# **hook の配線ターゲットはここには無い。** settings.json に書くのは Claude Code
# プラグイン（plugins/ccsessions/）の仕事で、ccsessions は他人の設定ファイルを
# 一切書き換えない（docs/adr/0021-distribution.md）。開発中に配線するなら
# Claude Code の中で `/plugin marketplace add .` を使う。

.DEFAULT_GOAL := help

# ---------------------------------------------------------------------------
# 見た目の高速ループ
# ---------------------------------------------------------------------------

.PHONY: dev
dev: ## 走行中の daemon を止め→build→dev binary を起動（install 不要）
	-@pkill -f '$(DAEMON_DEV)' 2>/dev/null || true
	-@launchctl bootout $(AGENT) 2>/dev/null || true
	@$(CARGO) build -q -p ccsessionsd
	@$(DAEMON_DEV) >/tmp/ccsessionsd-dev.log 2>&1 &
	@echo "ccsessionsd(dev) 起動。theme.rs を直して 'make dev' を繰り返す。log: /tmp/ccsessionsd-dev.log"

.PHONY: demo
demo: ## 6 状態のダミーセッションを出して見た目を確認する（実セッション不要）
	-@pkill -f '$(DAEMON_DEV)' 2>/dev/null || true
	-@launchctl bootout $(AGENT) 2>/dev/null || true
	@$(CARGO) build -q -p ccsessionsd
	@$(DAEMON_DEV) --demo >/tmp/ccsessionsd-dev.log 2>&1 &
	@echo "ccsessionsd(demo) 起動。log: /tmp/ccsessionsd-dev.log"

.PHONY: config
config: ## 設定と顔作りの Web UI を立ち上げる（127.0.0.1:8787）。設定の入口はここだけ
	@$(CARGO) build -q -p ccsessions
	@target/debug/ccsessions ui

.PHONY: watch
watch: ## ソース保存のたびに自動で make dev（要 watchexec）
	@command -v watchexec >/dev/null 2>&1 || { echo "watchexec 未導入: brew install watchexec"; exit 1; }
	watchexec -e rs -w ccsessionsd/src -- $(MAKE) dev

.PHONY: stop
stop: ## 走行中の ccsessionsd（dev / LaunchAgent 両方）を止める
	-@pkill -f '$(DAEMON_DEV)' 2>/dev/null || true
	-@pkill -f '$(DAEMON)' 2>/dev/null || true
	-@launchctl bootout $(AGENT) 2>/dev/null || true
	@echo "stopped"

# ---------------------------------------------------------------------------
# 品質ゲート
# ---------------------------------------------------------------------------

.PHONY: check
check: ## fmt --check + clippy -D warnings + test
	$(CARGO) fmt --all -- --check
	$(CARGO) clippy --all-targets -- -D warnings
	$(CARGO) test

.PHONY: fmt
fmt: ## cargo fmt
	$(CARGO) fmt --all

.PHONY: test
test: ## 全テスト（unit + 統合）
	$(CARGO) test

.PHONY: unit-test
unit-test: ## unit テストだけ（各 crate の #[cfg(test)]。速い）
	$(CARGO) test --lib --bins

# 実プロセスを起動して hook の契約（exit 0・stdout 無言・並列書き込み）を見るテスト。
# ライブラリの単体テストでは再現できない性質なので分けてある。
.PHONY: integration-test
integration-test: ## CLI 統合テスト（ccsessions/tests/cli.rs。実プロセスを起動する）
	$(CARGO) test -p ccsessions --test cli

# ---------------------------------------------------------------------------
# インストール / 常駐
# ---------------------------------------------------------------------------

.PHONY: install
install: ## release ビルドして ~/.cargo/bin へ入れる
	$(CARGO) install --path ccsessions --force
	$(CARGO) install --path ccsessionsd --force

.PHONY: plist
plist: ## LaunchAgent の plist を書き出す（常時起動・ログイン時起動）
	@mkdir -p $(HOME)/Library/LaunchAgents
	@# launchd は**ディレクトリを作らない**。無ければリダイレクトが黙って失敗し、
	@# ログの出ない daemon になる。plist と同時に必ず用意する。
	@mkdir -p $(LOGDIR)
	@printf '%s\n' \
	  '<?xml version="1.0" encoding="UTF-8"?>' \
	  '<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">' \
	  '<plist version="1.0">' \
	  '<dict>' \
	  '  <key>Label</key><string>$(LABEL)</string>' \
	  '  <key>ProgramArguments</key><array><string>$(DAEMON)</string></array>' \
	  '  <key>RunAtLoad</key><true/>' \
	  '  <key>KeepAlive</key><true/>' \
	  '  <key>StandardOutPath</key><string>$(LOGDIR)/ccsessionsd.log</string>' \
	  '  <key>StandardErrorPath</key><string>$(LOGDIR)/ccsessionsd.err</string>' \
	  '</dict>' \
	  '</plist>' > $(PLIST)
	@echo "wrote $(PLIST)"

# start は**ビルドしない**。使いたいだけの人が daemon を起こすたびに release
# ビルドの数十秒を払うのはおかしいので、ビルドを伴う導線は install / deploy に
# 分けてある（deploy = install + restart）。plist の書き出しは瞬時で冪等なので
# start 側に置き、消えていても自力で直せるようにしてある。
.PHONY: start
start: plist ## 導入済みの ccsessionsd を常駐開始する（ビルドしない）
	@test -x $(DAEMON) \
	  || { echo "$(DAEMON) が無い。先に 'make install'（or 'make deploy'）を実行する"; exit 1; }
	@if launchctl print $(AGENT) >/dev/null 2>&1; then \
	  echo "ccsessionsd はもう稼働中（バイナリを入れ替えるなら 'make deploy'）"; \
	else \
	  launchctl bootstrap gui/$(UID) $(PLIST) \
	    || { echo "bootstrap に失敗した。'launchctl print $(AGENT)' で状態を見る"; exit 1; }; \
	  sleep 1; \
	  launchctl print $(AGENT) >/dev/null 2>&1 \
	    && echo "ccsessionsd を常駐させた。ログ: $(LOGDIR)/ccsessionsd.log" \
	    || { echo "ccsessionsd が起動していない。ログ: $(LOGDIR)/ccsessionsd.err"; exit 1; }; \
	fi

# launchctl の bootout は**非同期**で、直後に bootstrap すると
# "Bootstrap failed: 5: Input/output error" で失敗し、daemon が居ないまま
# 静かに終わる（実際に踏んだ）。サービスが本当に消えるまで待ってから起こす。
.PHONY: restart
restart: ## 常駐を入れ替える（bootout の完了を待ってから start）
	-@launchctl bootout $(AGENT) 2>/dev/null || true
	@for i in 1 2 3 4 5 6 7 8 9 10; do \
	  launchctl print $(AGENT) >/dev/null 2>&1 || break; \
	  sleep 0.3; \
	done
	@$(MAKE) --no-print-directory start

.PHONY: deploy
deploy: install plist ## ビルドして入れ直し、常駐を入れ替える（更新の反映）
	@$(MAKE) --no-print-directory restart

.PHONY: uninstall
uninstall: stop ## 常駐解除 + plist 削除（hook はプラグイン側なので触らない）
	-@rm -f $(PLIST)
	@echo "uninstalled（バイナリは ~/.cargo/bin に残る: cargo uninstall ccsessions ccsessionsd）"
	@echo "hook を外すには Claude Code で /plugin uninstall ccsessions@ccsessions-marketplace"

# ---------------------------------------------------------------------------
# リリース
# ---------------------------------------------------------------------------

# 版を持っているファイルはこの 2 つだけ。**`release.yml` の guard ジョブが
# 「この 2 つが同じ版であること」を毎リリース検証する**ので、増やすときは
# あちらも一緒に動かす。
CARGO_TOML  := Cargo.toml
PLUGIN_JSON := plugins/ccsessions/.claude-plugin/plugin.json

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
	@test -z "$$(git status --porcelain)" \
	  || { echo "作業ツリーが汚れている。commit か stash をしてから実行する"; exit 1; }
	@# リリースブランチは origin/main から生やす。ここがずれていると、まだ
	@# レビューされていない変更をリリースに巻き込む。
	@git fetch --quiet origin main
	@test "$$(git rev-parse HEAD)" = "$$(git rev-parse origin/main)" \
	  || { echo "HEAD が origin/main と違う。main を最新にしてから実行する"; exit 1; }
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
