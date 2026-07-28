//! 組み立てた数値を **`faces/*.toml` のテキスト**にする。
//!
//! # なぜ「テキストを作ってから読み直す」のか
//! `compose` は `Draft` → TOML テキスト → `parse::parse` → `FaceSpec` の順で
//! 顔を作る。`FaceSpec` を直接組んでから「ついでに TOML も書き出す」ほうが
//! 一見素直だが、それだと**画面で見た顔と保存される顔が別物になりうる**:
//! 座標を `{:.4}` に丸めた時点で保存側だけ値が動くし、書き出しの取りこぼし
//! （`radius` を書き忘れる等）は保存するまで誰も気づかない。
//!
//! テキストを唯一の中間形にすれば、**プレビューは保存されるファイルそのものの
//! 描画**になる。ついでに「投稿者が手で書ける TOML」から外れた顔を作れなく
//! なるので、ビルダーの出力が `faces/README.md` のスキーマから逸れない。

use serde::Serialize;

/// 輪郭の書き出し方（`[outline]` の 3 種にそのまま対応）。
#[derive(Debug, Clone, PartialEq)]
pub enum OutlineDraft {
    /// CSS の border-radius 順（左上・右上・右下・左下）で `[水平, 垂直]`。
    Corners([[f64; 2]; 4]),
    Capsule,
    /// `d` は 0..1 比率で書いた `M` / `C` の列。
    Path {
        half: bool,
        d: String,
    },
}

/// パネル線 1 本。
#[derive(Debug, Clone, PartialEq)]
pub struct DetailDraft {
    pub name: String,
    /// bar にも描くか。`false` なら `sizes = ["dock"]`。
    /// **bar ⊆ dock を構造的に保つ**ので `bar-details-not-thinned` に落ちない。
    pub on_bar: bool,
    pub points: Vec<[f64; 2]>,
}

/// TOML 1 ファイルぶんの数値。
#[derive(Debug, Clone, PartialEq)]
pub struct Draft {
    pub id: String,
    pub label: String,
    pub author: Option<String>,
    /// bar / dock の体の寸法 (w, h)。
    pub bar: (f64, f64),
    pub dock: (f64, f64),
    pub outline: OutlineDraft,
    /// 目の多角形（`None` なら角丸矩形）。
    pub eye_polygon: Option<Vec<[f64; 2]>>,
    pub eye_v: f64,
    /// (bar, dock) の間隔（pt）。
    pub eye_gap: (f64, f64),
    /// (bar, dock) の [w, h]（pt）。
    pub eye_size: ([f64; 2], [f64; 2]),
    /// 角丸半径（pt）。多角形の目では書き出さない。
    pub eye_radius: f64,
    /// 目の色。`None` なら `[eyes.states.*]` を書かず既定ルールに任せる。
    pub eye_color: Option<&'static str>,
    pub details: Vec<DetailDraft>,
    /// 先頭コメントに載せる説明（1 要素 1 行）。
    pub notes: Vec<String>,
    /// 先頭コメントに埋める、ビルダーの設定そのもの（1 行 JSON）。
    /// これがあるおかげで**保存した TOML をビルダーに読み戻せる**。
    pub config_json: Option<String>,
}

/// 生成した TOML の先頭コメントに埋める目印。ビルダーが読み戻すときの鍵。
pub const CONFIG_MARKER: &str = "# ccchar: ";

/// TOML テキストにする。
pub fn to_toml(d: &Draft) -> String {
    let mut s = String::new();

    s.push_str("# ccsessions キャラクタービルダーが生成した顔。\n");
    s.push_str("# `make config`（ccsessions ui）の「キャラクター」で読み込めば、\n");
    s.push_str("# この組み合わせから編集を続けられる。\n");
    if !d.notes.is_empty() {
        s.push_str("#\n");
        for n in &d.notes {
            s.push_str("# ");
            s.push_str(&n.replace('\n', " "));
            s.push('\n');
        }
    }
    if let Some(j) = &d.config_json {
        s.push_str("#\n");
        s.push_str(CONFIG_MARKER);
        s.push_str(&j.replace('\n', " "));
        s.push('\n');
    }
    s.push('\n');

    s.push_str(&format!("id       = {}\n", quote(&d.id)));
    s.push_str(&format!("label    = {}\n", quote(&d.label)));
    if let Some(a) = &d.author {
        if !a.trim().is_empty() {
            s.push_str(&format!("author   = {}\n", quote(a)));
        }
    }

    s.push_str("\n[size]\n");
    s.push_str(&format!(
        "bar  = {{ w = {}, h = {} }}\n",
        num(d.bar.0),
        num(d.bar.1)
    ));
    s.push_str(&format!(
        "dock = {{ w = {}, h = {} }}\n",
        num(d.dock.0),
        num(d.dock.1)
    ));

    s.push_str("\n[outline]\n");
    match &d.outline {
        OutlineDraft::Corners(c) => {
            s.push_str("kind = \"corners\"\n");
            s.push_str(&format!(
                "corners = [[{}, {}], [{}, {}], [{}, {}], [{}, {}]]\n",
                num(c[0][0]),
                num(c[0][1]),
                num(c[1][0]),
                num(c[1][1]),
                num(c[2][0]),
                num(c[2][1]),
                num(c[3][0]),
                num(c[3][1]),
            ));
        }
        OutlineDraft::Capsule => s.push_str("kind = \"capsule\"\n"),
        OutlineDraft::Path { half, d: path } => {
            s.push_str("kind = \"path\"\n");
            s.push_str(&format!("half = {half}\n"));
            s.push_str("d = \"\"\"\n");
            s.push_str(path);
            if !path.ends_with('\n') {
                s.push('\n');
            }
            s.push_str("\"\"\"\n");
        }
    }

    s.push_str("\n[eyes]\n");
    let poly = d.eye_polygon.as_ref();
    s.push_str(&format!(
        "shape  = \"{}\"\n",
        if poly.is_some() { "polygon" } else { "rounded" }
    ));
    s.push_str(&format!("v      = {}\n", num(d.eye_v)));
    s.push_str(&format!(
        "gap    = {{ bar = {}, dock = {} }}\n",
        num(d.eye_gap.0),
        num(d.eye_gap.1)
    ));
    s.push_str(&format!(
        "size   = {{ bar = [{}, {}], dock = [{}, {}] }}\n",
        num(d.eye_size.0[0]),
        num(d.eye_size.0[1]),
        num(d.eye_size.1[0]),
        num(d.eye_size.1[1]),
    ));
    match poly {
        // `shape = "rounded"` に polygon を書くとパースエラーになるので、
        // radius と polygon は排他で出す。
        None => s.push_str(&format!("radius = {}\n", num(d.eye_radius))),
        Some(p) => {
            s.push_str("polygon = [\n");
            for q in p {
                s.push_str(&format!("  [{}, {}],\n", num(q[0]), num(q[1])));
            }
            s.push_str("]\n");
        }
    }

    // 目の色を変えたときだけ状態を明示する。
    //
    // 既定ルールを**丸ごと置き換える**仕様（`EyeOverride` の doc）なので、
    // 色だけ差し替えるつもりで書くと瞬きや横目が消える。既定と同じ挙動を
    // 書き下したうえで色を足す。触るのは「既定の色が eye である 3 状態」だけで、
    // 見開き（白）・アイドル（暗色）・エラー（赤）はそのままにする
    // — そこまで塗ると状態が読めなくなる。
    if let Some(color) = d.eye_color {
        s.push_str("\n# 目の色を変えたので、既定ルールと同じ挙動を書き下したうえで色を足す。\n");
        s.push_str("[eyes.states.working]\nblink = true\n");
        s.push_str(&format!("color = \"{color}\"\n"));
        s.push_str("\n[eyes.states.wait_agent]\ndx = 1.5\n");
        s.push_str(&format!("color = \"{color}\"\n"));
        s.push_str("\n[eyes.states.done]\n");
        s.push_str(&format!("color = \"{color}\"\n"));
    }

    for det in &d.details {
        s.push_str("\n[[details]]\n");
        s.push_str(&format!("name   = {}\n", quote(&det.name)));
        s.push_str(&format!(
            "sizes  = [{}]\n",
            if det.on_bar {
                "\"bar\", \"dock\""
            } else {
                "\"dock\""
            }
        ));
        s.push_str("points = [\n");
        for p in &det.points {
            s.push_str(&format!("  [{}, {}],\n", num(p[0]), num(p[1])));
        }
        s.push_str("]\n");
    }

    s
}

/// f64 を TOML の float リテラルにする。**必ず小数点を含む**
/// （`22` と書くと TOML の整数になるが、`RawWh` は f64 を要求するので
/// serde は受け付ける。とはいえ型がぶれないほうが読み手に親切）。
fn num(v: f64) -> String {
    if !v.is_finite() {
        return "0.0".to_string();
    }
    // -0.0 を "0.0" に潰す（"−0.0" は正しい TOML だが読み手を戸惑わせる）。
    let v = if v == 0.0 { 0.0 } else { v };
    let mut s = format!("{v:.4}");
    while s.ends_with('0') && !s.ends_with(".0") {
        s.pop();
    }
    s
}

/// TOML の基本文字列。制御文字とバックスラッシュ・引用符を逃がす。
fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04X}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// `d` 属性の 1 行を組む（`shape::smooth_path` の結果を書き出す用）。
pub fn path_d(start: (f64, f64), segs: &[crate::face::Seg]) -> String {
    let mut d = format!("M {} {}\n", num(start.0), num(start.1));
    for seg in segs {
        match *seg {
            crate::face::Seg::Line { to } => {
                d.push_str(&format!("L {} {}\n", num(to.0), num(to.1)));
            }
            crate::face::Seg::Cubic { c1, c2, to } => {
                d.push_str(&format!(
                    "C {} {} {} {} {} {}\n",
                    num(c1.0),
                    num(c1.1),
                    num(c2.0),
                    num(c2.1),
                    num(to.0),
                    num(to.1)
                ));
            }
        }
    }
    d
}

/// 生成した TOML から、埋め込んであるビルダー設定（1 行 JSON）を取り出す。
pub fn extract_config(toml_text: &str) -> Option<&str> {
    toml_text
        .lines()
        .find_map(|l| l.strip_prefix(CONFIG_MARKER))
        .map(str::trim)
}

/// 任意の `Serialize` を 1 行 JSON にする（コメント埋め込み用）。
pub fn inline_json<T: Serialize>(v: &T) -> Option<String> {
    serde_json::to_string(v).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_always_carry_a_decimal_point() {
        assert_eq!(num(22.0), "22.0");
        assert_eq!(num(0.5), "0.5");
        assert_eq!(num(0.1234), "0.1234");
        assert_eq!(num(-0.0), "0.0");
        assert_eq!(num(f64::NAN), "0.0");
    }

    #[test]
    fn strings_are_escaped() {
        assert_eq!(quote("a\"b\\c"), "\"a\\\"b\\\\c\"");
        assert_eq!(quote("わたし"), "\"わたし\"");
    }

    /// 埋め込んだ設定を読み戻せる（保存した TOML から編集を再開する経路）。
    #[test]
    fn the_embedded_config_round_trips() {
        let d = Draft {
            id: "x".into(),
            label: "エックス".into(),
            author: None,
            bar: (22.0, 20.0),
            dock: (37.0, 34.0),
            outline: OutlineDraft::Capsule,
            eye_polygon: None,
            eye_v: 0.5,
            eye_gap: (3.0, 4.7),
            eye_size: ([3.0, 3.4], [4.7, 5.3]),
            eye_radius: 1.5,
            eye_color: None,
            details: Vec::new(),
            notes: vec!["顔=capsule".into()],
            config_json: Some(r#"{"version":1,"id":"x"}"#.into()),
        };
        let text = to_toml(&d);
        assert_eq!(extract_config(&text), Some(r#"{"version":1,"id":"x"}"#));
        assert!(extract_config("id = \"a\"\n").is_none());
    }
}
