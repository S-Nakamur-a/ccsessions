# 0002 · セッションは 1 ファイル。書き手はストア全体を排他する

採用 · 2026-07-25 / 排他の追加 2026-07-26

## 文脈

複数の Claude Code プロセスが並行にセッション状態を書く。保存先の候補は
「全セッションを 1 つの JSON に持つ」か「1 セッション 1 ファイル」。

## 決定

`~/.local/state/ccsessions/sessions/<session_id>.json` に 1 セッション 1 ファイル、
atomic write（tmp へ書いて rename）で保存する。

さらに**書き手は `store::lock_exclusive()` を `load` から `save`/`remove` まで
保持する**。

## 理由

- 単一 JSON にすると、別セッションどうしの更新が read-modify-write の後勝ちで
  消える。ファイルを分ければセッション間は干渉しない。
- **同一セッション内では分けても足りなかった。** Claude Code は
  **マッチした hook をすべて並列プロセスで実行する**ので、1 つのセッションに対して
  複数の `ccsessions hook` が同時に `load → reduce → save` を回す。`write_atomic` が
  保証するのは「途中状態を読み手に晒さない」ことだけで、read-modify-write 全体では
  ない（実測: サブエージェントを 8 並列で起動したとき `agents` が 2 件しか残らなかった）。
- ロックを `load`/`save` の中に隠さないのは、**reduce を挟む区間を守れないから**。
  排他は書き手側の規律にしてある。

## 影響

- 書き手は `ccsessions hook` / `ccsessions set` と、ファイルを消す `store::sweep`。
- **読み手（daemon の poller）はロックを取らない。** rename の atomicity により、
  古い内容か新しい内容のどちらかしか見えない。
- 番人は `ccsessions/tests/cli.rs::parallel_hook_processes_do_not_lose_agents`
  （実プロセスを 8 個同時に起動する）。

## 撤回条件

セッション数が 100 を超えて列挙が重くなる（実測 5ms 超）なら索引を足す。
