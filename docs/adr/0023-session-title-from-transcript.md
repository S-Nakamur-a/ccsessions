# 0023 · セッションタイトルは transcript から読む

採用 · 2026-07-28

## 文脈

生き物の名札は cwd の basename（`Session::name`）で、ホバーカードの見出しも同じ。
**同じディレクトリで 2 セッション走らせると、見た目が完全に一致する。** ストアは
session_id ごとに別ファイルなので内部では区別できているが、画面上の手掛かりは
状態色・経過時間・並び位置だけで、「どっちがどの端末か」は分からない。

Claude Code は各セッションにタイトルを持っている（`/resume` の一覧に出るあれ）。
これを出せれば区別がつく。ところが **hook payload にタイトルは入っていない** —
`session_id` / `transcript_path` / `cwd` などしか来ない（[0001](0001-hook-as-the-state-source.md)）。

実測（Claude Code 2.1.220 · バイナリの strings と実データ）:

- タイトルは transcript の JSONL に独立した行として追記される。
  `{"type":"ai-title","aiTitle":"…","sessionId":"…"}` と
  `{"type":"custom-title","customTitle":"…","sessionId":"…"}` の 2 種。
- 本体の解決順は `agentName || customTitle || aiTitle || summary || firstPrompt`。
- 1 回きりではなく**メタ情報ブロックとして繰り返し追記される**。2.4MB の
  transcript で 53 件、追記間隔は 30〜91KB、最後の 1 件は EOF から 8KB。
- 逆方向の経路として、`SessionStart` / `UserPromptSubmit` hook の
  `hookSpecificOutput.sessionTitle` で**こちらから付ける**こともできる。

## 決定

`transcript::session_title` で **末尾 256KB を読み、`sessionId` が一致する
タイトル行を後ろから探す**。`custom-title` を `ai-title` より優先する
（種別で決める — 新しい自動生成が、ユーザの付けた名前を上書きしないように）。
得た文字列は `Session::title: Option<String>` としてストアに載せ、ホバーカードの
2 行目に出す。**無ければその行ごと出さない。**

読むのは `SessionStart` / `UserPromptSubmit` / `Stop` / `StopFailure` の 4 つだけ。
`reduce` は純関数のままにし（[0001](0001-hook-as-the-state-source.md)）、pid と
同じく I/O 層の `ccsessions hook` が保存直前に押す。

## 理由

- **未文書の内部フォーマットに依存する。** 公式の hook スキーマには無い。それでも
  採ったのは、(1) 代替経路が無い、(2) 壊れ方が「タイトル行が見つからない ＝ 出さない」
  に限られ、状態表示には一切影響しないため。`detect_errors` と同じ
  「補助手段」の扱いで、失敗は静かに無表示へ倒す。
- **`sessionTitle` を hook から自分で付ける案は採らない。** ccsessions が名前を
  決めてしまうと、Claude Code の `/resume` 一覧の表示まで書き換わる。状態を
  **観測する**だけの道具が、ユーザの持ち物に名前を書き込むのは越権。
- **タイトルが取れなかったときに既存の値を消さない。** タイトル行が tail の窓に
  入らない回はありうる。一度出た名前が回ごとに消えたり出たりする方が、少し古い
  名前が残るより悪い。
- **セッション id の先頭数文字を出す案では足りない。** 区別はつくが、どちらが
  どの作業かは分からない。区別だけでなく識別ができるのがタイトルの価値。
- **tail を 64KB（エラー判定と同じ）にしない。** 実測の追記間隔の最大が 91KB で、
  64KB では跨げない。256KB は実測の 2 倍以上の余裕を見た値。届かなければ
  出ないだけなので、安全側は「広め」。
- **全 hook イベントで読まない。** `SubagentStart` などは 1 ターンに何度も飛ぶ。
  タイトルはターンをまたいで滅多に変わらないので、ターンの区切りだけで足りる。

## 影響

- `Session` に `title` が増えた。`#[serde(default, skip_serializing_if)]` なので、
  旧バージョンが書いたファイルも読め、無いときはキーごと出ない（[0002](0002-one-file-per-session.md)
  の「混在しても壊れない」を保つ）。
- **カードの幅はタイトルで決まりうる。** 自動生成の日本語 1 文は長さを制御できない
  ので、`theme::CARD_SUBTITLE_MAX_W`（240pt）で実測幅を見て省略する。ここを外すと
  カードが画面幅を超えて伸びる。
- 番人は `transcript.rs` の `a_user_set_title_outranks_a_newer_generated_one` と
  `a_title_belonging_to_another_session_is_ignored`（resume/fork で他セッションの
  行が混ざる形）。Claude Code 側が形式を変えたらこれらは通ったまま実データで
  取れなくなるので、**タイトルが出なくなったらまず実物の transcript を grep する**。
- 生き物の名札（`short_name`）は変えていない。帯の中に文字を増やす余地が無く、
  [0010](0010-narrow-band-folds-inward.md) の折りたたみ方針とも合わないため。
