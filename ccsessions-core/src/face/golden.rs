//! 移行前からある組込み 4 顔（egg / round / squircle / bean）の
//! **数値を固定する golden テスト**。移行後に足した顔は「移行前の見た目」を
//! 持たないので、ここには入らない。
//!
//! 顔を `faces/*.toml` のデータ駆動へ移すとき、安全弁としてまず「TOML 由来の値が
//! 旧 `ccsessionsd/src/theme.rs` のハードコードと一致する」過渡テストを回した。
//! **そのテストは旧関数を消した時点で書けなくなる**ので、そこで確認した数値を
//! ここにリテラルとして焼き直してある。
//!
//! つまりこのファイルは「移行前の見た目」のスナップショットで、
//! `faces/*.toml` や解決ロジックをうっかり変えたときの番人になる。
//!
//! # 落とし穴（過渡テストが実際に踏んだもの）
//! 輪郭は**体の寸法そのまま**と**枠線幅ぶん内側に縮めた寸法**の両方で照合すること。
//! `creature.rs` は後者（`bw - BORDER_W, bh - BORDER_W`）で輪郭を作るので、
//! 前者だけで比べると bean のような「半径が `h` に追随する」顔の取り違えを見逃す。

#![cfg(test)]

use crate::face::palette;
use crate::face::{Registry, Size};
use crate::session::SessionState;

/// `ccsessionsd/src/theme.rs` の枠線幅。輪郭の照合を実際の描画寸法でも行うために持つ。
const BORDER_W: f64 = 1.5;

/// 4 隅の角丸（水平半径, 垂直半径）。
type FourCorners = [(f64, f64); 4];

fn near(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}

/// 体の寸法（移行前の `theme::body_size`）。
#[test]
fn body_sizes_are_unchanged() {
    let reg = Registry::builtin();
    let expected = [
        ("egg", (22.0, 20.0), (36.0, 34.0)),
        ("round", (20.0, 20.0), (32.0, 32.0)),
        ("squircle", (22.0, 20.0), (36.0, 34.0)),
        ("bean", (28.0, 20.0), (44.0, 34.0)),
    ];
    for (id, bar, dock) in expected {
        let f = reg.get(id).unwrap_or_else(|| panic!("{id} が無い"));
        assert_eq!(f.body_size(Size::Bar), bar, "{id} の bar の体");
        assert_eq!(f.body_size(Size::Dock), dock, "{id} の dock の体");
    }
}

/// 角丸（移行前の `theme::corners`）。**描画で使われる inset 後の寸法でも**照合する。
#[test]
fn corner_radii_are_unchanged() {
    let reg = Registry::builtin();

    // (id, size, 体の寸法での 4 隅, inset 後の寸法での 4 隅)
    let cases: [(&str, Size, FourCorners, FourCorners); 8] = [
        // egg: 比率 0.50/0.58（上）0.48/0.42（下）
        (
            "egg",
            Size::Bar,
            [(11.0, 11.6), (11.0, 11.6), (10.56, 8.4), (10.56, 8.4)],
            [(10.25, 10.73), (10.25, 10.73), (9.84, 7.77), (9.84, 7.77)],
        ),
        (
            "egg",
            Size::Dock,
            [(18.0, 19.72), (18.0, 19.72), (17.28, 14.28), (17.28, 14.28)],
            [
                (17.25, 18.85),
                (17.25, 18.85),
                (16.56, 13.65),
                (16.56, 13.65),
            ],
        ),
        // round: 真円
        ("round", Size::Bar, [(10.0, 10.0); 4], [(9.25, 9.25); 4]),
        ("round", Size::Dock, [(16.0, 16.0); 4], [(15.25, 15.25); 4]),
        // squircle: pt 固定（inset しても 7 / 10 のまま）
        ("squircle", Size::Bar, [(7.0, 7.0); 4], [(7.0, 7.0); 4]),
        ("squircle", Size::Dock, [(10.0, 10.0); 4], [(10.0, 10.0); 4]),
        // bean: カプセル。**inset で h が縮むと半径も追随する**（ここが capsule の要点）
        ("bean", Size::Bar, [(10.0, 10.0); 4], [(9.25, 9.25); 4]),
        ("bean", Size::Dock, [(17.0, 17.0); 4], [(16.25, 16.25); 4]),
    ];

    for (id, size, raw, inset) in cases {
        let f = reg.get(id).unwrap_or_else(|| panic!("{id} が無い"));
        let (w, h) = f.body_size(size);

        for (label, (cw, ch), want) in [
            ("体の寸法", (w, h), raw),
            ("inset 後", (w - BORDER_W, h - BORDER_W), inset),
        ] {
            let got = f.corners(cw, ch, size);
            for i in 0..4 {
                assert!(
                    near(got[i].0, want[i].0) && near(got[i].1, want[i].1),
                    "{id}/{size:?} の{label}の角 {i}: {:?} だが {:?} のはず",
                    got[i],
                    want[i]
                );
            }
        }
    }
}

/// bean がカプセルであり続ける（旧 `theme.rs::bean_is_a_capsule` の後継）。
///
/// **inset 後でも左右が半円**であることまで見る。`corners_pt` で写経すると
/// ここが壊れる（半径が固定 pt のまま取り残されて `rx != ry` になる）。
#[test]
fn bean_stays_a_capsule_at_every_size() {
    let reg = Registry::builtin();
    let bean = reg.get("bean").expect("bean が無い");
    for size in [Size::Bar, Size::Dock] {
        let (w, h) = bean.body_size(size);
        for (cw, ch) in [(w, h), (w - BORDER_W, h - BORDER_W)] {
            for (rx, ry) in bean.corners(cw, ch, size) {
                assert!(
                    near(rx, ch / 2.0),
                    "水平半径が h/2 でない: {rx} vs {}",
                    ch / 2.0
                );
                assert!(
                    near(ry, ch / 2.0),
                    "垂直半径が h/2 でない: {ry} vs {}",
                    ch / 2.0
                );
            }
        }
    }
}

/// 目の間隔（移行前の `theme::eye_gap`）。
#[test]
fn eye_gaps_are_unchanged() {
    let reg = Registry::builtin();
    for id in ["egg", "round", "squircle", "bean"] {
        let f = reg.get(id).unwrap();
        assert_eq!(f.eye_gap(Size::Bar), 3.0, "{id} の bar の目の間隔");
        assert_eq!(f.eye_gap(Size::Dock), 5.0, "{id} の dock の目の間隔");
    }
}

/// 角丸の目の 6 状態（移行前の `theme::eye` の角丸系）。
///
/// `(w, h, radius)` を全状態ぶん固定する。**bar と dock で倍率が違う**のが要点で、
/// ここが「状態別ルールを TOML の `w_scale`/`h_scale` で書けない」根拠でもある。
#[test]
fn rounded_eye_specs_are_unchanged() {
    let reg = Registry::builtin();
    // (状態, bar の (w,h,radius), dock の (w,h,radius))
    let cases = [
        (SessionState::Working, (3.0, 4.0, 2.0), (4.0, 6.0, 2.0)),
        (SessionState::WaitUser, (4.0, 4.0, 2.0), (5.0, 5.0, 2.5)),
        (SessionState::WaitAgent, (3.0, 4.0, 2.0), (4.0, 6.0, 2.0)),
        (SessionState::Idle, (4.0, 2.0, 1.0), (5.0, 2.0, 1.0)),
        (SessionState::Done, (3.0, 4.0, 2.0), (4.0, 6.0, 2.0)),
        (SessionState::Error, (3.0, 4.0, 2.0), (4.0, 6.0, 2.0)),
    ];
    // 角丸の目を持つ 4 顔はすべて同じ寸法表。
    for id in ["egg", "round", "squircle", "bean"] {
        let f = reg.get(id).unwrap();
        for (state, bar, dock) in cases {
            for (size, want) in [(Size::Bar, bar), (Size::Dock, dock)] {
                let e = f.eye(state, size);
                assert!(
                    near(e.w, want.0) && near(e.h, want.1) && near(e.radius, want.2),
                    "{id}/{size:?}/{state:?}: ({}, {}, {}) だが {want:?} のはず",
                    e.w,
                    e.h,
                    e.radius
                );
                assert_eq!(e.dy, 0.0, "{id} の目は顔の中央にある");
            }
        }
    }
}

/// 目の色・滲み・横目・瞬き（状態の読み取りやすさの核）。
#[test]
fn rounded_eye_decorations_are_unchanged() {
    let f = Registry::builtin().get("egg").unwrap().clone();
    let bar = Size::Bar;

    assert!(f.eye(SessionState::Working, bar).blink, "作業中は瞬きする");
    let wu = f.eye(SessionState::WaitUser, bar);
    assert_eq!(wu.color, (1.0, 1.0, 1.0), "判断待ちは白い目");
    assert_eq!(wu.glow, 4.0, "判断待ちは滲む");
    assert_eq!(f.eye(SessionState::WaitAgent, bar).dx, 1.5, "横目");
    assert_eq!(f.eye(SessionState::Idle, bar).color, palette::EYE_CLOSED);
    assert_eq!(f.eye(SessionState::Error, bar).color, palette::EYE_ERROR);
    assert_eq!(f.eye(SessionState::Done, bar).color, palette::EYE);
    assert_eq!(f.eye(SessionState::Done, bar).glow, 0.0);
}

/// 組込み顔は**どれも角丸の目とパネル線なし**。
///
/// 多角形の目とパネル線は「顔の形式が持つ機能」であって組込み顔の性質ではない
/// （番人は `spec.rs` の `eye_polygon_mirrors_for_the_left_eye` /
/// `details_are_filtered_by_size` と、ビルダーのテスト）。ここは
/// **組込み側にうっかり線画や多角形が生えたら気づく**ためのスナップショット。
#[test]
fn every_builtin_face_draws_rounded_eyes_without_panel_lines() {
    for id in ["egg", "round", "squircle", "bean"] {
        let f = Registry::builtin().get(id).unwrap().clone();
        assert!(f.eye_shape(9.0, 5.0, false).is_none(), "{id} が多角形の目");
        assert!(f.face_details(22.0, 20.0, Size::Bar).is_empty(), "{id}");
        assert!(f.face_details(36.0, 34.0, Size::Dock).is_empty(), "{id}");
    }
}
