# 0027 · リリースは Release PR のマージが引き金。タグは CI が打つ

採用 · 2026-07-29

[0021](0021-distribution.md) が配布の形（source formula + 自前 tap）を決め、実際に
配り終えた。あちらが最後に「**リリース手順は 4 手**」と書いた手作業を、ここで機械に
渡す。配布の形そのものは変えない。

## 文脈

[0021](0021-distribution.md) のリリース手順はこうだった — タグを切る → tarball の
`sha256` を取る → `packaging/homebrew/ccsessions.rb` の `url` / `sha256` を差し替える
→ tap の `Formula/ccsessions.rb` へコピー。全部人間がやる。残っていた穴は 2 つ。

| | 何が起きるか |
|---|---|
| **formula が壊れても気づけない** | `ci.yml` は Rust の品質ゲートだけで formula に一切触れていない。壊れた formula は**ユーザが `brew install` して初めて**発覚する |
| **GitHub Release が 1 つも無い** | タグだけがある。リリースノートも配布物の記録も残っていない |

目指す状態は「人間は `make release VERSION=x.y.z` の 1 コマンドだけ。**実際に
`brew install` が通った formula しか tap に載らない**」。

## 決定

1. **`make release VERSION=x.y.z` は Release PR を出すところで止まる。**
   版を上げて（`Cargo.toml` と `plugin.json`）`Cargo.lock` を追従させ、
   `release/vx.y.z` ブランチを push して PR を作るところまで。
2. **引き金はその PR がマージされること。** `release.yml` は `push: branches: [main]`
   で起動し、`Cargo.toml` の版に対応するタグが無ければリリースと判定する。
3. **タグは人間も `make` も打たない。CI が打つ。**
4. **タグ打ちから公開までを 1 本のワークフローで完結させる。** タグ push 起動の
   別ワークフローには分けない。
5. **tap を動かす前に、実際に `brew install` と `brew test` を通す。**
   通らなければ tap は 1 バイトも動かない。
6. **`main` へは差し戻さない。** tap の formula が唯一の実体で、
   `packaging/homebrew/ccsessions.rb` は**レビュー用の写し**に格下げする。
7. 写しと tap がずれていないことは **PR 時の CI** が見る。

## 理由

### なぜ Release PR のマージが引き金か（＝ なぜ人間もタグを打たないか）

初版は `make release` が `main` へ直 push してタグを打つ形だった。これだと
**リリースのたびに `main` の ruleset（承認 1 件必須）を人間が迂回する**ことになる。
Release PR にすれば、人間が触るのは「`make release` を実行する」「出てきた PR を
マージする」の 2 つだけで、**どちらも ruleset の中で完結する**。release-plz や
release-please が同じ形を採っているのもこの理由。

**タグ ref には ruleset が掛からない**（ruleset は `target: branch`）ので、CI が
`GITHUB_TOKEN` でタグを打つのに bypass actors も GitHub App も要らない。「人間は
`main` に直接書けない、CI もブランチには書かない、タグだけが自動で増える」という
形に収まる。

### なぜタグ起動の別ワークフローに分けないか

**`GITHUB_TOKEN` で打ったタグは他のワークフローを起こさない**（再帰を防ぐための
GitHub の仕様）。「CI がタグを打つ」→「タグ push でリリースワークフローが起動する」
という分け方をすると、**リリースが永遠に始まらない**。しかも何のエラーも出ない。
1 本にまとめてあるのはこれを踏まないため。

### なぜ `main` へ差し戻さないか

初版は「tap へ push したあと、同じ 2 行を `main` へも差し戻しコミット」だった。
捨てた理由は 2 つ。

1. **`main` の ruleset に当たる。** 回避するには GitHub App を作って bypass actors に
   登録する必要がある（`GITHUB_TOKEN` は bypass actor になれない）。
   **配布の自動化のために、リポジトリの保護に穴を開けることになる。**
2. **そもそもアプリ側リポジトリに formula の原本を置くのが主流ではない。** tap の
   formula が唯一の実体で、CI はそこだけ更新する。差し戻すという手順自体が無い。

写しをこのリポジトリに残すのは [0021](0021-distribution.md) と同じ理由
（formula の変更を ADR と一緒にレビューしたい）で、**ずれの検出は CI に持たせる**。

### なぜ `brew bump-formula-pr` ではなく自前レンダリングか

tap 更新の定番は
[dawidd6/action-homebrew-bump-formula](https://github.com/dawidd6/action-homebrew-bump-formula)
と [mislav/bump-homebrew-formula-action](https://github.com/mislav/bump-homebrew-formula-action)
だが、**どちらも「tap に書いてから」しか検証できない**。この自動化で一番買いたいのは
**tap を動かす前に実際に `brew install` を通す**という順序なので、レンダリングは
`url` / `sha256` の 2 行の sed に留めて、順序をこちらで持つ。

Rust 向けの [`dist`（旧 cargo-dist）](https://github.com/axodotdev/cargo-dist) は
tap 公開まで一括で面倒を見てくれるが、axo が商業撤退済みでコミュニティ維持のため、
配布経路の根幹には置かない。

### `TAP_TOKEN` の期限切れはリリースだけを静かに止める

tap への push には fine-grained PAT（`S-Nakamur-a/homebrew-tap` の Contents:
Read and write **だけ**）が要る。期限切れや剥奪が起きると `release.yml` の tap
checkout が落ちる。**そのとき壊れるのはリリースだけ**で、既に配ってあるものには
何も起きない（tap は前回のまま、ユーザの `brew install` も `brew upgrade` も通る）。
`main` の保護を越える権限は持たせていないので、この PAT が漏れてもリポジトリ本体は
守られる。将来 bot が `main` を触る必要が出たら、PAT ではなく GitHub App +
`actions/create-github-app-token` + ruleset の bypass actors が定石。

## 影響

- **版を持つファイルは `Cargo.toml` と `plugin.json` の 2 つ。** `release.yml` の
  `guard` ジョブが**毎リリース一致を検証する**。`#1` で踏んだ版ずれの再発防止で、
  版を持つファイルを増やすときは guard も一緒に動かす。
- **`Cargo.lock` を更新する場所は `make release` の `cargo metadata` だけ。**
  `ci.yml` も `release.yml` の guard も `--locked` を付ける側で、
  「lock が古ければ落とす」を保つ。
- **`ci.yml` に formula ジョブが増えた。** `brew style` + `brew audit --strict`
  （`--online` 無し）と、tap とのずれ検査。ruby の構文崩れを**リリース当日ではなく
  レビュー時**に落とす。`main` の ruleset の required status checks にも登録した
  （赤いままマージできると、ずれ検査を機械に握らせた意味が無くなるため）。**照合は
  ジョブ名の完全一致**なので、`name:` を変えるときは ruleset も一緒に直す。
- **`brew audit` / `brew style` はパス指定では呼べない。** Homebrew は
  `brew audit <path>` を無効化していて（"Calling `brew audit [path ...]` is
  disabled!"）、`brew style <path>` はパスだと formula ではなくただの ruby として
  見るので、Sorbet sigil や `frozen_string_literal` という formula には無関係な
  指摘を出す。**使い捨ての tap（`brew tap-new --no-git`）に置いて名前で呼ぶ**のが
  唯一の通し方で、`ci.yml` も `release.yml` もそうしている。
- **ずれ検査はコメントを比較から外す。** 写しの側にだけ「これはレビュー用の写しで
  手で直さない」と書く必要があり、そこは意図的にずれる（tap にとってその説明は嘘に
  なる）。比較するのは formula の実体（依存・install・service・caveats・test）で、
  ここがずれたら brew の挙動が変わる。`url` / `sha256` も tap 側が正なので外す。
- **sed が 0 行置換でも成功で終わることを前提に検査を入れてある。** レンダリングは
  置換の前に「対象の行が在る」ことを、後に「新しい値が入っている」ことを見る。
  これが無いと、formula の構造が変わったときに**古い `url` のまま tap に載る**。
- **リリース途中で失敗したら、打ったタグを消す。** そのタグはまだ誰にも参照されて
  いない（Release も tap も動いていない）ので、消せば直して**同じ版で**やり直せる。
  消さないと patch を 1 つ空費する。**ただし tap を更新したあとは消さない** —
  消すと tap が存在しないタグを指し、ユーザの `brew install` が壊れる。この一点だけ
  「タグを残して人間に知らせる」に倒してある。
- **`release.yml` は同時に 2 本走らせない**（`concurrency: release`）。同じ版で
  並走すると後発の `git push` がタグの重複で失敗し、**その失敗の掃除が先発の打った
  タグを消す**。直列にすれば後発の `detect` が「タグはもうある」を見て何もしない。
- **リハーサルができる。** `workflow_dispatch` の `dry_run`（既定 true）で、
  **既存の最新タグの tarball に対して**レンダリングと `brew install` / `brew test`
  だけを走らせる。タグも tap も Release も変わらない。次に出す版のタグはまだ無く
  tarball も落とせないので、リハーサルが見るのは Cargo.toml の版ではなく既存のタグ。
- **コミットメッセージは判定に使わない。** `release:` と書き忘れても書き間違えても
  壊れないように、判定材料は「`Cargo.toml` の版に対応するタグが無いこと」だけ。
- **bottle は配らない**（[0021](0021-distribution.md) 据え置き）。ユーザの
  `brew install` は毎回 `cargo build` する。裏返すと `release.yml` の検証も毎回
  フルビルドで、リリース 1 回に十数分かかる。

## 未決（この ADR は決めない）

- **`main` の ruleset の承認 1 件必須をどうするか。** 単独メンテナは自己承認できない
  ので、このままだと Release PR のマージも `--admin` になる。実配置を見ると bypass
  actors に **admin ロールが `bypass_mode: pull_request` で入っている**
  （`current_user_can_bypass: pull_requests_only`）ので、直 push は塞いだまま PR 経由
  でだけ抜けられる。ただし**抜けるには明示的に行使する必要がある** — `gh pr merge`
  なら `--admin`、UI なら「要件を待たずにマージ」。この PR 自身もそうやって入れた。
  世間の solo リポジトリは「承認 0 件 + required status checks + 直 push 禁止」が
  普通で、そちらに寄せれば `--admin` は要らなくなる。**この決定はどちらでも成立する**
  （承認が要るかは人間のマージ操作の手数の問題で、自動化の側は変わらない）。
