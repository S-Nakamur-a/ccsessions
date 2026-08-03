//! `FaceSpec` → SVG の純関数レンダラ。
//!
//! **画面収録権限が無い環境ではスクリーンショットが撮れない。**
//! `ccsessionsd` が画面ジオメトリとホバー対象を stderr へ出す設計なのはそのためで、
//! 顔にも同じ流儀が要る。
//!
//! CALayer と SVG は**同じ `FaceSpec` の同じ解決関数**（`outline_of` / `eye` /
//! `eye_shape` / `face_details`）を通るので、**SVG は CALayer の忠実なプレビュー**に
//! なる。違うのはグローとアニメだけで、SVG ではそこを静的に近似する。
//!
//! 効果:
//! 1. 投稿者が PR に SVG を貼れる（レビューが目視できる）
//! 2. `ccsessions face gallery` の出力がそのまま顔の一覧ドキュメントになる
//! 3. **Mac を持っていなくても顔を作れる**
//!
//! # 座標系
//! `FaceSpec` は左下原点・y 上向き（CALayer と同じ）。SVG は**左上原点・y 下向き**
//! なので、書き出すときに `y → H - y` で反転する。

use std::fmt::Write as _;

use crate::face::palette::{self, Rgb};
use crate::face::style;
use crate::face::{FaceSpec, Seg, Size};
use crate::session::SessionState;

/// 体のまわりに取る余白（pt）。グローの滲みと枠線がクリップされないように。
const PAD: f64 = 8.0;

/// プレビュー用チップの余白（pt）。
///
/// `PAD` より遥かに狭い。`PAD` はグローの滲みとグリフのためのもので、チップは
/// **どちらも描かない**。18pt まで縮めて並べる絵なので、余白に使う 1pt は形の
/// 判別に使う 1pt より価値が低い — `PAD` のままだと 18pt のうち体は 11pt しか
/// 残らず、egg と round と squircle が同じ絵に潰れる（実測）。
///
/// **詰めても切れない根拠**: 輪郭・目・パネル線は設計上どれも体の矩形の内側に
/// 収まる（`face::validate` の輪郭・目・パネル線の検査が言語化している条件で、組込みの
/// 顔 × 両サイズ × 全状態では実測はみ出し 0.000pt）。体の外に出るのは
/// **グリフだけ**なので、グリフを描かないチップに限って余白を削れる。
///
/// ただし **`validate` は `ccsessions face check` でしか走らず、`Registry::load_in`
/// は通さない**ので、これは「保証」ではなく「まともな顔なら成り立つ前提」。
/// それらを破る顔はこのチップより先に**群れの描画そのもの**が壊れるため、
/// ここで余白を稼いでも救いにはならない（＝ `PAD` を維持する理由にならない）。
/// `Frame::glyph` のコメントも参照。
const CHIP_PAD: f64 = 3.0;

/// 背景チップの角丸半径を高さに対する比で決める。
///
/// 固定 pt にすると、顔ごとに体の寸法が違うぶん丸みの見え方がばらつく。
/// プレビューは 1 枚のメニューに縦に並ぶので、**顔をまたいで同じ丸み**に
/// 見えることが優先。
const CHIP_RADIUS_RATIO: f64 = 0.16;

/// 背景チップの角丸半径の上限（pt）。
///
/// 背景は体を**クリップしない**（ただ下に敷くだけ）ので、角を丸めすぎると
/// 体の矩形の角 `(CHIP_PAD, CHIP_PAD)` が背景の外へ出て、そこだけメニューの
/// 地色に直に乗る — 「コントラストを環境依存にしない」という チップの根拠が
/// 角だけ崩れる。角が内側に留まる条件は `r <= CHIP_PAD * (2 + √2) ≈ 10.24` なので、
/// そこに余裕を見て止める。比で決めた半径がこれを超えるのは体高 58pt 超の顔
/// （組込みの最大は 34pt）だけなので、現実の顔の見た目は変わらない。
const CHIP_RADIUS_MAX: f64 = 10.0;

/// `render` と `render_chip` の差分だけを持つ。
///
/// 分けたいのは余白・背景の角丸・グリフの 3 点だけで、体・目・線画の描画は
/// **同じコード**を通す（プレビューがギャラリーと違う顔に見えたら、プレビューの
/// 意味が無い）。
struct Frame {
    /// 体のまわりに取る余白（pt）。
    pad: f64,
    /// 背景矩形の角丸半径（pt）。`0.0` なら `rx` 属性自体を書かない。
    corner: f64,
    /// 状態のグリフ（`›` `!` …）を描くか。**`false` のときだけ余白を詰めてよい**
    /// — グリフは体の外（`x = bw + 2`）に出るので、詰めると切れる。
    glyph: bool,
}

/// 顔 1 つを 1 枚の SVG にする。
///
/// 色は**顔が持っている**（`[colors.<状態>]`）ので、ここに色の引数は無い。
/// 同じ顔ファイルからは誰の手元でも同じ絵が出る。
pub fn render(face: &FaceSpec, state: SessionState, size: Size) -> String {
    render_with(
        face,
        state,
        size,
        Frame {
            pad: PAD,
            corner: 0.0,
            glyph: true,
        },
    )
}

/// 顔選択 UI 用のプレビュー。角丸の背景チップに載せ、グリフは描かない。
///
/// # なぜ背景を残すのか
/// パレットは面の色を `color-mix(in oklch, accent 22%, #0b0912)` として
/// **`INK` の上に載る前提**で解いてある（`palette::face_fill`）。背景を透明に
/// すると、その前提がメニューの地色（ライト／ダークで変わるうえ vibrancy で
/// 透ける）に置き換わり、コントラストが環境依存になる。SVG から起こした
/// `NSImage` は外観変化に追従しないので、**チップごと描いて決め打ちにする**方が
/// どちらの外観でも同じに見える。
///
/// # なぜグリフを描かないのか
/// ここで選ぶのは**形**であって状態ではない。18pt まで縮めた絵に状態記号が
/// 乗ると、判別の役に立たないうえ「その状態が選ばれる」と誤読させる。
pub fn render_chip(face: &FaceSpec, state: SessionState, size: Size) -> String {
    let (_, bh) = face.body_size(size);
    render_with(
        face,
        state,
        size,
        Frame {
            pad: CHIP_PAD,
            corner: ((bh + CHIP_PAD * 2.0) * CHIP_RADIUS_RATIO).min(CHIP_RADIUS_MAX),
            glyph: false,
        },
    )
}

fn render_with(face: &FaceSpec, state: SessionState, size: Size, frame: Frame) -> String {
    let (bw, bh) = face.body_size(size);
    let pad = frame.pad;
    let (w, h) = (bw + pad * 2.0, bh + pad * 2.0);

    // **顔の解決関数を通す**（`palette` を直に引かない）。顔が `[colors.<状態>]` を
    // 持っていればその色、書いていなければ状態の既定色が返る。
    let accent = face.accent(state);
    let fill = face.fill(state);
    let opacity = palette::face_opacity(state);

    let mut s = String::new();
    let _ = write!(
        s,
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="0 0 {w} {h}" role="img" aria-label="{id} / {st}">
<title>{id} — {st} / {sz}</title>
<rect width="{w}" height="{h}"{rx} fill="{ink}"/>
<g opacity="{opacity}" transform="translate({pad},{pad})">
"#,
        id = esc(&face.id),
        st = state.as_str(),
        sz = if size.is_bar() { "bar" } else { "dock" },
        // 角丸なしのときは属性ごと省く（`render` の出力を従来と 1 バイトも変えない）。
        rx = if frame.corner > 0.0 {
            format!(r#" rx="{:.3}""#, frame.corner)
        } else {
            String::new()
        },
        ink = hex(palette::INK),
    );

    // --- 体 -----------------------------------------------------------------
    // CALayer と同じく、枠線幅の半分だけ内側に縮めた輪郭を描く
    // （ストロークがパス上に中心を置くので、そうしないと枠が体からはみ出す）。
    let inset = style::BORDER_W / 2.0;
    let o = face.outline_of(bw - inset * 2.0, bh - inset * 2.0, size);
    let _ = writeln!(
        s,
        r#"  <path d="{d}" transform="translate({inset},{inset})" fill="{fill}" stroke="{stroke}" stroke-width="{bwid}"/>"#,
        d = path_d(&o, bh - inset * 2.0),
        fill = hex(fill),
        stroke = hex(accent),
        bwid = style::BORDER_W,
    );

    // --- 顔のパネル線 --------------------------------------------------------
    for line in face.face_details(bw, bh, size) {
        let pts: Vec<String> = line
            .iter()
            .map(|&(x, y)| format!("{:.3},{:.3}", x, bh - y))
            .collect();
        let _ = writeln!(
            s,
            r#"  <polyline points="{}" fill="none" stroke="{}" stroke-opacity="{}" stroke-width="{}" stroke-linecap="round"/>"#,
            pts.join(" "),
            hex(accent),
            style::DETAIL_ALPHA,
            style::detail_line_w(size),
        );
    }

    // --- 目 -----------------------------------------------------------------
    let e = face.eye(state, size);
    let gap = face.eye_gap(size);
    // `creature.rs::apply` と同じ配置式。
    let left = (bw - (e.w * 2.0 + gap)) / 2.0 + e.dx;
    let bottom = (bh - e.h) / 2.0 + e.dy;
    for i in 0..2 {
        let x = left + (e.w + gap) * i as f64;
        match face.eye_shape(e.w, e.h, i == 0) {
            Some(poly) => {
                let pts: Vec<String> = poly
                    .iter()
                    .map(|&(px, py)| format!("{:.3},{:.3}", x + px, bh - (bottom + py)))
                    .collect();
                let _ = writeln!(
                    s,
                    "  <polygon points=\"{}\" fill=\"{}\"/>",
                    pts.join(" "),
                    hex(e.color)
                );
            }
            None => {
                let _ = writeln!(
                    s,
                    r#"  <rect x="{:.3}" y="{:.3}" width="{:.3}" height="{:.3}" rx="{:.3}" fill="{}"/>"#,
                    x,
                    bh - (bottom + e.h),
                    e.w,
                    e.h,
                    e.radius,
                    hex(e.color)
                );
            }
        }
    }

    // --- グリフ --------------------------------------------------------------
    // 状態の記号（`›` `!` `⋯` `z` `✓` `×`）。CALayer では体の右上に浮く。
    if frame.glyph {
        let _ = writeln!(
            s,
            r#"  <text x="{gx:.3}" y="{gy:.3}" font-family="ui-monospace,SFMono-Regular,Menlo,monospace" font-size="{fs}" font-weight="700" fill="{c}" text-anchor="middle">{g}</text>"#,
            gx = bw + 2.0,
            gy = -2.0,
            fs = if size.is_bar() { 10.0 } else { 11.0 },
            c = hex(accent),
            g = esc(state.glyph()),
        );
    }

    s.push_str("</g>\n</svg>\n");
    s
}

/// 輪郭を SVG の `d` 属性にする。`flip_h` は y 反転の基準（体の高さ）。
fn path_d(o: &crate::face::Outline, flip_h: f64) -> String {
    let y = |v: f64| flip_h - v;
    let mut d = format!("M {:.3} {:.3}", o.start.0, y(o.start.1));
    for seg in &o.segs {
        match *seg {
            Seg::Line { to } => {
                let _ = write!(d, " L {:.3} {:.3}", to.0, y(to.1));
            }
            Seg::Cubic { c1, c2, to } => {
                let _ = write!(
                    d,
                    " C {:.3} {:.3} {:.3} {:.3} {:.3} {:.3}",
                    c1.0,
                    y(c1.1),
                    c2.0,
                    y(c2.1),
                    to.0,
                    y(to.1)
                );
            }
        }
    }
    d.push_str(" Z");
    d
}

/// sRGB 成分（0..1）を `#rrggbb` にする。
///
/// 実体は `palette::to_hex`。設定に書く色と SVG に書く色で書式がずれないよう、
/// 変換は 1 か所に置いてある。
fn hex(c: Rgb) -> String {
    palette::to_hex(c)
}

/// XML の特殊文字を逃がす。顔の id / label はユーザ入力なので必ず通す。
fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// 全顔 × 全状態 × bar/dock を 1 枚の HTML にまとめる。
///
/// これを README に貼れば顔の一覧がそのままドキュメントになる。
pub fn gallery(faces: &[std::sync::Arc<FaceSpec>]) -> String {
    let mut s = String::from(
        r#"<!doctype html>
<meta charset="utf-8">
<title>ccsessions — 顔のギャラリー</title>
<style>
 body { background:#0b0912; color:#e6e9f0; font-family:system-ui,sans-serif; margin:24px; }
 h1 { font-size:18px; } h2 { font-size:15px; margin-top:28px; }
 .row { display:flex; flex-wrap:wrap; gap:14px; align-items:flex-end; }
 .cell { text-align:center; }
 .cell figcaption { font-size:10px; color:#8b93a7; margin-top:4px; }
 .meta { font-size:12px; color:#8b93a7; }
 svg { display:block; }
</style>
<h1>ccsessions — 顔のギャラリー</h1>
<p class="meta">グローとアニメは SVG では静的な近似。輪郭・目・線画は CALayer と同じデータ由来。</p>
"#,
    );

    for face in faces {
        let _ = write!(s, "<h2>{}", esc(&face.label));
        let _ = write!(s, " <span class=\"meta\">{}", esc(&face.id));
        if let Some(a) = &face.author {
            let _ = write!(s, " — {}", esc(a));
        }
        s.push_str("</span></h2>\n");

        for size in [Size::Bar, Size::Dock] {
            let _ = writeln!(
                s,
                "<p class=\"meta\">{}</p>\n<div class=\"row\">",
                if size.is_bar() { "bar" } else { "dock" }
            );
            for state in SessionState::ORDER {
                let _ = writeln!(
                    s,
                    "<figure class=\"cell\">{}<figcaption>{}</figcaption></figure>",
                    render(face, state, size),
                    state.as_str()
                );
            }
            s.push_str("</div>\n");
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::face::Registry;

    /// 全顔 × 全状態 × 両サイズが SVG になり、体裁が壊れていない。
    #[test]
    fn every_face_renders_to_well_formed_svg() {
        for face in Registry::builtin().all() {
            for size in [Size::Bar, Size::Dock] {
                for state in SessionState::ORDER {
                    let s = render(face, state, size);
                    assert!(s.starts_with("<svg "), "{} が svg で始まらない", face.id);
                    assert!(
                        s.trim_end().ends_with("</svg>"),
                        "{} が閉じていない",
                        face.id
                    );
                    // タグの開閉数が釣り合う（雑だが体裁崩れは拾える）。
                    assert_eq!(
                        s.matches('<').count(),
                        s.matches('>').count(),
                        "{}/{state:?} の山かっこが釣り合わない",
                        face.id
                    );
                    assert!(!s.contains("NaN"), "{}/{state:?} に NaN がある", face.id);
                }
            }
        }
    }

    /// 顔選択 UI 用のチップは、角丸の背景を持ちグリフを描かない。
    #[test]
    fn the_chip_preview_rounds_its_backdrop_and_drops_the_glyph() {
        for face in Registry::builtin().all() {
            let s = render_chip(face, SessionState::Working, Size::Dock);
            assert!(s.starts_with("<svg "), "{} が svg で始まらない", face.id);
            assert!(
                s.trim_end().ends_with("</svg>"),
                "{} が閉じていない",
                face.id
            );
            // **背景の矩形を名指しで見る**。単に `rx=` を探すと、目が丸角
            // `<rect ... rx="2.000"/>` で描かれる顔では目の方が条件を
            // 満たしてしまい、背景の角丸が消える回帰を見逃す。
            let (bw, bh) = face.body_size(Size::Dock);
            let backdrop = format!(
                r#"<rect width="{}" height="{}" rx=""#,
                bw + CHIP_PAD * 2.0,
                bh + CHIP_PAD * 2.0
            );
            assert!(
                s.contains(&backdrop),
                "{} の背景チップが角丸でない（{backdrop:?} が無い）:\n{s}",
                face.id
            );
            // グリフは `<text>` でしか出さないので、要素ごと消えていることで見る。
            assert!(
                !s.contains("<text"),
                "{} のプレビューに状態グリフが残っている",
                face.id
            );
            assert!(!s.contains("NaN"), "{} のチップに NaN がある", face.id);
        }
    }

    /// **チップとギャラリーは同じ体を描く**（プレビューが別物に見えない根拠）。
    ///
    /// 差分は背景の角丸とグリフだけなので、体の `<path>` 行は一致するはず。
    #[test]
    fn the_chip_preview_draws_the_same_body_as_the_gallery() {
        for face in Registry::builtin().all() {
            let body = |s: &str| {
                s.lines()
                    .find(|l| l.trim_start().starts_with("<path "))
                    .map(str::to_string)
                    .unwrap_or_else(|| panic!("{} に体の path が無い", face.id))
            };
            assert_eq!(
                body(&render(face, SessionState::Working, Size::Dock)),
                body(&render_chip(face, SessionState::Working, Size::Dock)),
                "{} の体がプレビューとギャラリーで食い違う",
                face.id
            );
        }
    }

    /// 角丸半径は、体の角が背景からはみ出さない上限で止まる。
    ///
    /// 背景は体をクリップしないので、丸めすぎると体の角だけメニューの地色に
    /// 直に乗る。組込みの顔では上限に当たらないこと（＝見た目が変わらないこと）と、
    /// 病的に高い顔では確かに止まることの両方を見る。
    #[test]
    fn the_chip_corner_never_outgrows_its_padding() {
        // 角が内側に留まる厳密な条件。
        let bound = CHIP_PAD * (2.0 + 2.0_f64.sqrt());
        assert!(
            CHIP_RADIUS_MAX <= bound,
            "上限 {CHIP_RADIUS_MAX} が幾何的な限界 {bound} を超えている"
        );
        // 組込みの顔は上限に当たらない（＝比のままで、見た目は不変）。
        for face in Registry::builtin().all() {
            let (_, bh) = face.body_size(Size::Dock);
            let natural = (bh + CHIP_PAD * 2.0) * CHIP_RADIUS_RATIO;
            assert!(
                natural < CHIP_RADIUS_MAX,
                "{} が上限に当たっている（{natural}）。上限は病的な顔のための安全網で、\
                 まともな顔の見た目を変えてはいけない",
                face.id
            );
        }
    }

    /// **後方互換の番人**: `render` の出力は角丸フレームの追加で変わらない。
    ///
    /// `ccsessions face render` / `gallery` が出す SVG は README に貼られる想定なので、
    /// プレビューを足したせいで既存の出力が動くのは避けたい。
    #[test]
    fn the_gallery_renderer_keeps_its_square_backdrop() {
        let reg = Registry::builtin();
        let face = reg.get("egg").unwrap();
        let s = render(face, SessionState::Working, Size::Dock);
        let (bw, bh) = face.body_size(Size::Dock);
        assert!(
            s.contains(&format!(
                r#"<rect width="{}" height="{}" fill=""#,
                bw + PAD * 2.0,
                bh + PAD * 2.0
            )),
            "背景矩形に余計な属性が入っている:\n{s}"
        );
        assert!(s.contains("<text"), "ギャラリー側のグリフが消えている");
    }

    /// 顔が `[colors.<状態>]` に書いた色が、その状態の SVG に実際に出る。
    ///
    /// accent は**枠とパネル線の両方**に出るので、片方だけ差し替える取り違えを
    /// ここで捕まえる。
    #[test]
    fn the_colours_a_face_declares_reach_the_svg() {
        let face = coloured_face();
        let s = render(&face, SessionState::Working, Size::Dock);
        assert!(
            s.contains(r##"stroke="#7f3ac2""##),
            "枠が顔の色でない:\n{s}"
        );
        assert!(s.contains(r##"fill="#241038""##), "面が顔の色でない:\n{s}");
        assert!(s.contains(r##"fill="#00ff88""##), "目が顔の色でない:\n{s}");
        // パネル線も accent なので同じ色になる。
        assert!(
            s.matches(r##""#7f3ac2""##).count() >= 2,
            "パネル線が顔の accent を使っていない:\n{s}"
        );
    }

    /// **書かなかった状態は既定パレットのまま**。1 状態だけ塗った顔で、
    /// 他の 5 状態が動かないことを見る（既存の顔の見た目を守る番人）。
    #[test]
    fn a_state_without_colours_keeps_the_default_palette() {
        let face = coloured_face();
        for state in SessionState::ORDER {
            if state == SessionState::Working {
                continue;
            }
            let s = render(&face, state, Size::Dock);
            assert!(
                s.contains(&format!(r#"stroke="{}""#, hex(palette::accent(state)))),
                "{} の枠が既定色から動いている",
                state.as_str()
            );
            assert!(
                s.contains(&format!(r#"fill="{}""#, hex(palette::face_fill(state)))),
                "{} の面が既定色から動いている",
                state.as_str()
            );
        }
    }

    /// 色を書いていない組込みの顔は、状態のパレットどおりに描かれる。
    #[test]
    fn a_face_without_colours_renders_exactly_what_the_state_palette_says() {
        let reg = Registry::builtin();
        let face = reg.get("egg").unwrap();
        for state in SessionState::ORDER {
            let s = render(face, state, Size::Dock);
            assert!(s.contains(&format!(r#"fill="{}""#, hex(palette::face_fill(state)))));
            assert!(s.contains(&format!(r#"stroke="{}""#, hex(palette::accent(state)))));
        }
    }

    /// **SVG の座標が解決関数の値そのもの**であること（プレビューの忠実さの根拠）。
    ///
    /// 組込み顔はどれも角丸の輪郭・線画なしなので、**パネル線と自由なシルエットを
    /// 持つ顔をここで組み立てる**（この 2 つが「解決関数を通っているか」が
    /// いちばん怪しい経路なので、角丸の顔で代用しない）。
    #[test]
    fn svg_coordinates_come_from_the_same_resolution_as_calayer() {
        let face = &lined_face();
        let size = Size::Dock;
        let (_, bh) = face.body_size(size);
        let svg = render(face, SessionState::Done, size);

        // パネル線の 1 点目が `face_details` の値を y 反転しただけであること。
        let line = &face.face_details(30.0, bh, size)[0];
        let want = format!("{:.3},{:.3}", line[0].0, bh - line[0].1);
        assert!(svg.contains(&want), "パネル線の座標が一致しない: {want}");

        // 輪郭の始点も同じ。
        let inset = style::BORDER_W / 2.0;
        let o = face.outline_of(30.0 - inset * 2.0, bh - inset * 2.0, size);
        let want = format!("M {:.3} {:.3}", o.start.0, (bh - inset * 2.0) - o.start.1);
        assert!(svg.contains(&want), "輪郭の始点が一致しない: {want}");
    }

    /// 状態ごとに絵が変わる（同じ SVG が 6 枚出てこない）。
    #[test]
    fn each_state_renders_differently() {
        let reg = Registry::builtin();
        for face in reg.all() {
            let mut seen: Vec<String> = Vec::new();
            for state in SessionState::ORDER {
                let s = render(face, state, Size::Dock);
                assert!(!seen.contains(&s), "{} の状態が見分けられない", face.id);
                seen.push(s);
            }
        }
    }

    /// ギャラリーに全顔が出る。
    #[test]
    fn the_gallery_lists_every_face() {
        let reg = Registry::builtin();
        let html = gallery(reg.all());
        for face in reg.all() {
            assert!(html.contains(&face.label), "{} が載っていない", face.id);
        }
        assert!(html.starts_with("<!doctype html>"));
    }

    /// id / label の特殊文字が XML を壊さない（ユーザ顔は信頼できない入力）。
    #[test]
    fn special_characters_are_escaped() {
        assert_eq!(esc("a<b>&\"c\""), "a&lt;b&gt;&amp;&quot;c&quot;");
    }

    /// パネル線と自由なシルエットを持つ顔（組込みには無い組み合わせ）。
    /// `[colors.working]` を持つ顔。線画も持たせて、accent がパネル線にも
    /// 効くことを 1 つのフィクスチャで見られるようにする。
    fn coloured_face() -> crate::face::FaceSpec {
        crate::face::parse::parse(
            r##"
id = "coloured"
label = "色つき"
[size]
bar  = { w = 22, h = 20 }
dock = { w = 36, h = 34 }
[outline]
kind = "corners"
corners = [[0.5,0.5],[0.5,0.5],[0.5,0.5],[0.5,0.5]]
[eyes]
shape = "rounded"
gap  = { bar = 3.0, dock = 5.0 }
size = { bar = [3.0, 4.0], dock = [4.0, 6.0] }
[[details]]
name = "brow"
sizes = ["bar", "dock"]
points = [[0.19,0.725],[0.50,0.775],[0.81,0.725]]
[colors.working]
accent = "#7f3ac2"
fill   = "#241038"
eye    = "#00ff88"
"##,
            crate::face::Source::Builtin,
        )
        .expect("フィクスチャがパースできない")
    }

    fn lined_face() -> crate::face::FaceSpec {
        crate::face::parse::parse(
            r#"
id = "lined"
label = "線画つき"
[size]
bar  = { w = 18, h = 20 }
dock = { w = 30, h = 34 }
[outline]
kind = "path"
half = true
d = """
M 0.500 0.000
L 0.615 0.000
C 0.700 0.020 0.770 0.075 0.815 0.170
L 0.500 1.000
"""
[eyes]
shape = "rounded"
gap  = { bar = 3.0, dock = 5.0 }
size = { bar = [3.0, 4.0], dock = [4.0, 6.0] }
[[details]]
name = "brow"
sizes = ["bar", "dock"]
points = [[0.19,0.725],[0.50,0.775],[0.81,0.725]]
"#,
            crate::face::Source::Builtin,
        )
        .expect("フィクスチャがパースできない")
    }
}
