# 崩してはいけない不変条件

いずれも**実際に踏んだ問題**の再発防止で、変更すると**静かに壊れる**（テストは通り、
その場では正しく見え、あとで別の場所が壊れる）。コードを変えるより先にここを読む。

「なぜ他の案を採らなかったのか」は [`adr/`](adr/README.md)。実装の細部は各ファイルの
doc comment にある。

## hook

### `ccsessions hook` は何があっても exit 0・stdout に何も書かない

失敗は stderr 1 行に留める。行儀の問題ではなく**安全装置**（Claude Code 2.1.220 の
実装で確認済み）。

- **exit 2 は絶対に返さない** — `PreToolUse` ではツールをブロックし、
  `PermissionRequest` では**権限を拒否**し、`UserPromptSubmit` では**ユーザの
  プロンプトを消去**し、`PostToolBatch` では**エージェントのループを停止する**。
  購読イベントを増やすたびに事故の被害面が広がるので、この不変条件は
  イベントを足すときにこそ効く。
  （Rust の panic は exit 101 なのでブロックにはならない＝安全側。`exit 1` も非ブロック）
- **stdout に書かない** — `UserPromptSubmit` と `SessionStart` は exit 0 の stdout が
  **そのまま Claude に渡る**。書けばユーザのプロンプトにゴミが混入する。
- **`--record` は本番の settings.json に残さない** — payload にはユーザが打った生の
  プロンプトが入る。`ccsessions doctor` が付けっぱなしを検出して警告する。

番人は `ccsessions/tests/cli.rs::hook_always_exits_zero_and_writes_nothing_to_stdout`
（空 stdin・壊れた JSON・値の無いフラグ・書けないパス・巨大 payload を通す）。

### Claude Code の設定ファイルは他ツールの hook が同居する本番ファイルで、1 つとは限らない

- **ccsessions は settings.json を一切書かない。** hook を配線するのは Claude Code
  プラグイン（`plugins/ccsessions/`）で、`settings.json` に入るのは
  `enabledPlugins` の 1 行だけ。`hooks` セクションに触らないので、他ツールの hook を
  壊す経路そのものが無い（[ADR 0021](adr/0021-distribution.md)）。かつてあった
  `install-hooks` / `uninstall-hooks` は、書き込みコードごと消した。
  **書き込みを戻すなら [ADR 0007](adr/0007-settings-json-merge-only.md) を読み直すこと** —
  追記マージ・バックアップ・同意・冪等の 4 つが同時に要る。
- **読む側は全候補を走査する**（`known_settings_paths`）。`doctor` が 1 ファイルしか
  見ないと、hook を別のファイルに置いた人へ「NOT installed」と嘘をつく。
- **プラグイン経由の配線は `hooks` には現れない。** `MARKER` の走査では原理的に
  見つからないので、`doctor` は `enabledPlugins` も見る
  （`enabled_ccsessions_plugins`）。これを忘れると、正しく入れた人に
  「NOT installed」と言うことになる。
- **プラグインの `hooks.json` は `SIMPLE_EVENTS` / `hook_timeout_secs` と一致する。**
  同じ内容が Rust の定数と JSON の 2 か所にあるので、真実は Rust 側と決めてある。
  番人は `settings_json.rs` の
  `the_plugin_subscribes_to_exactly_the_declared_events` /
  `the_plugin_declares_the_timeout_the_contract_requires` /
  `the_plugin_runs_its_bundled_wrapper_rather_than_a_baked_absolute_path`。
- **購読するのは `settings_json.rs` の `SIMPLE_EVENTS`（10 種）だけで、すべて matcher 無し**。
  matcher で絞ると**そのイベントの他の payload が一切届かない**（かつて `PreToolUse` を
  `matcher: "Task"` で仕込んでいたために「判断待ちを解除する」経路が本番で死んでいた）。
  高頻度イベント（`PostToolUse` / `PreToolUse`）は購読しない。判断待ちからの復帰は
  **モデル往復ごとに 1 回**の `PostToolBatch` で取る。
- **`PreToolUse` は購読しない**。サブエージェント追跡は `SubagentStart`（実際に起動した）に
  一本化してあり、`PreToolUse(Agent|Task)`（ツールを呼んだ）と併存させると agents を
  二重に push する。旧バージョンで入ったエントリは reducer が無視するので無害だが無駄なので、
  `ccsessions doctor` が「stale hook」として報告する。
- **新規エントリには必ず `timeout` を入れる**。明示しないと既定 600 秒が効き、hook が万一
  ブロックするとユーザのターンが最大 10 分止まる。値は**そのイベントの既定を上回らない**
  （`SessionEnd` の既定は 1.5 秒なので 1 秒）。
  **既存エントリのうち `command` に `ccsessions hook` を含むものには後追いで足す** —
  「既存エントリを変更しない」の目的は他ツールの hook を壊さないことであり、自分の
  エントリまで例外にすると既に導入済みのユーザに timeout が永久に届かない。
- **テストは本物の `~/.claude/settings.json` に絶対に触らない**。tempdir に**埋め込み
  フィクスチャ**を書き、そのパスを位置引数で明示的に渡す（本物をコピーして使うと、
  ccsessions 導入済みの環境で冪等性テストが壊れる。実際に踏んだ）。走査を伴うコマンドの
  テストは `HOME` とカレントディレクトリごと tempdir に閉じる。

### `Stop` は「メインスレッドのターンが終わった」であって「サブエージェントも終わった」ではない

lead が喋り終えても、バックグラウンドのサブエージェントや teammate は走り続ける。
`Stop` で `agents` を消して `Done` にしていたため、**teammate が黙々と働いている間ずっと
「完了」**になり、そのあと無操作タイマー（`Notification{idle_prompt}`）で「判断待ち」に
なっていた。teammate が再開されるたび `SubagentStart` が飛んで正しい表示に戻るので、
「たまにおかしい」形で出る（実測は [`adr/0024`](adr/0024-stop-is-not-the-end-of-subagents.md)）。

- **`Stop` では `background_tasks` と突き合わせて `agents` を作り直す**（消さない）。
  `background_tasks[].id` は `SubagentStart.agent_id` と**同じ値**なので、残骸の除去と
  取りこぼしの回収が同時にできる。数えるのは `type` が `subagent` / `teammate` のものだけ
  （`shell` を数えると、バックグラウンドの `sleep` 1 つでターンが完了しなくなる）。
- **フィールドごと無い（`None`）と空（`Some([])`）を区別する。** 前者は教えてくれない
  古い Claude Code なので、判断材料が無いまま待たず従来どおり完了に倒す。
- **`StopFailure` では `agents` を消さない。** この payload に `background_tasks` は無く、
  生死が分からないものを消すと生きている teammate を見失う。
- **`Notification{idle_prompt}` は `agents` が居るあいだ無視する。** この通知は
  「メインスレッドが手空き」のタイマーで、Claude Code はこれを出すときにバックグラウンドの
  仕事を見ていない。ユーザの操作が本当に要る `permission_prompt` / `agent_needs_input` は
  対象外（判断待ちにする）。
- **`main_stopped` を落とさない。** 最後の `SubagentStop` の戻り先が `Working` か `Done` かは
  これでしか決まらない。無いと、lead がとっくに終わったセッションが「作業中」に戻る。

payload の形は**収録した実物**で固定してある（`a_recorded_stop_payload_is_understood_end_to_end`）。
想像で書いた payload だと、今回のような「実際には別の形だった」を検出できない。

## ストア

### 書き手は `store::lock_exclusive()` を `load` から `save`/`remove` まで保持する

Claude Code は**マッチした hook をすべて並列プロセスで実行する**ので、排他しないと
read-modify-write が後勝ちで消える（8 並列の `SubagentStart` で agents が 2 件しか
残らなかった）。`write_atomic` が保証するのは「途中状態を読み手に晒さない」ことだけで、
read-modify-write 全体ではない。

ロックは `load`/`save` に内蔵していない — 内蔵しても reduce を挟む区間は守れないため、
**書き手側の規律**にしてある。現在の書き手は `hook.rs` と `set_cmd.rs`、および
ファイルを消す `store::sweep`（daemon から呼ばれる。こちらは関数内で取る）。
**読み手（daemon の poller）はロックを取らない** — rename の atomicity で足りる。

番人は `ccsessions/tests/cli.rs::parallel_hook_processes_do_not_lose_agents`
（実プロセスを 8 個同時に起動する）。設計の理由は `ccsessions-core/src/lock.rs` の doc。

### セッションが死んだかは pid で決める。TTL は保険であって主判定ではない

`SessionEnd` は「普通に終わった」ときしか飛ばず、**強制終了・端末を閉じる・親ツールに
よる kill では飛ばない**。それだけに頼っていたため、死んだセッションが `session_ttl`
（8 時間）居座り、`max_sessions` の枠を食って生きているセッションを押し出していた
（同じ作業ディレクトリのゾンビが 9 匹並び、枠 12 のうち 9 を占有した）。

`ccsessions hook` が毎回**セッションの持ち主の pid（＝ Claude Code 本体）**を記録し、
`Session::dead_reason` が `process::is_alive` で生存を見る。

- **持ち主は「直接の親」ではない。祖先をたどってシェルを読み飛ばした先**にいる。
  直接の親を持ち主にしていた頃、プラグインが配る `ccsessions-hook.sh` を経由すると
  **全セッションが表示されなくなった**（`v0.1.0` の実害）。ラッパーは「何があっても
  exit 0」を守るため `exec` を使わず、`ccsessions hook` から見た親は**ラッパーの
  `sh`** になる。それは hook が終わった瞬間に死ぬので、書いた直後に死んだ判定で
  一掃されていた（`reaped session ccsessions — pid 15801 が居ない`／持ち主の
  claude は 15780 だった）。hook もストアも daemon も正常なのに何も出ない、という
  壊れ方をする。番人は
  `cli.rs::a_session_recorded_through_the_plugin_wrapper_is_still_live_afterwards`
  （本物のラッパーを本物の `sh` で起動する）。
  **`make demo` はこの経路を通らない**（`main.rs` がメモリ上のダミーを描く）ので、
  表示まわりを demo だけで確かめても検出できない。
- **「存在するか」だけでは足りない。ゾンビ（終了したが親が `wait` していない残骸）を
  明示的に弾く**。`kill(pid, 0)` はゾンビにも成功を返すので、回収しない親の下では pid
  判定が丸ごと無効化される（ラッパーツールが配下の `claude` を `wait` しないことがあり、
  判断待ちのセッションが TTL の 8 時間居座った）。ゾンビは
  `libc::proc_pidinfo(PROC_PIDT_SHORTBSDINFO)` が `ESRCH` で落ちることで分かる
  （`kill` と違い libproc はゾンビを「タスクが無い」として扱う）。番人は
  `process.rs::a_child_nobody_waited_for_is_not_alive`。
- **判定は必ず安全側（生きている）へ倒す**。`pid` が `None` なら TTL だけで判断する。
  「死」と言い切れるのは `kill` が `ESRCH` を返したときか、ゾンビだと確認できたときだけ。
  libproc が `EPERM` 等で答えられなければ生きている側に倒す。**pid の再利用も
  「生きている」側に転ぶ**（表示が少し長引くだけ）。
- **`reduce` に pid を渡さない**。pid は stdin の JSON ではなく実行環境から取る事実なので、
  reducer を純関数のまま保ち、保存直前に `hook.rs` が押す。
- 消したものは `ccsessionsd` が必ず 1 行ログに残す（`reaped session ... — pid 123 が居ない`）。
  生きているセッションが消える事故が起きたら、まずこの行を見る。
- 掃除（ファイル削除）は常駐している `ccsessionsd` の仕事（起動時 + 60 秒ごと）。
  以前は `store::sweep` がどこからも呼ばれておらず、ファイルが無限に溜まっていた。

### `ignore` は表示のフィルタ。`take(max)` の**前**で外し、`sweep` には効かせない

`config.toml` の `ignore` に当たったセッションを `store::list_live` が一覧から外す
（[ADR 0026](adr/0026-ignore-is-a-display-filter.md)）。守る点は 2 つ。

- **絞り込みの順序は `is_live` → `ignore` → `take(max)`。** ignore を `take(max)` の
  後ろに置くと、隠したはずのセッションが `max_sessions` の枠を食って生きている
  セッションを押し出す — 1 つ上の「死んだセッションが枠を食う」不具合と同じ形が、
  別の入口から復活する。番人は
  `store.rs::ignored_sessions_do_not_consume_the_max_slots`。
- **`sweep` は `ignore` を見ない。** 見せないことと死んだことは別で、消してしまうと
  `--all` で戻せず、次の hook で作り直されるだけの空振りになる。番人は
  `store.rs::sweep_does_not_look_at_the_ignore_list`。
- 外した件数は `LiveSessions.ignored` で返す。**引き算では求められない** —
  枠の前で外すので、1 件隠せば別のセッションが 1 件繰り上がる。`ccsessions list` の
  「N 件を非表示」も `doctor` の `stale` 計数もこの数を直接使う。

### `Session::set_state` は状態が変わったときだけ `since` を更新する

再通知で経過時間の起点が戻らないため。あわせて **`Error` 以外へ遷移したら `error_kind`
を落とす** — 「`error_kind` は `Error` のときだけ意味を持つ」を呼び出し側の規律に任せると、
一度 rate limit で落ちたセッションが次のターンで成功しても "rate limit" が残る。
`Error` にするときは種別も一緒に渡す `set_error` を使う。

## レイアウト（bar はメニューバーの中に収まらなければならない）

窓は**自分の矩形ぶんだけ下のクリックを奪う**ので、帯がメニューバーからはみ出すと
その面積ぶんメニューバーの通常操作が壊れる。

**メニューバー高は機種依存**: ノッチ機 **33pt** / 非ノッチ画面（外部モニタ・Air・旧機種）
**24pt**。`NSScreen::mainScreen` はフォーカス追従なので、外部モニタにフォーカスを移すだけで
後者に切り替わる（「他人の Mac の問題」ではない）。

- `layout::bar_fit` が差を 3 段階で吸収する（33pt はそのまま / 24pt はグリフを体に重ねる /
  それ以下は体も縮める）。bar では吹き出しと `z` を出さず**グリフ 1 個に集約**し、
  バッジも体の右下に重ねる（dock は元デザインどおり全部出す）。
- **24pt でもアニメは絞らない**。グリフを体に重ねた時点で余地 4pt が丸ごとアニメのものに
  なり、hop の 4pt がちょうど収まる。`anim_scale` の分母は `BAR_MAX_ANIM_AMP`(4) であって
  `BAR_HEADROOM`(12) ではない（12 で割ると 24pt で 1/3 になり、bob が 0.67pt ＝事実上の静止）。
- **顔の bar の体の高さは 22pt 以下**（`face::validate::MAX_BAR_BODY_H`）。
- **窓は帯のサイズぴったりに作る**。帯からはみ出すホバーカードはクリック透過の**別窓**に
  出す（ウィンドウが 2 枚ある理由）。

番人は `layout.rs::every_face_fits_inside_every_supported_menu_bar`（**全顔 × {24, 33}**）と
`the_glyph_box_stays_inside_the_window_even_mid_animation`。この 2 つは上限しか見ない
（絞りすぎても通る）ので、絞り方は
`the_animation_is_scaled_to_exactly_the_room_above_the_body` が押さえる。

### 「使える幅」が変わりうる経路は、必ず `refit` を通らなければならない

`Packing::avail_w`（＝縮小率を解く材料）を計算するのは `main::current_packing` だけで、
それを呼ぶのは `refit()` だけ、`refit()` を呼ぶのは `rebuild_and_reposition()`
（設定・画面・顔の 3 つの入口）と `on_sessions()` だけ。**`avail_w` の入力が変わるのに
`refit` を通らない経路を作ってはいけない。**

通らないと**テストは通ったまま静かに壊れる**: 群れはちゃんと出るが縮小率が古いままなので、
`reposition` だけが新しい幅で置き直す。結果、**縮めれば右に収まるのに縮まず、ノッチ左へ
逃げる** — アプリメニューと重なりうる方＝ [0012](adr/0012-notch-avoidance.md) と
[0013](adr/0013-uniform-squeeze.md) が避けたかった側に落ちる。

実際に踏んだ: bar の `avail_w` は当初 画面ジオメトリにしか依存せず、それが変わる経路は
必ず `ScreenChanged → rebuild_and_reposition → refit` を通っていた。メニューエクストラの
実行時計測を足したことで `avail_w` が**エクストラにも依存する**ようになったが、
**エクストラの増減は `ScreenChanged` を起こさない**。そのため 10 秒ポーリングの
`on_sessions` が `reposition` だけを呼び、縮小率が取り残された。

- `on_sessions` は `refit()` を呼んでから `Packing` の変化を見る。**変化したときだけ**
  組み直す（毎回やるとレイヤが作り直されてアニメ位相が揃う → 描画の不変条件に反する）。
  守っているのは `Flock::needs_reconfigure` の `Packing` 比較。
- **番人はユニットテストではない。** ここは AppKit のイベント経路なので、確かめるには
  実機で level 25 の窓を帯の中に出して（＝偽のメニューエクストラ）、
  `/tmp/ccsessionsd-dev.log` の `avail_w` と `squeeze scale` と `window` の x が
  揃って動くことを見る。**`avail_w` だけ動いて `squeeze scale` が 1.000 のままなら
  この不変条件が破れている。**

### コンパクト表示の縮小は「一様」でなければならない

`layout::Squeeze` の倍率を、体・間隔・余白・付属パーツ・アニメの振れ幅すべてに掛ける。
一様でないと 2 通りに静かに壊れる。

1. `layout::squeeze` は**幅が倍率に比例する**前提で「収まる倍率 = `avail_w / natural_w`」を
   解いている。余白だけ据え置く／条件で足し引きすると、縮めたのにはみ出す。
2. 縦の不変条件（メニューバーに収まる・グリフがアニメの頂点で切れない）は不等式の両辺が
   同じだけ縮むから保たれる。体だけ縮めると余白を突き抜ける。

`creature.rs` が theme／`FaceSpec` から読む pt 値には**もれなく `sq.scale` を掛ける**
（`FaceSpec::eye` は体が等倍である前提の pt を返すので、掛け忘れると縮んだ体から目が
はみ出す）。唯一の例外は dock のラベル 2 段とバッジで、これは省くので高さ・幅から落ちる。
番人は `a_compact_flock_still_fits_inside_every_supported_menu_bar` と
`squeezing_scales_the_whole_layout_uniformly`。

## 描画（ccsessionsd）

- **アニメは CoreAnimation に自走させる**。毎フレーム描画するタイマを足さない
  （アイドル時 CPU 0.0–0.1% が要件）。レイヤは作り直さず `apply` で差し替える
  （作り直すとアニメ位相がリセットされ、群れが不自然に同期する）。
- **`Flock::reconfigure` は顔・配置・`BarFit`・倍率が実際に変わったときだけ呼ぶ**
  （`needs_reconfigure` で判定）。毎ポーリングで呼ぶと上と同じ理由で群れが同期する。
- **`tao::Window::set_visible` を使わない**。中身が `makeKeyAndOrderFront` で、表示の
  たびにフォーカスを奪う。`window::set_visible`（`orderFront:`/`orderOut:`）を使い、窓は
  `with_visible(false)` で作る。あわせて `set_activate_ignoring_other_apps(false)` を
  `run()` より前に呼ぶ（起動時のフォーカス奪取防止）。
- **表示順は `updated` 降順ではなく名前 → id の安定順**（`main.rs::sort_for_display`）。
  recency 順で描くと生き物が飛び回る。`store::list_live` の recency 順は「どのセッションを
  選ぶか」専用。
- **依存バージョンは実際に FFI が通ることを確かめた組み合わせでピン**
  （tao 0.35.3 / objc2 0.6 / objc2-* 0.3）。objc2 系は minor 更新でメソッド名・型が変わる。

## 設定

**入口を増やさない。GUI は `ccsessions ui` だけ**（メニューバーの status item は廃止した）。
入口が 2 つあると設定を 1 つ足すたびに「core・CLI・メニュー・Web」の 4 か所を直すことになり、
しかもメニューは開かないと見えず画面収録権限も無いので確かめられない。

- **キー・型・選択肢・検証は `config::fields()` / `config::set_field()` の 1 か所**。
  CLI (`ccsessions config set`) も `/api/config` もここを通るので、設定を足すときに触るのは
  `Config`・`RawConfig`・`render_toml`・`fields()` だけ。番人は
  `every_key_written_to_the_toml_is_in_the_schema`（`render_toml` が書く全キーがスキーマに
  載っている＝画面に出る）と `the_current_value_of_every_field_round_trips`。
- **画面は設定のキーを 1 つも知らない**。`/api/config` が返すスキーマを列挙して描く。
- **daemon は設定を読むだけ**。UI が `config.toml` を書き、poller が mtime 差分で
  `ConfigChanged` を流す。daemon が書くのは dock のドラッグ位置だけで、これは
  「離した瞬間の 1 回」に限る（`DockDragEnded`）。
- **`/api/config` は 1 項目ずつ書く**（`{key, value}`）。画面が持っているスナップショットを
  丸ごと書き戻すと、その間に daemon が書いた dock の位置が黙って巻き戻る。
- **`ccsessions ui` のテストは本物の `~/.config/ccsessions` に触らない**。設定も顔も tempdir に
  閉じ、`Paths`（`--config` / `--faces-dir`）で渡す。

## 顔（生き物のデザイン）

- **`faces/*.toml` が唯一の定義。`theme.rs` に顔ごとの分岐を戻さない**。形・目・線画・寸法は
  `FaceSpec` が持ち、`theme.rs` には**全部の顔に共通のもの**だけを置く。色・アニメ・グリフは
  顔ごとに変えられない（状態の読み取りやすさを守るため）。既存の顔の見た目は
  `face/golden.rs` が数値で固定しているので、解決ロジックを触ったら必ず走らせる。
- **ビルダーは「顔の形式」を増やさない**。出すのは `faces/*.toml` で、しかも
  **TOML テキストを唯一の中間形にする**（`Draft` → テキスト → `parse::parse` → `FaceSpec`）。
  `FaceSpec` を直接組んで「ついでに TOML も書き出す」形にすると、座標の丸めや書き出し漏れで
  **画面で見た顔と保存される顔が別物になる**（保存するまで誰も気づかない）。同じ理由で
  プレビューは JS で描き直さず `face::svg` の出力をそのまま貼る。番人は
  `builder::tests::the_generated_toml_parses_back_into_the_same_face`。
- 「どの組み合わせでも検証を通る」は 3 つの仕掛けで支えている: パネル線の幅を輪郭の
  断面（`shape::Profile`）に比例させる / 目は**検証器そのものを使って**収まるまで縮める /
  bar のパネル線を dock の部分集合に縛る。総当たりのテストがこれを守るので、
  パーツを足したら必ず走らせる。**バリエーション数のテストは下限しか見ない** —
  個数の等値で固定すると行を足すたびに落ち、「1 行足すだけ」という設計を罰することになる。
- **左右 1 対のパーツは、右を計算して鏡像を取る**（`line_details`）。別々に計算すると丸めで
  1 単位ずれ、生き物が傾いて見える。`off`（端への寄せ）も幅と同じく**半幅に比例させる**
  （比例させないと細い顔で耳が輪郭からはみ出す）。対のパーツだけ `dx` が「横位置」ではなく
  「左右の開き」なのも同じ理由。**その顔専用のパーツは表に置かない**（他の輪郭に載せると
  比率が合わず浮く）。番人は `builder::tests::the_builder_can_express_the_built_in_faces`。
