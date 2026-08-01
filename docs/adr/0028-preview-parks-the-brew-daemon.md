# 0028 · 常駐の入口は brew ひとつ。手元のコードを起こすときは退避する（bootout で）

採用 · 2026-08-01

## 文脈

`brew services start ccsessions` で常用しているマシンで、手元のコードを動かすと
**生き物が二重に出る**。`make dev` / `make demo` が止めるのは当時の `make start` が
置く LaunchAgent（`dev.ccsessions.ccsessionsd`）と `target/debug/ccsessionsd` だけで、
brew 由来の `homebrew.mxcl.ccsessions` は対象外。
`make stop` の `pkill` も `~/.cargo/bin/ccsessionsd` を狙うので当たらないし、仮に
当たっても formula は `keep_alive true` なので launchd が即座に蘇らせる。

回したいフローは短い。

1. `main` に PR がどんどん入る
2. 「そろそろリリースするか」→ **1 コマンド**で `main` のコードを production と
   同じ形で起こして目視する（brew の production と重複しない）
3. OK → **1 コマンド**で手元のものを畳んでリリースに入る

[2026-07-30 の計画](../plans/2026-07-30-brew-dev-coexistence.md)は「**検出して落とす**」
（brew 常駐が動いていたら `make dev` が止まり、次に打つべき 1 行を出す）を採っていた。
状態を持たない良さはあるが、**上のフローでは人間の手数が減らない** — 止める・戻すを
毎回自分で打つことになる。

## 決定

### 常駐の入口は brew ひとつだけにする

Makefile から常駐させる導線 — `install` / `plist` / `start` / `restart` / `deploy` /
`uninstall` の 6 つ（約 60 行）— を廃止した。brew 配布より前の「手元で production を
常駐させる」経路で、[0021](0021-distribution.md) 以降は役割が無い。

**入口が 2 つあること自体が、この ADR が直している問題の原因である。** `doctor` の
`RESIDENCIES` はこの衝突を検出するために在り、実際に開発機で警告が出ていた
（`dev.ccsessions.ccsessionsd` の plist だけが残り、指す先の
`~/.cargo/bin/ccsessionsd` は既に無い、という状態）。

手元のコードを見る用途は `make dev`（debug）と `make preview`（release）が引き取る。
どちらも LaunchAgent ではなく**セッションのプロセス**として起こすので、ログアウトで
消える。常駐させたいなら brew を使う。

`doctor` の検出は**残す**（`origin` を "make start, which no longer exists" に変えた）。
導線を消しても、既に置かれた plist は消えない。

### 手元のコードを起こす側が退避し、畳む側が戻す

| 側 | ターゲット | すること |
|---|---|---|
| 起こす | `dev` / `demo` / `preview` | `brew-pause`（brew 常駐が動いていれば退避） |
| 畳む | `stop` / `release` | `brew-resume`（退避してあれば戻す） |

`make preview` を足した。`main-sync`（作業ツリーが clean・`HEAD` が `origin/main`）を
前提にして、**release ビルド**を起こす。`make dev` の debug ビルドは速さのためのもので
配るものではないので、リリース直前の目視は brew で入るのと同じ最適化で見る。

### 退避は `launchctl bootout`。`brew services` の 2 つはどちらも使えない

**`brew services kill` は使えない。** `keep_alive true` の service を Homebrew は
拒む（`Service 'ccsessions' is set to automatically restart and can't be killed.`）。
しかも**終了コードは 0** なので、呼んだ側は成功したと思い込む。実際に実装して踏んだ
（＝**黙って二重に出る**）。

**`brew services stop` も使わない。** 止まりはするが `~/Library/LaunchAgents/` から
plist ごと消す。これは 2 つの意味で困る。

1. **退避したのか、ユーザが自分で止めたのかが区別できなくなる。** 戻す条件が書けない。
2. **戻し忘れが恒久化する。** ログインし直しても復活しないので、常用のオーバーレイが
   黙って消えたままになる。原因が数日前に打った `make dev` だと気づける人はいない。

`launchctl bootout gui/$(id -u)/homebrew.mxcl.ccsessions` は**プロセスを止めて plist を
残す**。ここから 2 つ手に入る。

- **「plist は在るのに service が居ない」＝退避中**、という判定がそのまま書ける。
  ユーザが `brew services stop` した場合は plist が無いので、こちらは何もしない
  （＝**ユーザが明示的に止めたものを勝手に起こさない**。[0021](0021-distribution.md) の流儀）。
- **戻し忘れても次のログインで launchd が読み直す。** 恒久的に消える経路が無い。

これは Makefile が自前の LaunchAgent（`dev.ccsessions.ccsessionsd`）に既に使っている
手口と同じで、道具を増やしていない。

## 採らなかった案

**検出して落とす**（2026-07-30 の計画）。手数が減らない。上のフローの「1 コマンドで
起こす／1 コマンドで畳む」に対して、毎回 2 コマンド増える。この ADR で差し替える。

**状態ファイルで退避を記録する。** `brew services stop` を使う場合は必要になるが、
plist の有無がそのまま状態なので**持たなくて済む**。壊れたときの症状が
「オーバーレイが出ない」で原因が Makefile の残骸、という追いにくい形も避けられる。

**daemon 側に「休止」を持たせる**（config に `paused` を足し、走行中の常駐が自分で
引っ込む）。プロセスを殺さないので最も行儀が良いが、**すでに配ってある版はそのキーを
知らない**（serde が未知キーを捨てる）ので、効くのは次の次の版から。今の問題は今の
マシンで起きている。

## 結果

`make dev` / `make demo` / `make preview` を打った時点で二重に出なくなった。
`ccsessions doctor` の二重常駐の検出は**事後の網**として残す（Makefile を通さずに
手で起こす道は塞いでいない）。

戻すのは `make stop` と `make release`。**どちらも打たずにターミナルを閉じた場合**は
次のログインで production が復活する（bootout の性質）。その間 production の
オーバーレイは出ないが、これは「手元のコードを起こしている最中」と同じ状態で、
二重に出るよりは静かでない壊れ方になっている。
