# ccsessions の Homebrew formula。
#
# **実体は tap（`S-Nakamur-a/homebrew-tap` の `Formula/ccsessions.rb`）に置く。**
# このファイルはその原本で、リリースのたびに `url` と `sha256` を差し替えて
# tap へコピーする。原本をこのリポジトリに置くのは、formula の変更が
# 「なぜそう書いたか」（＝ ADR）と一緒にレビューできるようにするため。
#
# 方針は [docs/adr/0021-distribution.md]。要点だけ再掲する。
#
# - **source formula にする。** ダウンロードした .app ではないので
#   `com.apple.quarantine` が付かず、Apple Developer Program（公証）が要らない。
#   Apple Silicon で必須の ad-hoc 署名は、macOS 上で `cargo build` すれば
#   リンカが自動的に付ける。
# - **prefix の外を一切触らない。** `~/.claude/settings.json`（hook 配線）も
#   `~/Library/LaunchAgents/`（常駐化）も formula は書かない。前者は他人の
#   エディタ設定、後者は `brew services` の担当で、どちらもユーザが明示的に
#   起こす操作。`caveats` で案内するに留める。
class Ccsessions < Formula
  desc "Menu bar overlay showing running Claude Code sessions as a flock of creatures"
  homepage "https://github.com/S-Nakamur-a/ccsessions"
  url "https://github.com/S-Nakamur-a/ccsessions/archive/refs/tags/v0.1.0.tar.gz"
  # タグを push したあとに差し替える:
  #   curl -sL https://github.com/S-Nakamur-a/ccsessions/archive/refs/tags/v0.1.0.tar.gz | shasum -a 256
  sha256 "96865d6ccc23da649c0a775dcccf7faca7589b08ffe5e168d50795af1cd7e860"
  license "MIT"
  head "https://github.com/S-Nakamur-a/ccsessions.git", branch: "main"

  depends_on "rust" => :build
  # `ccsessionsd` は AppKit / CoreAnimation を直に叩くので macOS 専用。
  # objc2 依存を target 修飾していないため、他 OS ではそもそもビルドが通らない。
  depends_on :macos

  def install
    # 3 crate の workspace なので member を名指しする。`std_cargo_args` は
    # `--locked` を付ける ＝ **コミット済みの Cargo.lock でビルドが固定される**
    # （lock をコミットしてあるのはこのため）。
    system "cargo", "install", *std_cargo_args(path: "ccsessions")
    system "cargo", "install", *std_cargo_args(path: "ccsessionsd")
    # 顔（`faces/*.toml`）と設定 UI の HTML/JS/CSS は `include_str!` で
    # バイナリに焼き込んである。別途 install するデータファイルは無い。
  end

  service do
    run [opt_bin/"ccsessionsd"]
    keep_alive true
    # **ログを /tmp に置かない。** /tmp は全ユーザ共有で、他人が先に同名の
    # symlink を置けば launchd が*こちらのユーザ権限で*その先へ追記する。
    # 再起動で消え 3 日で掃除される点も、常駐の障害調査には向かない。
    # `Makefile` の LOGDIR と揃えてある。
    #
    # 親ディレクトリ（`~/Library/Logs/ccsessions`）は `brew services start` が
    # 作る（Homebrew の `Service#path_dirs` が log_path の親を集め、
    # `services/cli.rb` が `mkpath` する）。**formula が prefix の外へ
    # mkdir する必要はない**ので、上の「prefix の外を触らない」と両立する。
    log_path "#{Dir.home}/Library/Logs/ccsessions/ccsessionsd.log"
    error_log_path "#{Dir.home}/Library/Logs/ccsessions/ccsessionsd.err"
  end

  def caveats
    <<~EOS
      hook の配線と常駐の開始は、どちらも自動では行いません（あなたの
      settings.json と LaunchAgents は、あなたが明示的に操作したときだけ
      変わります）。

      1) hook を入れる — Claude Code の中で:

           /plugin marketplace add S-Nakamur-a/ccsessions
           /plugin install ccsessions@ccsessions-marketplace

         settings.json に入るのは enabledPlugins の 1 行だけで、hooks
         セクションには触りません。

      2) 常駐を開始する:

           brew services start ccsessions

      設定と顔作りの GUI:   ccsessions ui
      導入状況の確認:       ccsessions doctor

      `make start` で入れた LaunchAgent が残っていると生き物が二重に出ます。
      `ccsessions doctor` が検出します。
    EOS
  end

  test do
    # hook の不変条件（ADR 0004）＝ **必ず exit 0 で、stdout に何も書かない**。
    # 破ると Claude Code 側で権限拒否やプロンプト消失として現れるので、
    # 壊れた入力でも守られることをここで見る。
    assert_empty pipe_output("#{bin}/ccsessions hook", "not json at all", 0)

    # help は hook の入れ方（プラグイン）を案内する。
    assert_match "/plugin install", shell_output("#{bin}/ccsessions help")

    # daemon はバイナリが入っていることだけ確かめる。起動は GUI セッションを
    # 要求するので、sandbox のテストでは走らせない。
    assert_path_exists bin/"ccsessionsd"
  end
end
