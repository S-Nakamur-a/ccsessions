//! 顔の検証。問題は `ProblemCode`（`face::spec`）の分類で返す。
//!
//! 顔がデータになった以上、形の妥当性は**コンパイル時には検査できない**。
//! 組込み顔はテストから（`every_builtin_face_passes_validation`）、ユーザ顔は
//! 読み込み時にここを通す。
//!
//! # 全部集めて返す
//! 最初の 1 件で止めない。投稿者が 1 回の `ccsessions face check` で全部直せるように。
//!
//! # ここに入らないもの
//! **`lay_out` を使うレイアウト検査は `ccsessionsd/src/layout.rs` のテスト**。
//! `lay_out` / `BAR_HEADROOM` / `glyph_offset` は `ccsessionsd` のデザイン定数で、
//! `ccsessions-core` からは参照できない（依存の向きが逆）。ここには「データだけで
//! 見える制約」＝ `BodySize`（`bar.h <= 22`）だけを置く。

use crate::face::spec::{is_valid_id, EyeShape, FaceSpec, Problem, ProblemCode};
use crate::face::{contains, flatten, Size};
use crate::session::SessionState;

/// bar の体の高さの上限（pt）。
///
/// **なぜ 22 か**: メニューバーは非ノッチ画面（外部モニタ・Air・旧機種）で 24pt しかない。
/// `bar_fit` はここに収めるためにグリフを体へ重ねる（段階 2）が、体そのものより
/// 2pt 以上低いと**体を縮める**段階 3 に落ちて生き物が小さくなる。22 は
/// 「24pt でも体を縮めずに済む」上限（24 − 上下の余裕 2）。
pub const MAX_BAR_BODY_H: f64 = 22.0;

/// 輪郭を折れ線に潰すときの分割数。目とパネル線の内外判定の精度。
const FLATTEN_STEPS: usize = 24;

/// 座標の許容誤差（pt）。ベジェの丸め誤差を吸収する。
const EPS: f64 = 0.001;

/// 顔を検証する。問題があれば**全部**返す。
pub fn validate(spec: &FaceSpec) -> Result<(), Vec<Problem>> {
    let mut p = Vec::new();

    check_id(spec, &mut p);
    check_bar_height(spec, &mut p);
    check_outline(spec, &mut p);
    check_symmetry(spec, &mut p);
    check_eyes(spec, &mut p);
    check_details(spec, &mut p);

    if p.is_empty() {
        Ok(())
    } else {
        Err(p)
    }
}

// ---------------------------------------------------------------------------
// id
// ---------------------------------------------------------------------------

fn check_id(spec: &FaceSpec, p: &mut Vec<Problem>) {
    if !is_valid_id(&spec.id) {
        p.push(Problem::new(
            ProblemCode::Id,
            format!(
                "id {:?} は使えません。英小文字・数字・ハイフンだけで、\
                 先頭は英数字、32 文字以内にしてください（例: \"my-face\"）",
                spec.id
            ),
        ));
    }
}

// ---------------------------------------------------------------------------
// 体の寸法（bar の高さ）
// ---------------------------------------------------------------------------

fn check_bar_height(spec: &FaceSpec, p: &mut Vec<Problem>) {
    let h = spec.size.bar.h;
    if h > MAX_BAR_BODY_H {
        p.push(Problem::new(
            ProblemCode::BodySize,
            format!(
                "bar の体の高さが {h}pt です。{MAX_BAR_BODY_H}pt 以下にしてください。\
                 メニューバーは非ノッチ画面（外部モニタ・Air・旧機種）で 24pt しかなく、\
                 上下の余裕 2pt を引くと {MAX_BAR_BODY_H}pt が上限になります。\
                 これを超えると体そのものが縮められて生き物が小さくなります"
            ),
        ));
    }
    for (size, s) in [("bar", spec.size.bar), ("dock", spec.size.dock)] {
        if !(s.w.is_finite() && s.h.is_finite()) || s.w <= 0.0 || s.h <= 0.0 {
            p.push(Problem::new(
                ProblemCode::BodySize,
                format!("{size} の体の寸法 {}x{} が不正です（正の有限値）", s.w, s.h),
            ));
        }
    }
}

// ---------------------------------------------------------------------------
// 輪郭が閉じている & 矩形に収まる
// ---------------------------------------------------------------------------

fn check_outline(spec: &FaceSpec, p: &mut Vec<Problem>) {
    for size in [Size::Bar, Size::Dock] {
        let (w, h) = spec.body_size(size);
        let o = spec.body_outline(size);

        let Some(last) = o.segs.last().map(|s| crate::face::seg_to(*s)) else {
            p.push(Problem::new(
                ProblemCode::Outline,
                format!("{} の輪郭に手が 1 つもありません", tag(size)),
            ));
            continue;
        };
        if (last.0 - o.start.0).abs() > EPS || (last.1 - o.start.1).abs() > EPS {
            p.push(Problem::new(
                ProblemCode::Outline,
                format!(
                    "{} の輪郭が閉じていません（始点 {:?} → 終点 {last:?}）。\
                     最後の手で始点へ戻してください（half = true なら鏡像が自動で戻します）",
                    tag(size),
                    o.start
                ),
            ));
        }

        for (x, y) in crate::face::outline_points(&o) {
            if !(-EPS..=w + EPS).contains(&x) || !(-EPS..=h + EPS).contains(&y) {
                p.push(Problem::new(
                    ProblemCode::Outline,
                    format!(
                        "{} で輪郭が体の矩形 {w}x{h} からはみ出します（点 ({x:.3}, {y:.3})）。\
                         座標は 0..1 の比率で書いてください",
                        tag(size)
                    ),
                ));
                break; // 1 サイズにつき 1 件でよい（同じ原因が何十点も出る）
            }
        }
    }
}

// ---------------------------------------------------------------------------
// half = true の輪郭が左右対称
// ---------------------------------------------------------------------------

fn check_symmetry(spec: &FaceSpec, p: &mut Vec<Problem>) {
    let half = matches!(
        spec.outline,
        crate::face::OutlineSpec::Path { half: true, .. }
    );
    if !half {
        return;
    }
    let (w, _) = spec.body_size(Size::Dock);
    let pts = crate::face::outline_points(&spec.body_outline(Size::Dock));

    // 点列は 始点 + 右半分 + 左半分 の奇数個で、真ん中が折り返し。
    if pts.len() % 2 != 1 {
        p.push(Problem::new(
            ProblemCode::Symmetry,
            format!(
                "half = true の輪郭の点数が偶数（{}）で折り返し点が定まりません",
                pts.len()
            ),
        ));
        return;
    }
    let mid = pts.len() / 2;
    if (pts[mid].0 - w / 2.0).abs() > EPS {
        p.push(Problem::new(
            ProblemCode::Symmetry,
            format!(
                "half = true の輪郭の折り返し点が中央にありません（x = {:.3}、中央は {:.3}）。\
                 d の最後の点は x = 0.5 にしてください",
                pts[mid].0,
                w / 2.0
            ),
        ));
    }
    for k in 1..=mid {
        let (rx, ry) = pts[mid - k];
        let (lx, ly) = pts[mid + k];
        if (rx - (w - lx)).abs() > EPS || (ry - ly).abs() > EPS {
            p.push(Problem::new(
                ProblemCode::Symmetry,
                format!("half = true の輪郭が左右非対称です（{rx:.3} と {lx:.3}）"),
            ));
            break;
        }
    }
}

// ---------------------------------------------------------------------------
// 目
// ---------------------------------------------------------------------------

fn check_eyes(spec: &FaceSpec, p: &mut Vec<Problem>) {
    for size in [Size::Bar, Size::Dock] {
        let (bw, bh) = spec.body_size(size);
        let poly = flatten(&spec.body_outline(size), FLATTEN_STEPS);
        let gap = spec.eye_gap(size);

        for state in SessionState::ORDER {
            let e = spec.eye(state, size);

            // --- 両目が重ならない ---
            if e.w * 2.0 + gap > bw + EPS {
                p.push(Problem::new(
                    ProblemCode::EyesTooWide,
                    format!(
                        "{}/{} で両目が体からはみ出します（目 {:.2} × 2 + 間隔 {gap} = {:.2} > 体の幅 {bw}）。\
                         eyes.size を小さくするか eyes.gap を詰めてください",
                        tag(size),
                        state.as_str(),
                        e.w,
                        e.w * 2.0 + gap
                    ),
                ));
            }

            // --- 目が輪郭の内側 ---
            // 配置は `creature.rs::apply` と同じ式にすること（ここがズレると検証が無意味）。
            let left = (bw - (e.w * 2.0 + gap)) / 2.0 + e.dx;
            let bottom = (bh - e.h) / 2.0 + e.dy;
            for i in 0..2 {
                let pts: Vec<(f64, f64)> = match spec.eye_shape(e.w, e.h, i == 0) {
                    Some(shape) => shape,
                    // 角丸矩形は 4 隅が内側なら十分。
                    None => vec![(0.0, 0.0), (e.w, 0.0), (e.w, e.h), (0.0, e.h)],
                };
                for (px, py) in pts {
                    let pt = (left + (e.w + gap) * i as f64 + px, bottom + py);
                    if !contains(&poly, pt) {
                        p.push(Problem::new(
                            ProblemCode::EyesOutsideBody,
                            format!(
                                "{}/{} で目が顔からはみ出します（点 ({:.2}, {:.2})）。\
                                 eyes.size を小さくするか eyes.v で位置を変えるか、輪郭を広げてください",
                                tag(size),
                                state.as_str(),
                                pt.0,
                                pt.1
                            ),
                        ));
                        break;
                    }
                }
            }
        }

        // --- 状態ごとに見え方が変わる（bar でだけ見る） ---
        if size.is_bar() {
            check_state_readability(spec, size, p);
        }
    }
}

/// 状態が目で読み分けられること。
///
/// 色・アニメは顔ごとに変えられないので、**顔が状態を表現する余地は目だけ**。
/// ここが効いていないと「見れば分かる」が壊れる。
fn check_state_readability(spec: &FaceSpec, size: Size, p: &mut Vec<Problem>) {
    let done = spec.eye(SessionState::Done, size);
    let differs = |a: &crate::face::EyeSpec| {
        (a.w - done.w).abs() > EPS
            || (a.h - done.h).abs() > EPS
            || a.color != done.color
            || (a.dx - done.dx).abs() > EPS
            || a.glow != done.glow
            || a.blink != done.blink
    };
    for state in [
        SessionState::Working,
        SessionState::WaitUser,
        SessionState::WaitAgent,
        SessionState::Idle,
        SessionState::Error,
    ] {
        if !differs(&spec.eye(state, size)) {
            p.push(Problem::new(
                ProblemCode::StatesLookAlike,
                format!(
                    "{} の目が done と見分けられません。\
                     [eyes.states.{}] で寸法・色・横目・瞬きのどれかを変えてください\
                     （色とアニメは顔ごとに変えられないので、状態は目で読ませます）",
                    state.as_str(),
                    state.as_str()
                ),
            ));
        }
    }
}

// ---------------------------------------------------------------------------
// パネル線
// ---------------------------------------------------------------------------

fn check_details(spec: &FaceSpec, p: &mut Vec<Problem>) {
    if spec.details.is_empty() {
        return;
    }
    for size in [Size::Bar, Size::Dock] {
        let (w, h) = spec.body_size(size);
        let poly = flatten(&spec.body_outline(size), FLATTEN_STEPS);
        // 名前つきで報告したいので `face_details` ではなく生の定義を辿る。
        for d in spec.details.iter().filter(|d| d.sizes.contains(&size)) {
            for &[u, v] in &d.points {
                let pt = (u * w, v * h);
                if !contains(&poly, pt) {
                    p.push(Problem::new(
                        ProblemCode::DetailOutsideBody,
                        format!(
                            "{} のパネル線 {:?} が顔からはみ出します（点 ({:.2}, {:.2})）。\
                             輪郭の内側へ寄せてください（縁ぎりぎりだと二重線に見えます）",
                            tag(size),
                            d.name,
                            pt.0,
                            pt.1
                        ),
                    ));
                    break;
                }
            }
        }
    }

    // --- bar は線を間引く ---
    let bar = spec.face_details(1.0, 1.0, Size::Bar).len();
    let dock = spec.face_details(1.0, 1.0, Size::Dock).len();
    if bar > dock {
        p.push(Problem::new(
            ProblemCode::BarDetailsNotThinned,
            format!(
                "bar のパネル線が {bar} 本で dock の {dock} 本より多くなっています。\
                 bar は狭い（18x20pt に 7 本引くと潰れて塊になります）ので、\
                 [[details]] の sizes で間引いてください"
            ),
        ));
    }
}

// ---------------------------------------------------------------------------
// bar で 6 匹並べた幅（**警告のみ**）
// ---------------------------------------------------------------------------

/// ノッチ右に見込める空きから、群れがそこへ収まるかの閾値（pt）。
///
/// `ccsessionsd/src/geometry.rs` の `MENU_EXTRA_RESERVE(225) - GUTTER(8)`。
/// **これは特定機種の実測に基づく見積もりで、実行時に測った値ではない。**
const NOTCH_RIGHT_BUDGET: f64 = 217.0;

/// 群れの幅の計算に使う bar の定数（`ccsessionsd/src/layout.rs` と一致させる）。
/// 左右の余白 9pt ずつ、生き物どうしの間隔 9pt。
const BAR_SIDE_MARGIN: f64 = 9.0;
const BAR_FLOCK_GAP: f64 = 9.0;

/// 「bar で 6 匹並べるとノッチ右に入らない」ことの**警告**を返す。
///
/// **検証エラーにはしない**。理由:
/// `bar_align = "auto"` は「右に入らなければ左へフォールバック」を既に実装しており
/// （`geometry.rs`）、**組込みの bean が幅 231pt で実際にその経路を通って正常に
/// 動いている**。超える顔を拒否すると、システムが問題なく扱えるデザインを弾くことになる。
///
/// それでも投稿者には知らせる価値がある（「なぜ自分の顔だけ左に出るのか」が分かる）。
pub fn notch_width_warning(spec: &FaceSpec) -> Option<Problem> {
    let w = flock_width_for_six(spec);
    (w > NOTCH_RIGHT_BUDGET).then(|| {
        Problem::new(
            ProblemCode::NotchWidth,
            format!(
                "bar で 6 匹並べると幅 {w:.1}pt で、ノッチ右の見込み空き \
                 {NOTCH_RIGHT_BUDGET:.1}pt を超えます。bar_align = \"auto\" は\
                 ノッチの左へ逃げます（正常動作。前面アプリのメニューと重なることがあります）"
            ),
        )
    })
}

/// bar で 6 匹並べたときの窓幅（pt）。
fn flock_width_for_six(spec: &FaceSpec) -> f64 {
    let (bw, _) = spec.body_size(Size::Bar);
    BAR_SIDE_MARGIN * 2.0 + bw * 6.0 + BAR_FLOCK_GAP * 5.0
}

fn tag(size: Size) -> &'static str {
    if size.is_bar() {
        "bar"
    } else {
        "dock"
    }
}

/// 目の描き方が `polygon` なのに多角形が無い、等の取り違えを検証側でも拾う。
/// （パーサでも見ているが、`FaceSpec` を手で組んだ場合の保険）
pub fn shape_matches_polygon(spec: &FaceSpec) -> bool {
    match spec.eyes.shape {
        EyeShape::Polygon => spec.eyes.polygon.is_some(),
        EyeShape::Rounded => spec.eyes.polygon.is_none(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::face::parse::parse;
    use crate::face::spec::Source;
    use crate::face::Registry;

    /// **組込み顔がすべて検証を通る。**
    ///
    /// これが「誰でも追加しやすい」の実体 — 投稿者は TOML を 1 つ置くだけで、
    /// このテストがはみ出しや高さ超過を自動的に指摘してくれる。
    #[test]
    fn every_builtin_face_passes_validation() {
        let reg = Registry::builtin();
        assert!(reg.problems().is_empty(), "{:?}", reg.problems());
        for face in reg.all() {
            if let Err(ps) = validate(face) {
                panic!("組込み顔 {} が検証に落ちた: {ps:#?}", face.id);
            }
        }
        assert_eq!(reg.all().len(), 4, "組込み顔が 4 つない");
    }

    /// 組込み顔は `shape` と `polygon` の対応も取れている。
    #[test]
    fn every_builtin_face_has_a_consistent_eye_shape() {
        for face in Registry::builtin().all() {
            assert!(shape_matches_polygon(face), "{} の目の形が不整合", face.id);
        }
    }

    // ---- 落ちる顔（検証が効いている証拠）---------------------------------

    fn face(body: &str) -> crate::face::FaceSpec {
        parse(body, Source::Builtin).unwrap_or_else(|e| panic!("パースできない: {e:?}"))
    }

    fn codes(spec: &crate::face::FaceSpec) -> Vec<ProblemCode> {
        match validate(spec) {
            Ok(()) => Vec::new(),
            Err(ps) => ps.iter().map(|p| p.code).collect(),
        }
    }

    /// 素直な角丸の顔。各テストがここから 1 か所だけ壊す。
    fn ok_toml(bar_h: f64, eye_w: f64, gap: f64) -> String {
        format!(
            r#"
id = "probe"
label = "検査用"
[size]
bar  = {{ w = 22, h = {bar_h} }}
dock = {{ w = 36, h = 34 }}
[outline]
kind = "corners"
corners = [[0.5,0.5],[0.5,0.5],[0.5,0.5],[0.5,0.5]]
[eyes]
shape = "rounded"
gap  = {{ bar = {gap}, dock = 5.0 }}
size = {{ bar = [{eye_w}, 4.0], dock = [4.0, 6.0] }}
"#
        )
    }

    #[test]
    fn the_probe_face_itself_passes() {
        // 「落ちる顔」テストが本当に 1 か所だけを突いている証拠。
        assert_eq!(
            codes(&face(&ok_toml(20.0, 3.0, 3.0))),
            Vec::<ProblemCode>::new()
        );
    }

    /// bar が高すぎる顔は落ちる。
    #[test]
    fn c5_rejects_a_too_tall_bar_body() {
        let c = codes(&face(&ok_toml(26.0, 3.0, 3.0)));
        assert!(c.contains(&ProblemCode::BodySize), "{c:?}");
        // エラーメッセージに理由（24pt）が入っていること。
        let e = validate(&face(&ok_toml(26.0, 3.0, 3.0))).unwrap_err();
        assert!(
            e.iter().any(|p| p.message.contains("24pt")),
            "なぜ駄目かが書かれていない: {e:?}"
        );
    }

    /// 目が広すぎて両目が体に収まらない顔は落ちる。
    #[test]
    fn c7_rejects_overlapping_eyes() {
        let c = codes(&face(&ok_toml(20.0, 11.0, 3.0)));
        assert!(c.contains(&ProblemCode::EyesTooWide), "{c:?}");
    }

    /// 目が輪郭からはみ出す顔は落ちる。
    ///
    /// 真円（corners 0.5）の体に、体幅いっぱいの目を置くと角が輪郭の外に出る。
    #[test]
    fn c3_rejects_eyes_poking_outside_the_outline() {
        let c = codes(&face(&ok_toml(20.0, 9.0, 3.0)));
        assert!(c.contains(&ProblemCode::EyesOutsideBody), "{c:?}");
    }

    /// 全状態で目が同じ顔は落ちる。
    #[test]
    fn c6_rejects_a_face_whose_states_all_look_alike() {
        // 6 状態すべてを「既定と同じ見た目」に上書きしてしまう。
        let mut t = ok_toml(20.0, 3.0, 3.0);
        for s in [
            "working",
            "wait_user",
            "wait_agent",
            "idle",
            "done",
            "error",
        ] {
            t.push_str(&format!("[eyes.states.{s}]\nw_scale = 1.0\n"));
        }
        let c = codes(&face(&t));
        assert!(c.contains(&ProblemCode::StatesLookAlike), "{c:?}");
    }

    /// パネル線が輪郭からはみ出す顔は落ちる。
    #[test]
    fn c4_rejects_details_outside_the_outline() {
        let t = format!(
            "{}\n[[details]]\nname = \"out\"\npoints = [[0.0,0.0],[1.0,1.0]]\n",
            ok_toml(20.0, 3.0, 3.0)
        );
        let c = codes(&face(&t));
        assert!(c.contains(&ProblemCode::DetailOutsideBody), "{c:?}");
    }

    /// bar の線が dock より多い顔は落ちる。
    #[test]
    fn c9_rejects_a_face_that_does_not_thin_out_bar_lines() {
        let t = format!(
            "{}\n\
             [[details]]\nname = \"a\"\nsizes = [\"bar\"]\npoints = [[0.45,0.45],[0.55,0.55]]\n\
             [[details]]\nname = \"b\"\nsizes = [\"bar\"]\npoints = [[0.45,0.5],[0.55,0.5]]\n",
            ok_toml(20.0, 3.0, 3.0)
        );
        let c = codes(&face(&t));
        assert!(c.contains(&ProblemCode::BarDetailsNotThinned), "{c:?}");
    }

    /// half = true なのに左右非対称になる d は落ちる。
    ///
    /// 折り返し点（最後の点）が中央でない場合。
    #[test]
    fn c2_rejects_a_half_path_that_does_not_end_at_the_centre() {
        let t = r#"
id = "lop"
label = "非対称"
[size]
bar  = { w = 22, h = 20 }
dock = { w = 36, h = 34 }
[outline]
kind = "path"
half = true
d = """
M 0.500 0.000
L 0.900 0.000
L 0.700 1.000
"""
[eyes]
shape = "rounded"
gap  = { bar = 3.0, dock = 5.0 }
size = { bar = [3.0, 4.0], dock = [4.0, 6.0] }
"#;
        let c = codes(&face(t));
        assert!(c.contains(&ProblemCode::Symmetry), "{c:?}");
    }

    /// 閉じていない輪郭（half = false でパスが戻ってこない）は落ちる。
    #[test]
    fn c1_rejects_an_unclosed_outline() {
        let t = r#"
id = "open"
label = "開いた輪郭"
[size]
bar  = { w = 22, h = 20 }
dock = { w = 36, h = 34 }
[outline]
kind = "path"
half = false
d = """
M 0.100 0.100
L 0.900 0.100
L 0.900 0.900
"""
[eyes]
shape = "rounded"
gap  = { bar = 3.0, dock = 5.0 }
size = { bar = [3.0, 4.0], dock = [4.0, 6.0] }
"#;
        let c = codes(&face(t));
        assert!(c.contains(&ProblemCode::Outline), "{c:?}");
    }

    /// 輪郭が体の矩形からはみ出す（比率が 1 を超える）顔は落ちる。
    #[test]
    fn c1_rejects_an_outline_outside_the_body_rect() {
        let t = r#"
id = "big"
label = "はみ出し"
[size]
bar  = { w = 22, h = 20 }
dock = { w = 36, h = 34 }
[outline]
kind = "path"
half = false
d = """
M 0.000 0.000
L 1.500 0.000
L 1.500 1.000
L 0.000 1.000
L 0.000 0.000
"""
[eyes]
shape = "rounded"
gap  = { bar = 3.0, dock = 5.0 }
size = { bar = [3.0, 4.0], dock = [4.0, 6.0] }
"#;
        let c = codes(&face(t));
        assert!(c.contains(&ProblemCode::Outline), "{c:?}");
    }

    /// id が不正な顔は落ちる（パーサでも弾くが、検証側でも見る）。
    #[test]
    fn c8_rejects_a_bad_id() {
        let mut f = face(&ok_toml(20.0, 3.0, 3.0));
        f.id = "Bad Id".into();
        assert!(codes(&f).contains(&ProblemCode::Id));
    }
}
