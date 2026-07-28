//! `ccsessions ui` — 設定とキャラクタービルダーの Web UI をローカルに立てる。
//!
//! # なぜ入口が 1 つなのか
//! 以前は**設定がメニューバーの status item、顔作りがこの Web UI**に分かれていた。
//! 分けている限り、設定を 1 つ足すたびに AppKit のメニュー（tag による分岐・
//! チェックの貼り直し）と Web の両方を直すことになり、しかもメニュー側は
//! 見た目を確かめる手段が「実際に開く」しかない（画面収録権限が無ければなおさら）。
//! 顔を選ぶ体験も、プレビュー付きのメニュー項目という窮屈な器に押し込んでいた。
//!
//! そこで**設定の入口をここへ一本化**し、daemon からは status item を消した。
//! daemon は `config.toml` の mtime を見ているので、ここで保存すれば数百 ms で
//! 反映される — つまり UI と daemon の間に新しい通信路は要らない。
//!
//! # なぜサーバなのか（JS で描き直さないのか）
//! このリポジトリの資産のひとつは「**SVG が CALayer の忠実なプレビュー**である」
//! こと — 輪郭も目もパネル線も、daemon が描くのと同じデータ・同じ解決関数から
//! 出ている（`face/svg.rs` の doc）。ブラウザ側で顔を描き直すと、その瞬間に
//! 3 つ目の実装が生まれて、いずれ静かにずれる。
//!
//! そこで**描画も検証も TOML の生成も全部 Rust 側**に置き、ブラウザは
//! 「選んで、返ってきた SVG を貼る」だけにした。検証結果がそのまま
//! 画面に出るので、「目が顔からはみ出しています」を作りながら知れる。
//!
//! # なぜ HTTP を手で書くのか
//! 依存を足さないため（CLI が clap を使わずに引数を手で読んでいるのと同じ判断）。
//! 相手は**自分のブラウザ 1 つ**なので、必要なのは HTTP/1.1 の
//! 「リクエスト行 + ヘッダ + Content-Length のボディ」だけで足りる。
//! keep-alive もチャンクも要らないので、1 リクエスト 1 接続で閉じる。
//!
//! # 安全側の作り
//! `/api/save` と `/api/config` は**ファイルを書く**ので、「ローカルだから安全」で
//! 済ませない。ユーザがこのツールを開いたまま別のサイトを見ることは普通にあり、
//! そのページの JS は `http://127.0.0.1:8787/api/save` を叩ける（応答は読めなくても、
//! 書き込みはもう済んでいる）。守りは `guard` にまとめてある:
//!
//! - **127.0.0.1 にしか bind しない**（LAN からは触れない）
//! - **Host / Origin / Content-Type の 3 枚**でクロスオリジンの書き込みを止める
//!   （それぞれ何を塞ぐかは `guard` の doc）
//! - 書き込み先は「顔ディレクトリ / `<id>.toml`」と `config.toml` だけ。`id` は
//!   `is_saveable_id`（`[a-z0-9-]+`）を通すのでパス区切りを含められない
//! - 中身も自由には書かせない。顔は `compose` が組み立てて**検証を通した** TOML
//!   だけ、設定は `config::set_field` が**キーごとに検証した**値だけで、外から来た
//!   文字列がそのままファイルになる経路は無い
//! - ボディは 256KiB で打ち切る

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};

use ccsessions_core::config::{self, FieldKind};
use ccsessions_core::face::builder::{self, CharacterConfig};
use ccsessions_core::face::{svg, EyeColor, Registry, Size};
use ccsessions_core::session::SessionState;
use serde_json::{json, Value};

/// サーバが触るファイル。テストが本物の `~/.config/ccsessions` に触らずに済むよう、
/// **パスは引数で持ち回る**（`store` / `config` の `*_in` 系と同じ流儀）。
struct Paths {
    faces_dir: PathBuf,
    config_path: PathBuf,
}

/// 既定のポート。塞がっていたら順に上げる。
const DEFAULT_PORT: u16 = 8787;
/// ポートを探す回数。
const PORT_TRIES: u16 = 20;
/// リクエストボディの上限。
const MAX_BODY: usize = 256 * 1024;

// ---------------------------------------------------------------------------
// 静的アセット
// ---------------------------------------------------------------------------

const INDEX_HTML: &str = include_str!("ui/index.html");
const APP_JS: &str = include_str!("ui/app.js");
const STYLE_CSS: &str = include_str!("ui/style.css");

/// タブのアイコン。生き物 1 匹（作業中の色の丸と目）を SVG で直に書く。
const FAVICON: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 32 32">
<rect width="32" height="32" rx="7" fill="#0b0912"/>
<rect x="6" y="5" width="20" height="22" rx="10" fill="#242b45" stroke="#00e1e2" stroke-width="2"/>
<rect x="11" y="14" width="4" height="5" rx="2" fill="#eef2ff"/>
<rect x="17" y="14" width="4" height="5" rx="2" fill="#eef2ff"/>
</svg>"##;

// ---------------------------------------------------------------------------
// エントリポイント
// ---------------------------------------------------------------------------

pub fn run(args: &[String]) -> i32 {
    let mut port = DEFAULT_PORT;
    let mut open_browser = true;
    let mut faces_dir: Option<PathBuf> = None;
    let mut config_path: Option<PathBuf> = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--port" => {
                let Some(v) = args.get(i + 1).and_then(|v| v.parse::<u16>().ok()) else {
                    eprintln!("ccsessions: ui: --port には 1..65535 の数を渡してください");
                    return 1;
                };
                port = v;
                i += 2;
            }
            "--faces-dir" => {
                let Some(v) = args.get(i + 1) else {
                    eprintln!("ccsessions: ui: --faces-dir に値がありません");
                    return 1;
                };
                faces_dir = Some(PathBuf::from(v));
                i += 2;
            }
            "--config" => {
                let Some(v) = args.get(i + 1) else {
                    eprintln!("ccsessions: ui: --config に値がありません");
                    return 1;
                };
                config_path = Some(PathBuf::from(v));
                i += 2;
            }
            "--no-open" => {
                open_browser = false;
                i += 1;
            }
            other => {
                eprintln!("ccsessions: ui: 未知の引数 {other:?}");
                eprintln!(
                    "usage: ccsessions ui [--port <n>] [--faces-dir <path>] [--config <path>] [--no-open]"
                );
                return 1;
            }
        }
    }

    let paths = Paths {
        faces_dir: faces_dir.unwrap_or_else(ccsessions_core::faces_dir),
        config_path: config_path.unwrap_or_else(ccsessions_core::config_path),
    };
    let Some((listener, bound)) = bind(port) else {
        eprintln!(
            "ccsessions: ui: ポート {port}〜{} がすべて塞がっています。--port で別の番号を指定してください",
            port.saturating_add(PORT_TRIES - 1)
        );
        return 1;
    };

    let url = format!("http://127.0.0.1:{bound}/");
    println!("ccsessions 設定 / キャラクタービルダー: {url}");
    println!("設定: {}", paths.config_path.display());
    println!("顔の保存先: {}", paths.faces_dir.display());
    println!("止めるには Ctrl-C。");
    if open_browser {
        // 開けなくても致命的ではない（URL は上に出してある）。
        let _ = std::process::Command::new("open").arg(&url).status();
    }

    for stream in listener.incoming() {
        match stream {
            Ok(s) => {
                if let Err(e) = handle(s, &paths, bound) {
                    // 接続 1 本の失敗でサーバを落とさない（ブラウザは平気で
                    // 接続を切る）。
                    eprintln!("ccsessions: ui: {e}");
                }
            }
            Err(e) => eprintln!("ccsessions: ui: accept: {e}"),
        }
    }
    0
}

fn bind(from: u16) -> Option<(TcpListener, u16)> {
    for p in from..from.saturating_add(PORT_TRIES) {
        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), p);
        if let Ok(l) = TcpListener::bind(addr) {
            return Some((l, p));
        }
    }
    None
}

// ---------------------------------------------------------------------------
// HTTP
// ---------------------------------------------------------------------------

struct Request {
    method: String,
    path: String,
    query: String,
    host: String,
    origin: String,
    content_type: String,
    body: String,
}

fn handle(stream: TcpStream, paths: &Paths, port: u16) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let Some(req) = read_request(&mut reader)? else {
        return Ok(());
    };
    let mut out = stream;
    let (status, ctype, body) = route(&req, paths, port);
    respond(&mut out, status, ctype, &body)
}

// ---------------------------------------------------------------------------
// 門番
// ---------------------------------------------------------------------------

/// リクエストを通してよいか。駄目なら返す応答。
///
/// # 何から守るのか
/// `/api/save` は**ファイルを書く**。127.0.0.1 に bind してあるだけでは、
/// 「ユーザがたまたま開いた別のページ」からの書き込みは止まらない
/// — 攻撃ページの JS が `fetch('http://127.0.0.1:8787/api/save', …)` を投げると、
/// **ブラウザは Host をこちらの名前で送る**からだ。応答は読めなくても、
/// 書き込みはもう済んでいる（CSRF）。
///
/// そこで 3 枚重ねる。**どれか 1 枚ではなく 3 枚とも要る**:
///
/// 1. **Host** … DNS リバインディング避け。攻撃者のドメインを 127.0.0.1 へ
///    向けられた場合、Host は攻撃者のドメインになる
/// 2. **Origin** … 付いていたら自分のオリジンでなければ拒否。ブラウザは
///    POST に必ず Origin を付けるので、クロスオリジンの書き込みはここで止まる。
///    curl 等は Origin を送らないが、CSRF は「被害者のブラウザを使う」攻撃なので
///    それは脅威モデルの外（送らない相手は素通しでよい）
/// 3. **Content-Type** … 書き込みは `application/json` 限定。これが本命の一枚で、
///    `text/plain` などの「単純リクエスト」ではプリフライトが飛ばないため
///    Origin だけでは古い経路を塞ぎきれない。JSON を要求すると
///    ブラウザは必ずプリフライトを送り、こちらは CORS 許可を一切返さないので
///    本番のリクエストが発射されない。HTML フォームも 3 種類の
///    単純タイプしか送れないので同時に塞がる
fn guard(req: &Request, port: u16) -> Option<(u16, &'static str, String)> {
    let deny = |status: u16, msg: &str| Some((status, JSON, json!({ "error": msg }).to_string()));

    let host = req.host.split(':').next().unwrap_or("");
    if !matches!(host, "127.0.0.1" | "localhost" | "[::1]" | "::1") {
        return deny(403, "localhost 以外の Host からは使えません");
    }

    if !req.origin.is_empty() && !is_own_origin(&req.origin, port) {
        return deny(
            403,
            "他のページからは使えません（このツールは自分のブラウザ専用です）",
        );
    }

    // 書き込みを伴うメソッドだけ。GET は本文を持たず、返す中身も
    // クロスオリジンからは読めない（CORS 許可を返していない）。
    if req.method != "GET" {
        let base = req
            .content_type
            .split(';')
            .next()
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();
        if base != "application/json" {
            return deny(415, "Content-Type: application/json で送ってください");
        }
    }

    None
}

/// このサーバ自身のオリジンか。ユーザは 127.0.0.1 でも localhost でも開ける。
fn is_own_origin(origin: &str, port: u16) -> bool {
    [
        format!("http://127.0.0.1:{port}"),
        format!("http://localhost:{port}"),
        format!("http://[::1]:{port}"),
    ]
    .iter()
    .any(|o| o == origin)
}

fn read_request(reader: &mut BufReader<TcpStream>) -> std::io::Result<Option<Request>> {
    let mut line = String::new();
    if reader.read_line(&mut line)? == 0 {
        return Ok(None);
    }
    let mut it = line.split_whitespace();
    let method = it.next().unwrap_or("").to_string();
    let target = it.next().unwrap_or("/").to_string();
    let (path, query) = match target.split_once('?') {
        Some((p, q)) => (p.to_string(), q.to_string()),
        None => (target, String::new()),
    };

    let mut len = 0usize;
    let mut host = String::new();
    let mut origin = String::new();
    let mut content_type = String::new();
    loop {
        let mut h = String::new();
        if reader.read_line(&mut h)? == 0 {
            break;
        }
        let h = h.trim_end();
        if h.is_empty() {
            break;
        }
        if let Some((k, v)) = h.split_once(':') {
            let v = v.trim();
            match k.to_ascii_lowercase().as_str() {
                "content-length" => len = v.parse().unwrap_or(0),
                "host" => host = v.to_string(),
                "origin" => origin = v.to_string(),
                "content-type" => content_type = v.to_string(),
                _ => {}
            }
        }
    }

    let mut body = Vec::new();
    if len > 0 {
        let capped = len.min(MAX_BODY);
        body.resize(capped, 0);
        reader.read_exact(&mut body)?;
    }

    Ok(Some(Request {
        method,
        path,
        query,
        host,
        origin,
        content_type,
        body: String::from_utf8_lossy(&body).into_owned(),
    }))
}

fn respond(out: &mut TcpStream, status: u16, ctype: &str, body: &str) -> std::io::Result<()> {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        415 => "Unsupported Media Type",
        500 => "Internal Server Error",
        _ => "OK",
    };
    write!(
        out,
        "HTTP/1.1 {status} {reason}\r\n\
         Content-Type: {ctype}\r\n\
         Content-Length: {}\r\n\
         Cache-Control: no-store\r\n\
         Connection: close\r\n\r\n",
        body.len()
    )?;
    out.write_all(body.as_bytes())?;
    out.flush()
}

const JSON: &str = "application/json; charset=utf-8";

// ---------------------------------------------------------------------------
// ルーティング
// ---------------------------------------------------------------------------

fn route(req: &Request, paths: &Paths, port: u16) -> (u16, &'static str, String) {
    if let Some(denied) = guard(req, port) {
        return denied;
    }
    let dir = paths.faces_dir.as_path();
    match (req.method.as_str(), req.path.as_str()) {
        ("GET", "/") => (200, "text/html; charset=utf-8", INDEX_HTML.to_string()),
        ("GET", "/app.js") => (200, "text/javascript; charset=utf-8", APP_JS.to_string()),
        ("GET", "/style.css") => (200, "text/css; charset=utf-8", STYLE_CSS.to_string()),
        // ブラウザが必ず取りに来る。返さないとコンソールが 404 で汚れて、
        // 本物のエラーが埋もれる。
        ("GET", "/favicon.ico") => (200, "image/svg+xml", FAVICON.to_string()),

        ("GET", "/api/config") => json_result(config_json(paths)),
        ("POST", "/api/config") => json_result(set_config(paths, &req.body)),

        ("GET", "/api/parts") => (200, JSON, parts_json().to_string()),
        ("GET", "/api/saved") => (200, JSON, saved_json(dir).to_string()),
        ("GET", "/api/load") => json_result(load_saved(dir, &req.query)),

        ("POST", "/api/preview") => json_result(preview(&req.body)),
        ("POST", "/api/random") => json_result(random(&req.body, &req.query)),
        ("POST", "/api/save") => json_result(save(dir, &req.body)),

        _ => (
            404,
            JSON,
            json!({"error": format!("{} {} は無い", req.method, req.path)}).to_string(),
        ),
    }
}

fn json_result(r: Result<Value, String>) -> (u16, &'static str, String) {
    match r {
        Ok(v) => (200, JSON, v.to_string()),
        Err(e) => (400, JSON, json!({ "error": e }).to_string()),
    }
}

// ---------------------------------------------------------------------------
// /api/config — 設定の読み書き
// ---------------------------------------------------------------------------

/// 設定画面が必要とするもの全部: **スキーマ（`config::fields`）に現在値を載せたもの**。
///
/// 画面はキーを 1 つも知らない。ここが返した項目を順に描くだけなので、設定を
/// 足すときに触るのは core のスキーマだけで済む（`config.rs` のスキーマ節）。
fn config_json(paths: &Paths) -> Result<Value, String> {
    let cfg = config::load(&paths.config_path)?;
    let faces = Registry::load_in(&paths.faces_dir);

    let fields: Vec<Value> = config::fields()
        .iter()
        .map(|f| {
            let mut v = json!({
                "key": f.key,
                "label": f.label,
                "help": f.help,
                "value": config::field_value(&cfg, f.key),
            });
            let o = v.as_object_mut().expect("json! で作った object");
            match f.kind {
                FieldKind::Choice(choices) => {
                    o.insert("kind".into(), json!("choice"));
                    o.insert(
                        "choices".into(),
                        choices
                            .iter()
                            .map(|(id, label)| json!({"id": id, "label": label}))
                            .collect(),
                    );
                }
                FieldKind::Bool => {
                    o.insert("kind".into(), json!("bool"));
                }
                FieldKind::Int { min, max, unit } => {
                    o.insert("kind".into(), json!("int"));
                    o.insert("min".into(), json!(min));
                    o.insert("max".into(), json!(max));
                    o.insert("unit".into(), json!(unit));
                }
                FieldKind::Face => {
                    o.insert("kind".into(), json!("face"));
                }
                FieldKind::Coord => {
                    o.insert("kind".into(), json!("coord"));
                }
            }
            v
        })
        .collect();

    Ok(json!({
        "path": paths.config_path.display().to_string(),
        "fields": fields,
        "faces": faces_json(&faces),
    }))
}

/// `design` の選択肢。**メニューに並べていたプレビューをそのまま画面へ持ってきた**
/// もので、絵は daemon が描くのと同じ `face::svg` の出力。
fn faces_json(faces: &Registry) -> Value {
    let builtin: Vec<&str> = ccsessions_core::face::builtin_ids();
    Value::Array(
        faces
            .all()
            .iter()
            .map(|f| {
                json!({
                    "id": f.id,
                    "label": f.display_label(),
                    "builtin": builtin.contains(&f.id.as_str()),
                    "svg": svg::render_chip(f, SessionState::Working, Size::Dock),
                })
            })
            .collect(),
    )
}

/// 設定を 1 項目だけ書き換える。
///
/// **1 項目ずつなのは、画面が持っている設定で丸ごと上書きしないため。** 設定は
/// daemon 側も書く（dock をドラッグして決めた位置）ので、画面を開いたときの
/// スナップショットを丸ごと書き戻すと、その間に動かした位置が黙って巻き戻る。
///
/// 値は**すべて文字列**で受ける。`config::set_field` が CLI と同じ検証を掛けるので、
/// 型ごとの分岐と「画面だけ緩い」検証がここに生えない。
fn set_config(paths: &Paths, body: &str) -> Result<Value, String> {
    let v: Value = serde_json::from_str(body).map_err(|e| format!("JSON として読めません: {e}"))?;
    let key = v["key"].as_str().ok_or("key がありません")?;
    // 数値・真偽値でそのまま送られてきても受ける（画面側の取り違えで
    // 「保存できない」になるより、素直に文字列化する方が親切）。
    let value = match &v["value"] {
        Value::String(s) => s.clone(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::Null => "auto".to_string(),
        other => return Err(format!("value が扱えない型です: {other}")),
    };

    let mut cfg = config::load(&paths.config_path)?;
    let faces = Registry::load_in(&paths.faces_dir);
    config::set_field(&mut cfg, key, &value, &faces)?;
    config::save(&paths.config_path, &cfg)
        .map_err(|e| format!("{} に書けません: {e}", paths.config_path.display()))?;

    Ok(json!({
        "key": key,
        "value": config::field_value(&cfg, key),
        "message": format!("{key} を {value} にした"),
    }))
}

// ---------------------------------------------------------------------------
// /api/parts — レジストリをそのまま JSON にする
// ---------------------------------------------------------------------------

/// UI が列挙するための唯一の情報源。
///
/// **パーツを足すときに触るのは `face::builder::parts` の表だけ**で、
/// ここも JS も触らなくてよい、というのがこの経路の目的。
fn parts_json() -> Value {
    use ccsessions_core::face::builder::parts;

    let mut categories = vec![
        json!({
            "id": "face",
            "label": "顔のライン",
            "kind": "face",
            "variants": parts::FACES.iter()
                .map(|p| json!({"id": p.id, "label": p.label}))
                .collect::<Vec<_>>(),
        }),
        json!({
            "id": "eyes",
            "label": "目",
            "kind": "eyes",
            "variants": parts::EYES.iter()
                .map(|p| json!({"id": p.id, "label": p.label}))
                .collect::<Vec<_>>(),
        }),
    ];
    for c in parts::LINES {
        categories.push(json!({
            "id": c.id,
            "label": c.label,
            "kind": "line",
            "on_bar": c.on_bar,
            "variants": c.variants.iter()
                .map(|p| json!({"id": p.id, "label": p.label}))
                .collect::<Vec<_>>(),
        }));
    }

    json!({
        "version": builder::CONFIG_VERSION,
        "categories": categories,
        "states": SessionState::ORDER.iter()
            .map(|s| json!({"id": s.as_str(), "label": s.ja(), "glyph": s.glyph()}))
            .collect::<Vec<_>>(),
        "eye_colors": [
            {"id": EyeColor::Eye.as_str(), "label": "標準（明るい）"},
            {"id": EyeColor::White.as_str(), "label": "白"},
            {"id": EyeColor::EyeClosed.as_str(), "label": "くすんだ色"},
            {"id": EyeColor::EyeError.as_str(), "label": "赤"},
        ],
        "default": CharacterConfig::default(),
    })
}

// ---------------------------------------------------------------------------
// /api/preview
// ---------------------------------------------------------------------------

fn parse_config(body: &str) -> Result<CharacterConfig, String> {
    CharacterConfig::from_json(body)
}

fn preview(body: &str) -> Result<Value, String> {
    let cfg = parse_config(body)?;
    let c = builder::compose(&cfg);

    let state = SessionState::from_str(&cfg.preview.state).unwrap_or(SessionState::Working);
    let size = if cfg.preview.size == "bar" {
        Size::Bar
    } else {
        Size::Dock
    };

    Ok(json!({
        "toml": c.toml,
        "eye_fit": c.eye_fit,
        "problems": c.problems.iter()
            .map(|p| json!({"code": p.code.as_str(), "message": p.message}))
            .collect::<Vec<_>>(),
        "warning": c.warning.map(|p| json!({"code": p.code.as_str(), "message": p.message})),
        "main": svg::render(&c.spec, state, size),
        // 6 状態を並べて見せる。色とアニメは状態が決めるので、
        // 「色を選ぶ」の代わりにここで実際の配色を確かめてもらう。
        "states": SessionState::ORDER.iter()
            .map(|s| json!({
                "id": s.as_str(),
                "label": s.ja(),
                "svg": svg::render_chip(&c.spec, *s, Size::Dock),
            }))
            .collect::<Vec<_>>(),
        "thumbs": thumbs(&cfg, state),
    }))
}

/// パーツ選択のサムネイル。**いま作っている顔にそのパーツだけ載せ替えた絵**を
/// 返す（カタログの絵ではない）ので、「この髪を選んだらどう見えるか」が
/// 選ぶ前に分かる。
fn thumbs(cfg: &CharacterConfig, state: SessionState) -> Value {
    use ccsessions_core::face::builder::parts;

    let one = |category: &str, id: &str| -> String {
        let mut c = cfg.clone();
        c.parts.insert(category.to_string(), id.to_string());
        svg::render_chip(&builder::compose(&c).spec, state, Size::Dock)
    };

    let mut out = serde_json::Map::new();
    out.insert(
        "face".into(),
        Value::Array(
            parts::FACES
                .iter()
                .map(|p| json!({"id": p.id, "svg": one("face", p.id)}))
                .collect(),
        ),
    );
    out.insert(
        "eyes".into(),
        Value::Array(
            parts::EYES
                .iter()
                .map(|p| json!({"id": p.id, "svg": one("eyes", p.id)}))
                .collect(),
        ),
    );
    for cat in parts::LINES {
        out.insert(
            cat.id.into(),
            Value::Array(
                cat.variants
                    .iter()
                    .map(|p| json!({"id": p.id, "svg": one(cat.id, p.id)}))
                    .collect(),
            ),
        );
    }
    Value::Object(out)
}

// ---------------------------------------------------------------------------
// /api/random
// ---------------------------------------------------------------------------

fn random(body: &str, query: &str) -> Result<Value, String> {
    let keep = parse_config(body)?;
    // 種はクライアントが持つ（同じ種で同じ顔が出る＝「さっきの顔に戻す」が効く）。
    let seed: u64 = param(query, "seed")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    serde_json::to_value(builder::random(seed, &keep))
        .map_err(|e| format!("設定を JSON にできません: {e}"))
}

// ---------------------------------------------------------------------------
// /api/save · /api/saved · /api/load
// ---------------------------------------------------------------------------

fn save(dir: &Path, body: &str) -> Result<Value, String> {
    let cfg = parse_config(body)?;
    let id = cfg.id.trim().to_string();
    if !builder::is_saveable_id(&id) {
        return Err(format!(
            "id {id:?} は使えません。英小文字・数字・ハイフンだけで、\
             先頭は英数字、32 文字以内にしてください（例: \"my-face\"）"
        ));
    }

    let c = builder::compose(&cfg);
    if !c.problems.is_empty() {
        // 壊れた顔を顔ディレクトリに置くと、`ccsessionsd` が起動のたびに
        // エラーを吐き続ける。保存の時点で止める。
        return Err(format!(
            "検証に通っていないので保存しません:\n{}",
            c.problems
                .iter()
                .map(|p| format!("  [{}] {}", p.code, p.message))
                .collect::<Vec<_>>()
                .join("\n")
        ));
    }

    // `id` は上で `[a-z0-9-]+` に絞ってあるので、パス区切りは入り得ない。
    let path = dir.join(format!("{id}.toml"));
    ccsessions_core::write_atomic(&path, &c.toml)
        .map_err(|e| format!("{} に書けません: {e}", path.display()))?;

    Ok(json!({
        "id": id,
        "path": path.display().to_string(),
        "message": format!(
            "{} に保存しました。走っている ccsessionsd が数百 ms で拾います",
            path.display()
        ),
    }))
}

/// 顔ディレクトリに置いてある、**ビルダーで作った顔**の一覧。
///
/// 手書きの顔も列挙するが `editable: false` にする。30 種の素形のどれとも
/// 一致しない形はパーツに逆変換できないので、「読み込めます」と嘘をつかない。
fn saved_json(dir: &Path) -> Value {
    let mut items = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return json!({ "items": items, "dir": dir.display().to_string() });
    };
    let mut paths: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "toml") && p.is_file())
        .collect();
    paths.sort();

    for p in paths {
        let Ok(text) = std::fs::read_to_string(&p) else {
            continue;
        };
        let cfg = builder::config_from_toml(&text);
        let id = p
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        items.push(json!({
            "id": id,
            "name": cfg.as_ref().map(|c| c.name.clone()),
            "editable": cfg.is_some(),
            "path": p.display().to_string(),
        }));
    }
    json!({ "items": items, "dir": dir.display().to_string() })
}

fn load_saved(dir: &Path, query: &str) -> Result<Value, String> {
    let id = param(query, "id").ok_or("id がありません")?;
    if !builder::is_saveable_id(&id) {
        return Err(format!("id {id:?} は使えません"));
    }
    let path = dir.join(format!("{id}.toml"));
    let text = std::fs::read_to_string(&path)
        .map_err(|e| format!("{} を読めません: {e}", path.display()))?;
    let cfg = builder::config_from_toml(&text).ok_or_else(|| {
        format!(
            "{} はビルダーで作った顔ではないので読み込めません\
             （手で書いた顔をパーツに戻すことはできません）",
            path.display()
        )
    })?;
    serde_json::to_value(cfg).map_err(|e| format!("設定を JSON にできません: {e}"))
}

/// `a=1&b=2` から値を 1 つ取り出す（パーセントデコードつき）。
fn param(query: &str, key: &str) -> Option<String> {
    query.split('&').find_map(|kv| {
        let (k, v) = kv.split_once('=')?;
        (k == key).then(|| percent_decode(v))
    })
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'%' if i + 2 < b.len() => {
                let hex = std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or("");
                match u8::from_str_radix(hex, 16) {
                    Ok(v) => {
                        out.push(v);
                        i += 3;
                    }
                    Err(_) => {
                        out.push(b[i]);
                        i += 1;
                    }
                }
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// テストが使うポート。`guard` の Origin 判定と揃える。
    const PORT: u16 = 8787;

    /// 素のリクエスト（自分のページから来た正常なもの）。
    /// CSRF のテストはこれを 1 か所だけ書き換えて作る。
    fn req(method: &str, path: &str, body: &str) -> Request {
        Request {
            method: method.into(),
            path: path.split('?').next().unwrap().into(),
            query: path
                .split_once('?')
                .map(|(_, q)| q.into())
                .unwrap_or_default(),
            host: format!("127.0.0.1:{PORT}"),
            origin: if method == "GET" {
                // 同一オリジンの GET にブラウザは Origin を付けない。
                String::new()
            } else {
                format!("http://127.0.0.1:{PORT}")
            },
            content_type: if method == "GET" {
                String::new()
            } else {
                "application/json".into()
            },
            body: body.into(),
        }
    }

    /// テストは**本物の `~/.config/ccsessions` に絶対に触らない**（設定も顔も
    /// tempdir に閉じる）。`settings.json` のテストと同じ規律。
    fn paths(dir: &Path) -> Paths {
        Paths {
            faces_dir: dir.to_path_buf(),
            // 顔ディレクトリの**外**に置く。同じ階層に置くと `config.toml` が
            // 顔の一覧に「手書きの顔」として現れてしまう（本番の配置でも
            // `~/.config/ccsessions/config.toml` と `faces/` は別階層）。
            config_path: dir.join("state").join("config.toml"),
        }
    }

    fn send(r: Request, dir: &Path) -> (u16, String) {
        let (s, _, b) = route(&r, &paths(dir), PORT);
        (s, b)
    }

    fn get(path: &str, dir: &Path) -> (u16, String) {
        send(req("GET", path, ""), dir)
    }

    fn post(path: &str, body: &str, dir: &Path) -> (u16, String) {
        send(req("POST", path, body), dir)
    }

    fn default_json() -> String {
        CharacterConfig::default().to_json_pretty()
    }

    /// `/api/parts` にレジストリが全部出る（UI はこれだけを見て描く）。
    #[test]
    fn the_registry_is_served_whole() {
        let dir = TempDir::new().unwrap();
        let (s, body) = get("/api/parts", dir.path());
        assert_eq!(s, 200);
        let v: Value = serde_json::from_str(&body).unwrap();
        let cats = v["categories"].as_array().unwrap();
        assert_eq!(
            cats.len(),
            2 + ccsessions_core::face::builder::parts::LINES.len()
        );
        // **数を literal で書かない** — 表に 1 行足すたびにここが落ちると、
        // 「レジストリに 1 行足すだけ」という設計そのものを罰することになる。
        // 見たいのは「表がまるごと届いているか」なので、表と突き合わせる。
        let expected = |id: &str| -> usize {
            match id {
                "face" => ccsessions_core::face::builder::parts::FACES.len(),
                "eyes" => ccsessions_core::face::builder::parts::EYES.len(),
                _ => ccsessions_core::face::builder::parts::category(id)
                    .expect("表に無いカテゴリが配信されている")
                    .variants
                    .len(),
            }
        };
        for c in cats {
            let id = c["id"].as_str().unwrap();
            assert_eq!(
                c["variants"].as_array().unwrap().len(),
                expected(id),
                "{id} のバリエーション数"
            );
        }
        assert_eq!(v["states"].as_array().unwrap().len(), 6);
    }

    /// プレビューは SVG・TOML・検証結果・サムネイルを返す。
    #[test]
    fn preview_returns_everything_the_ui_needs() {
        let dir = TempDir::new().unwrap();
        let (s, body) = post("/api/preview", &default_json(), dir.path());
        assert_eq!(s, 200, "{body}");
        let v: Value = serde_json::from_str(&body).unwrap();
        assert!(v["main"].as_str().unwrap().starts_with("<svg "));
        assert!(v["toml"].as_str().unwrap().contains("[outline]"));
        assert!(v["problems"].as_array().unwrap().is_empty());
        assert_eq!(v["states"].as_array().unwrap().len(), 6);
        // サムネイルは「そのカテゴリの全バリエーション」ぶん出る（数は表しだい）。
        use ccsessions_core::face::builder::parts;
        assert_eq!(
            v["thumbs"]["face"].as_array().unwrap().len(),
            parts::FACES.len()
        );
        assert_eq!(
            v["thumbs"]["mouth"].as_array().unwrap().len(),
            parts::category("mouth").unwrap().variants.len()
        );
        // 後から生えたカテゴリもサムネイルまで届いている。
        assert_eq!(
            v["thumbs"]["side"].as_array().unwrap().len(),
            parts::category("side").unwrap().variants.len()
        );
    }

    /// 壊れた JSON は 400 で、理由が返る。
    #[test]
    fn a_broken_body_is_reported_not_crashed() {
        let dir = TempDir::new().unwrap();
        let (s, body) = post("/api/preview", "{not json", dir.path());
        assert_eq!(s, 400);
        assert!(body.contains("error"));
    }

    /// 保存 → 一覧 → 読み込みが一周する。
    #[test]
    fn saving_then_loading_restores_the_same_config() {
        let dir = TempDir::new().unwrap();
        let mut cfg = CharacterConfig {
            id: "test-face".into(),
            ..CharacterConfig::default()
        };
        cfg.parts.insert("hair".into(), "spike4".into());

        let (s, body) = post("/api/save", &cfg.to_json_pretty(), dir.path());
        assert_eq!(s, 200, "{body}");
        assert!(dir.path().join("test-face.toml").is_file());

        let (s, body) = get("/api/saved", dir.path());
        assert_eq!(s, 200);
        let v: Value = serde_json::from_str(&body).unwrap();
        let items = v["items"].as_array().unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["id"], "test-face");
        assert_eq!(items[0]["editable"], true);

        let (s, body) = get("/api/load?id=test-face", dir.path());
        assert_eq!(s, 200, "{body}");
        let back: CharacterConfig = serde_json::from_str(&body).unwrap();
        assert_eq!(back, cfg);
    }

    /// **保存した TOML はそのまま顔として読める**（顔ディレクトリに置く以上、
    /// これが崩れると daemon が起動のたびにエラーを吐く）。
    #[test]
    fn the_saved_file_is_a_loadable_face() {
        let dir = TempDir::new().unwrap();
        let cfg = CharacterConfig {
            id: "loadable".into(),
            ..CharacterConfig::default()
        };
        assert_eq!(post("/api/save", &cfg.to_json_pretty(), dir.path()).0, 200);

        let reg = ccsessions_core::face::Registry::load_in(dir.path());
        assert!(reg.problems().is_empty(), "{:?}", reg.problems());
        assert!(reg.get("loadable").is_some(), "顔として読めていない");
    }

    /// 危ない id はファイルを作らせない。
    #[test]
    fn a_dangerous_id_is_refused() {
        let dir = TempDir::new().unwrap();
        for bad in ["../evil", "a/b", "", "UPPER", "with space"] {
            let cfg = CharacterConfig {
                id: bad.into(),
                ..CharacterConfig::default()
            };
            let (s, body) = post("/api/save", &cfg.to_json_pretty(), dir.path());
            assert_eq!(s, 400, "{bad:?} が通ってしまった: {body}");
        }
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    /// 検証に落ちる顔は保存しない。
    #[test]
    fn an_invalid_face_is_not_written() {
        let dir = TempDir::new().unwrap();
        // label が空だと parse に落ちる。
        let cfg = CharacterConfig {
            id: "broken".into(),
            name: "   ".into(),
            ..CharacterConfig::default()
        };
        let (s, body) = post("/api/save", &cfg.to_json_pretty(), dir.path());
        assert_eq!(s, 400, "{body}");
        assert!(!dir.path().join("broken.toml").exists());
    }

    /// ランダムは種で決まり、保存できる顔になる。
    #[test]
    fn random_is_seeded_and_valid() {
        let dir = TempDir::new().unwrap();
        let (s, a) = post("/api/random?seed=42", &default_json(), dir.path());
        assert_eq!(s, 200);
        let (_, b) = post("/api/random?seed=42", &default_json(), dir.path());
        assert_eq!(a, b, "同じ種で違う顔が出る");
        let (_, c) = post("/api/random?seed=43", &default_json(), dir.path());
        assert_ne!(a, c, "種を変えても同じ顔が出る");

        let cfg: CharacterConfig = serde_json::from_str(&a).unwrap();
        let (s, body) = post("/api/preview", &cfg.to_json_pretty(), dir.path());
        assert_eq!(s, 200);
        let v: Value = serde_json::from_str(&body).unwrap();
        assert!(v["problems"].as_array().unwrap().is_empty(), "{body}");
    }

    /// 手書きの顔は「編集できない」と正直に返す。
    #[test]
    fn a_handwritten_face_is_listed_but_not_editable() {
        let dir = TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("hand.toml"),
            include_str!("../../faces/egg.toml"),
        )
        .unwrap();
        let (_, body) = get("/api/saved", dir.path());
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["items"][0]["editable"], false);

        let (s, body) = get("/api/load?id=hand", dir.path());
        assert_eq!(s, 400);
        assert!(body.contains("ビルダーで作った顔ではない"), "{body}");
    }

    // ---- 設定 ---------------------------------------------------------------
    //
    // 設定の入口はここ 1 つになった（メニューバーの status item は消した）ので、
    // 「画面に出る項目」と「保存できる項目」がずれないことをここで押さえる。

    /// 設定画面が描くのに要るものが全部返る。**キーは core のスキーマが唯一の
    /// 情報源**なので、数を literal で書かず表と突き合わせる。
    #[test]
    fn the_config_schema_is_served_with_current_values() {
        let dir = TempDir::new().unwrap();
        let (s, body) = get("/api/config", dir.path());
        assert_eq!(s, 200, "{body}");
        let v: Value = serde_json::from_str(&body).unwrap();

        let fields = v["fields"].as_array().unwrap();
        assert_eq!(fields.len(), ccsessions_core::config::fields().len());
        for (got, want) in fields.iter().zip(ccsessions_core::config::fields()) {
            assert_eq!(got["key"], want.key);
            assert!(!got["label"].as_str().unwrap().is_empty());
            assert!(got["value"].is_string(), "{} に現在値が無い", want.key);
        }
        // 設定ファイルがまだ無ければ組込みデフォルトが見えること（初回起動）。
        let placement = fields.iter().find(|f| f["key"] == "placement").unwrap();
        assert_eq!(placement["value"], "bar");
        assert_eq!(placement["kind"], "choice");

        // 顔の選択肢はプレビュー付きで来る（メニューのプレビューを引き継いだ）。
        let faces = v["faces"].as_array().unwrap();
        assert_eq!(faces.len(), ccsessions_core::face::builtin_ids().len());
        assert!(faces[0]["svg"].as_str().unwrap().starts_with("<svg "));
        assert_eq!(faces[0]["builtin"], true);
    }

    /// 保存すると**設定ファイルに書かれ**、次に読むとその値が返る
    /// （daemon はこのファイルの mtime を見ているので、これが反映経路そのもの）。
    #[test]
    fn setting_a_field_writes_the_config_file() {
        let dir = TempDir::new().unwrap();
        let (s, body) = post(
            "/api/config",
            r#"{"key":"placement","value":"dock"}"#,
            dir.path(),
        );
        assert_eq!(s, 200, "{body}");

        let p = paths(dir.path()).config_path;
        let written = std::fs::read_to_string(&p).unwrap();
        assert!(written.contains("placement = \"dock\""), "{written}");

        let (_, body) = get("/api/config", dir.path());
        let v: Value = serde_json::from_str(&body).unwrap();
        let f = v["fields"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["key"] == "placement")
            .unwrap();
        assert_eq!(f["value"], "dock");
    }

    /// **1 項目ずつ書く**ので、他の項目は触らない。画面が持っているスナップショットで
    /// 丸ごと上書きすると、その間に daemon が書いた dock の位置が黙って消える。
    #[test]
    fn setting_one_field_leaves_the_others_alone() {
        let dir = TempDir::new().unwrap();
        let p = paths(dir.path()).config_path;
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, "placement = \"dock\"\ndock_x = 500.0\ndock_y = 20.0\n").unwrap();

        assert_eq!(
            post(
                "/api/config",
                r#"{"key":"reduce_motion","value":"true"}"#,
                dir.path()
            )
            .0,
            200
        );
        let cfg = ccsessions_core::config::load(&p).unwrap();
        assert!(cfg.reduce_motion);
        assert_eq!((cfg.dock_x, cfg.dock_y), (Some(500.0), Some(20.0)));
    }

    /// 真偽値・数値をそのまま送っても受ける（画面側の型の取り違えで詰まらせない）。
    #[test]
    fn a_json_bool_or_number_is_accepted_as_a_value() {
        let dir = TempDir::new().unwrap();
        assert_eq!(
            post(
                "/api/config",
                r#"{"key":"show_glyphs","value":false}"#,
                dir.path()
            )
            .0,
            200
        );
        assert_eq!(
            post(
                "/api/config",
                r#"{"key":"max_sessions","value":6}"#,
                dir.path()
            )
            .0,
            200
        );
        let cfg = ccsessions_core::config::load(&paths(dir.path()).config_path).unwrap();
        assert!(!cfg.show_glyphs);
        assert_eq!(cfg.max_sessions, 6);
    }

    /// 不正な値・未知のキーは 400 で、**理由が人間に読める形で返る**
    /// （検証は core の `set_field` ＝ CLI と同じもの）。
    #[test]
    fn an_invalid_setting_is_refused_with_a_reason() {
        let dir = TempDir::new().unwrap();
        for (body, want) in [
            (r#"{"key":"placement","value":"floating"}"#, "placement"),
            (r#"{"key":"design","value":"no-such-face"}"#, "design"),
            (r#"{"key":"max_sessions","value":"0"}"#, "max_sessions"),
            (r#"{"key":"nope","value":"1"}"#, "nope"),
        ] {
            let (s, got) = post("/api/config", body, dir.path());
            assert_eq!(s, 400, "{body} が通ってしまった");
            assert!(got.contains(want), "理由が分からない: {got}");
        }
        assert!(
            !paths(dir.path()).config_path.exists(),
            "拒否したのに設定ファイルを作っている"
        );
    }

    /// 保存した自作の顔は `design` の選択肢に出て、そのまま設定できる。
    #[test]
    fn a_face_saved_here_can_be_selected_as_the_design() {
        let dir = TempDir::new().unwrap();
        let cfg = CharacterConfig {
            id: "my-face".into(),
            ..CharacterConfig::default()
        };
        assert_eq!(post("/api/save", &cfg.to_json_pretty(), dir.path()).0, 200);

        let (_, body) = get("/api/config", dir.path());
        let v: Value = serde_json::from_str(&body).unwrap();
        let mine = v["faces"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["id"] == "my-face")
            .expect("保存した顔が選択肢に出ていない");
        assert_eq!(mine["builtin"], false);

        let (s, body) = post(
            "/api/config",
            r#"{"key":"design","value":"my-face"}"#,
            dir.path(),
        );
        assert_eq!(s, 200, "{body}");
    }

    /// 設定の書き込みも他所のページからは叩けない（顔の保存と同じ守り）。
    #[test]
    fn a_cross_origin_config_write_is_refused() {
        let dir = TempDir::new().unwrap();
        let mut r = req(
            "POST",
            "/api/config",
            r#"{"key":"placement","value":"dock"}"#,
        );
        r.origin = "https://evil.example".into();
        assert_eq!(send(r, dir.path()).0, 403);
        assert!(!paths(dir.path()).config_path.exists(), "書かれてしまった");
    }

    /// 無いパスは 404。
    #[test]
    fn unknown_paths_are_404() {
        let dir = TempDir::new().unwrap();
        assert_eq!(get("/nope", dir.path()).0, 404);
    }

    // ---- CSRF -------------------------------------------------------------
    //
    // `/api/save` はファイルを書くので、**ユーザがたまたま開いた別のページ**から
    // 叩けてはいけない。127.0.0.1 に bind してあるだけでは足りない
    // （攻撃ページの fetch でも、ブラウザは Host をこちらの名前で送る）。

    /// 他所のページからの書き込みは拒否され、**ファイルができない**。
    #[test]
    fn a_cross_origin_write_is_refused() {
        let dir = TempDir::new().unwrap();
        let cfg = CharacterConfig {
            id: "csrf-poc".into(),
            ..CharacterConfig::default()
        };
        let mut r = req("POST", "/api/save", &cfg.to_json_pretty());
        r.origin = "https://evil.example".into();

        let (status, body) = send(r, dir.path());
        assert_eq!(status, 403, "{body}");
        assert!(
            !dir.path().join("csrf-poc.toml").exists(),
            "書かれてしまった"
        );
    }

    /// **プリフライトを迂回する手（単純リクエスト）を塞ぐ。**
    ///
    /// `Content-Type: text/plain` は CORS の「単純リクエスト」なのでプリフライトが
    /// 飛ばない。Origin だけを見ていると、Origin を送らない経路（フォーム送信等）が
    /// 残る。JSON を要求すればブラウザは必ずプリフライトを送り、こちらは CORS を
    /// 一切許可していないので本番のリクエストが発射されない。
    #[test]
    fn a_write_with_a_simple_content_type_is_refused() {
        let dir = TempDir::new().unwrap();
        let cfg = CharacterConfig {
            id: "csrf-simple".into(),
            ..CharacterConfig::default()
        };
        for ct in [
            "text/plain;charset=UTF-8",
            "application/x-www-form-urlencoded",
            "multipart/form-data; boundary=x",
            "",
        ] {
            let mut r = req("POST", "/api/save", &cfg.to_json_pretty());
            // Origin を消して「フォーム送信」の形にしても通らないこと。
            r.origin = String::new();
            r.content_type = ct.into();
            let (status, body) = send(r, dir.path());
            assert_eq!(status, 415, "{ct:?} が通ってしまった: {body}");
        }
        assert!(!dir.path().join("csrf-simple.toml").exists());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    /// 自分のページからの書き込みは通る（門番が厳しすぎないこと）。
    /// `localhost` で開いた場合も同じ。
    #[test]
    fn a_same_origin_write_still_works() {
        let dir = TempDir::new().unwrap();
        let cfg = CharacterConfig {
            id: "same-origin".into(),
            ..CharacterConfig::default()
        };
        for (host, origin) in [
            (
                format!("127.0.0.1:{PORT}"),
                format!("http://127.0.0.1:{PORT}"),
            ),
            (
                format!("localhost:{PORT}"),
                format!("http://localhost:{PORT}"),
            ),
            // curl 等は Origin を送らない。CSRF は「被害者のブラウザを使う」
            // 攻撃なので、送らない相手は脅威モデルの外。
            (format!("127.0.0.1:{PORT}"), String::new()),
        ] {
            let mut r = req("POST", "/api/save", &cfg.to_json_pretty());
            r.host = host.clone();
            r.origin = origin.clone();
            let (status, body) = send(r, dir.path());
            assert_eq!(status, 200, "host={host} origin={origin:?}: {body}");
        }
        assert!(dir.path().join("same-origin.toml").is_file());
    }

    /// Host を偽った DNS リバインディングも拒否される（既存の一枚）。
    #[test]
    fn a_rebound_host_is_refused() {
        let dir = TempDir::new().unwrap();
        let mut r = req("GET", "/api/parts", "");
        r.host = "evil.example".into();
        assert_eq!(send(r, dir.path()).0, 403);
    }

    /// **ポートが違うオリジンも他所扱い**（同じ 127.0.0.1 で別のツールが
    /// 動いている場合を通さない）。
    #[test]
    fn another_port_on_localhost_is_still_cross_origin() {
        let dir = TempDir::new().unwrap();
        let mut r = req("POST", "/api/save", &default_json());
        r.origin = format!("http://127.0.0.1:{}", PORT + 1);
        assert_eq!(send(r, dir.path()).0, 403);
        assert!(is_own_origin(&format!("http://localhost:{PORT}"), PORT));
        assert!(!is_own_origin("http://127.0.0.1.evil.example", PORT));
    }

    #[test]
    fn query_parameters_are_decoded() {
        assert_eq!(param("id=my%2Dface&x=1", "id").as_deref(), Some("my-face"));
        assert_eq!(param("a=1", "b"), None);
        assert_eq!(percent_decode("a+b%20c"), "a b c");
    }
}
