# 0020 · 設定の入口は Web UI ひとつだけ

採用 · 2026-07-27

## 文脈

かつては**設定がメニューバーの status item、顔作りが Web UI** に分かれていた。
分けている限り、設定を 1 つ足すたびに AppKit のメニュー（tag による分岐・チェックの
貼り直し・顔ごとのプレビュー画像）と Web の両方を直すことになる。しかもメニューは
開かないと見えず、画面収録権限が無いので目視で確かめられない。

実際に `show_glyphs` が「設定にも CLI にもメニューにも README にもあるのに、描画側が
一度も読んでいない」状態で放置されていた。設定が嘘をついていた。

## 決定

**GUI は `ccsessions ui`（`make config`）だけ。** daemon から status item とメニューを
削除した。

- **キー・型・選択肢・検証は `config::fields()` / `config::set_field()` の 1 か所。**
  CLI (`ccsessions config set`) も `/api/config` もここを通るので、設定を足すときに
  触るのは `Config`・`RawConfig`・`render_toml`・`fields()` だけ。
- **画面は設定のキーを 1 つも知らない。** `/api/config` が返すスキーマを列挙して描く。
- **daemon は設定を読むだけ。** UI が `config.toml` を書き、poller が mtime の差分で
  変更を拾う。daemon が書くのは dock のドラッグ位置だけで、これは「離した瞬間の
  1 回」に限る。
- **`/api/config` は 1 項目ずつ書く。** 画面が持っているスナップショットを丸ごと
  書き戻すと、その間に daemon が書いた dock の位置が黙って巻き戻る。

## 理由

入口が 2 つあると、片方が必ず腐る。UI と daemon の間に新しい通信路も要らない
（設定ファイルの mtime ポーリングが既にある）。

## 影響

- 番人は `every_key_written_to_the_toml_is_in_the_schema`（`render_toml` が書く全キーが
  スキーマに載っている＝画面に出る）と `the_current_value_of_every_field_round_trips`。
- Web UI は依存を足さずに HTTP を手で書いている。相手は自分のブラウザ 1 つなので、
  必要なのは「リクエスト行 + ヘッダ + Content-Length のボディ」だけ。
- **ローカルだから安全、で済ませない。** ユーザがこのツールを開いたまま別のサイトを
  見ることは普通にあり、そのページの JS は書き込み API を叩ける。127.0.0.1 への
  bind に加え、Host / Origin / Content-Type の 3 枚で止める（`ui_cmd::guard`）。
