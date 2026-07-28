//! 顔（生き物のデザイン）の共通データ型。
//!
//! ここに置くのは「顔ごとに変わる形・色の定義」であり、CALayer の組み立てや
//! AppKit 呼び出しは `ccsessionsd` 側の責務。顔そのものは `faces/*.toml` が定義する
//! （`docs/adr/0014-faces-as-data.md`）。
//!
//! `ccsessions` CLI（`ccsessionsd` に依存しない）が顔の SVG プレビューを描くために、
//! この crate から直接これらの型・関数へ手が届く必要がある。`theme.rs` は
//! ここから re-export することで、既存の呼び出し側（`ccsessionsd` 内の各モジュール）
//! を無変更のまま保つ。

pub mod builder;
pub mod palette;
pub mod parse;
pub mod registry;
pub mod spec;
pub mod style;
pub mod svg;
pub mod validate;

mod golden;

pub use registry::{builtin_ids, Registry, DEFAULT_FACE_ID};

pub use spec::{
    state_index, BodySize, BySize, CornerSpec, DetailSpec, EyeColor, EyeOverride, EyeShape,
    EyeSpec, EyesSpec, FaceSpec, OutlineSpec, Problem, Source,
};

// ---------------------------------------------------------------------------
// 寸法
// ---------------------------------------------------------------------------

/// 配置ごとに寸法が二段階（bar は小さく、dock は大きく）。
/// 元デザインの `bar` 真偽値に対応する。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Size {
    /// メニューバー帯（小）
    Bar,
    /// 画面下ドック（大）
    Dock,
}

impl Size {
    pub fn is_bar(self) -> bool {
        matches!(self, Size::Bar)
    }
}

// ---------------------------------------------------------------------------
// 形（border-radius プロファイル）
// ---------------------------------------------------------------------------

/// 角丸半径 4 隅ぶん。CSS の `border-radius` と同じ順（左上・右上・右下・左下）で、
/// 各要素は (水平半径, 垂直半径) の pt。楕円弧なので水平と垂直で別の半径を持つ。
pub type Corners = [(f64, f64); 4];

/// 閉じた輪郭。`start` から `segs` を順に辿ると 1 周する。
///
/// 角丸長方形（直線＋楕円弧の 8 手）もヘルメット（多角形）も同じ型で表せるように
/// 手数を固定していない。`ffi::path_from_outline` がそのまま `CGPath` に流す。
#[derive(Debug, Clone, PartialEq)]
pub struct Outline {
    pub start: (f64, f64),
    pub segs: Vec<Seg>,
}

/// 輪郭を進める 1 手。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Seg {
    /// 直線で `to` まで。
    Line { to: (f64, f64) },
    /// 3 次ベジェで `to` まで。
    Cubic {
        c1: (f64, f64),
        c2: (f64, f64),
        to: (f64, f64),
    },
}

/// 手の終点（＝次の手の始点）。
pub fn seg_to(s: Seg) -> (f64, f64) {
    match s {
        Seg::Line { to } | Seg::Cubic { to, .. } => to,
    }
}

/// 角丸プロファイルから、閉じた輪郭のベジェ制御点列を作る。
///
/// 各コーナーの楕円弧を 1 本の 3 次ベジェで近似する（円弧近似の標準係数
/// kappa = 0.5523）。返すのは「始点 → 手の並び」で、呼び出し側は
/// `CGMutablePath` の `move_to_point` / `add_line_to_point` /
/// `add_curve_to_point` にそのまま流せる。座標は左下原点（CALayer の既定）。
///
/// 半径の和が辺の長さを超える場合は CSS と同じ比率縮小をかける（卵型は
/// 上辺で 50%+50%=100% ちょうどなので、丸め誤差で溢れないようここで守る）。
pub fn outline(w: f64, h: f64, c: Corners) -> Outline {
    const K: f64 = 0.552_284_749_8;

    // CSS の overlap 縮小: 各辺について (r1 + r2) / 辺長 の最大比で全体を割る。
    let f = {
        let mut f: f64 = 1.0;
        let pairs = [
            (c[3].0 + c[2].0, w), // 下辺: BL.x + BR.x
            (c[0].0 + c[1].0, w), // 上辺: TL.x + TR.x
            (c[0].1 + c[3].1, h), // 左辺: TL.y + BL.y
            (c[1].1 + c[2].1, h), // 右辺: TR.y + BR.y
        ];
        for (sum, len) in pairs {
            if sum > 0.0 {
                f = f.min(len / sum);
            }
        }
        f.min(1.0)
    };
    let s = |(x, y): (f64, f64)| (x * f, y * f);
    let (tl, tr, br, bl) = (s(c[0]), s(c[1]), s(c[2]), s(c[3]));

    // 左下原点。y 上向き。CSS の「左上」はここでは y=h 側。
    // 各辺は「直線で走ってからコーナーの楕円弧を曲がる」の繰り返し。
    Outline {
        start: (bl.0, 0.0),
        segs: vec![
            // 下辺 → 右下コーナー（右へ進み、上へ曲がる）
            Seg::Line {
                to: (w - br.0, 0.0),
            },
            Seg::Cubic {
                c1: (w - br.0 + br.0 * K, 0.0),
                c2: (w, br.1 - br.1 * K),
                to: (w, br.1),
            },
            // 右辺 → 右上コーナー
            Seg::Line { to: (w, h - tr.1) },
            Seg::Cubic {
                c1: (w, h - tr.1 + tr.1 * K),
                c2: (w - tr.0 + tr.0 * K, h),
                to: (w - tr.0, h),
            },
            // 上辺 → 左上コーナー
            Seg::Line { to: (tl.0, h) },
            Seg::Cubic {
                c1: (tl.0 - tl.0 * K, h),
                c2: (0.0, h - tl.1 + tl.1 * K),
                to: (0.0, h - tl.1),
            },
            // 左辺 → 左下コーナー（始点へ戻る）
            Seg::Line { to: (0.0, bl.1) },
            Seg::Cubic {
                c1: (0.0, bl.1 - bl.1 * K),
                c2: (bl.0 - bl.0 * K, 0.0),
                to: (bl.0, 0.0),
            },
        ],
    }
}

// ---------------------------------------------------------------------------
// 輪郭に対する幾何の問い合わせ
// ---------------------------------------------------------------------------

/// 輪郭の全点（制御点を含む）。矩形に収まっているかの検査に使う。
pub fn outline_points(o: &Outline) -> Vec<(f64, f64)> {
    let mut v = vec![o.start];
    for s in &o.segs {
        match *s {
            Seg::Line { to } => v.push(to),
            Seg::Cubic { c1, c2, to } => v.extend([c1, c2, to]),
        }
    }
    v
}

/// 輪郭を折れ線に潰す。目とパネル線の内外判定のための近似。
///
/// もとは `theme.rs` のテスト用ヘルパだったが、データ駆動化で
/// **「目やパネル線が顔からはみ出さない」がユーザ顔にも効く必要がある**ため
/// 本体コードへ昇格させた。SVG レンダラからも使う。
pub fn flatten(o: &Outline, steps: usize) -> Vec<(f64, f64)> {
    let mut poly = vec![o.start];
    for s in &o.segs {
        match *s {
            Seg::Line { to } => poly.push(to),
            Seg::Cubic { c1, c2, to } => {
                let p0 = *poly.last().expect("始点が必ず入っている");
                for i in 1..=steps {
                    let t = i as f64 / steps as f64;
                    let u = 1.0 - t;
                    let b = |a: f64, b: f64, c: f64, d: f64| {
                        u * u * u * a + 3.0 * u * u * t * b + 3.0 * u * t * t * c + t * t * t * d
                    };
                    poly.push((b(p0.0, c1.0, c2.0, to.0), b(p0.1, c1.1, c2.1, to.1)));
                }
            }
        }
    }
    poly
}

/// 点が閉じた折れ線の内側にあるか（偶奇規則）。
pub fn contains(poly: &[(f64, f64)], (x, y): (f64, f64)) -> bool {
    let mut inside = false;
    for i in 0..poly.len() {
        let (x1, y1) = poly[i];
        let (x2, y2) = poly[(i + 1) % poly.len()];
        if (y1 > y) != (y2 > y) && x < (x2 - x1) * (y - y1) / (y2 - y1) + x1 {
            inside = !inside;
        }
    }
    inside
}

#[cfg(test)]
mod tests {
    use super::*;

    use outline_points as points;

    /// 輪郭が矩形 `w`×`h` からはみ出さず、始点と終点が一致して閉じること。
    fn assert_closed_and_fits(o: &Outline, w: f64, h: f64) {
        let last = seg_to(*o.segs.last().expect("手が 1 つも無い"));
        assert!(
            (last.0 - o.start.0).abs() < 0.001 && (last.1 - o.start.1).abs() < 0.001,
            "輪郭が閉じていない: {:?} → {last:?}",
            o.start
        );
        for (x, y) in points(o) {
            assert!((-0.001..=w + 0.001).contains(&x), "x が矩形外: {x}");
            assert!((-0.001..=h + 0.001).contains(&y), "y が矩形外: {y}");
        }
    }

    /// 半径の和が辺長を超えるとき CSS と同じ比率縮小がかかる。
    /// squircle の 10pt 角丸を高さ 12pt の体に適用すると 2 隅で 20 > 12 になる。
    #[test]
    fn oversized_corners_are_scaled_down() {
        let (w, h) = (30.0, 12.0);
        assert_closed_and_fits(&outline(w, h, [(10.0, 10.0); 4]), w, h);
    }
}
