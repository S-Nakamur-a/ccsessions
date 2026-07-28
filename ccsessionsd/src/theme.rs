//! 元デザイン（HTML/CSS のモック）を Rust の定数・純関数へ写経した層。
//!
//! ここには **FFI を一切置かない**。色・寸法・形・アニメの仕様だけを持ち、
//! `creature.rs` がこれを読んで CALayer を組む。デザインのイテレーション
//! （「もう少し大きく」「グローを強く」）はこのファイルの一行編集で完結させたい。
//!
//! # 座標系の約束
//! 元デザインは CSS（y は下向き）。CALayer はホスト NSView が非 flipped なので
//! **y は上向き**。そのため「CSS の `translateY(-3px)`（上へ 3px）」は
//! ここでは `+3.0` と書く。各アニメ仕様のコメントに CSS 側の値を併記してある。
//!
//! # 色の由来
//! 元デザインは `oklch()` 指定。CALayer は sRGB 成分しか受けないので、
//! oklch → sRGB を事前計算した数値を焼き込んである（`scripts/oklch.py` 相当の変換）。
//! `working` の accent だけは sRGB 域外（赤成分が負）なので 0.0 にクランプ済み。
//! 面の塗りは CSS の `color-mix(in oklch, <accent> 22%, #0b0912)` を
//! 同じ極座標補間（短い方の色相弧）で解いた結果。

use ccsessions_core::session::SessionState;

/// 顔の形（`Outline` / `Seg` / `outline()` / `Size`）は `ccsessions_core::face` にある。
/// **顔ごとに変わるものは `faces/*.toml` が持ち、このファイルには「全部の顔に
/// 共通のデザイン定数」だけが残る**（`docs/adr/0014-faces-as-data.md`）。
///
/// `ccsessions` CLI（`ccsessionsd` に依存しない）が FFI 抜きで顔の SVG を描けるように
/// core 側に置いてある。既存の呼び出し側を無変更で保つため、ここから re-export する。
pub use ccsessions_core::face::{outline, Outline, Seg, Size};

/// パレット（`Rgb` / `accent` / `face_fill` / 減光率 / 目の色）も同じ理由で
/// `ccsessions_core::face::palette` へ移設済み。ここから re-export する。
pub use ccsessions_core::face::palette::{accent, face_fill, face_opacity, Rgb, EYE, INK};

/// 枠線幅とパネル線の太さ・濃さも SVG プレビューと共有する（`face::style`）。
pub use ccsessions_core::face::style::{detail_line_w, BORDER_W, DETAIL_ALPHA};

// ---------------------------------------------------------------------------
// パレット
// ---------------------------------------------------------------------------

/// 判断待ちの吹き出し文字色。元: `#241a00`
pub const BUBBLE_INK: Rgb = (0.1412, 0.1020, 0.0000);
/// `z`（idle）の色。元: `#aeb6c8`
pub const Z_COLOR: Rgb = (0.6824, 0.7137, 0.7843);

/// ホバーカード：背景 / 枠 / 主テキスト / 副テキスト。
/// 元: `#0a0e14f7` / `#ffffff20` / `#f2f4f9` / `oklch(0.56 0.02 260)`
pub const CARD_BG: (f64, f64, f64, f64) = (0.0392, 0.0549, 0.0784, 0.969);
pub const CARD_BG_DOCK: (f64, f64, f64, f64) = (0.0588, 0.0431, 0.1020, 0.969);
pub const CARD_BORDER: (f64, f64, f64, f64) = (1.0, 1.0, 1.0, 0.125);
pub const CARD_TEXT: Rgb = (0.9490, 0.9569, 0.9765);
pub const CARD_TEXT_DIM: Rgb = (0.4302, 0.4585, 0.5041);
/// カード内 agent 行の名前色。元: `#e6e9f0`
pub const CARD_AGENT: Rgb = (0.9020, 0.9137, 0.9412);

/// dock パネルの背景・枠。元: `#0a0812cc` / `#ffffff16`
///
/// 背景は**色と不透明度を分けて持つ**。カーソルが乗っているかで濃さを変えるため
/// （`DOCK_PANEL_ALPHA_HOVER` / `DOCK_PANEL_ALPHA_IDLE`）。
pub const DOCK_PANEL_BG: Rgb = (0.0392, 0.0314, 0.0706);
pub const DOCK_PANEL_BORDER: (f64, f64, f64, f64) = (1.0, 1.0, 1.0, 0.086);

/// カーソルが dock に乗っているときの背景の不透明度。
///
/// **触っているあいだは濃くする。** 掴んで動かす・生き物を見分けるという
/// 「読む」操作をしている最中なので、背景を締めてパネルの輪郭と中身を
/// はっきりさせる方がよい。
pub const DOCK_PANEL_ALPHA_HOVER: f64 = 0.75;

/// カーソルが離れているときの背景の不透明度。
///
/// **放っておくときは薄くして背景に馴染ませる。** 常時表示のオーバーレイなので、
/// 使っていないあいだは下にあるものを邪魔しないことを優先する（生き物自体は
/// 不透明なままなので、状態は薄い背景越しでも読める）。
pub const DOCK_PANEL_ALPHA_IDLE: f64 = 0.30;

/// 2 つの濃さを行き来するフェードの時間（秒）。
///
/// 短すぎるとパチパチと切り替わって落ち着かず、長すぎるとカーソルに追随していない
/// ように感じる。CSS の `transition` でよく使う 150〜200ms の真ん中を採る。
pub const DOCK_PANEL_FADE_SECS: f64 = 0.18;

/// bar 配置で体の上に確保する余白（pt）。グリフ 1 個ぶん。
///
/// **なぜ bar だけ切り詰めるか**: メニューバーの高さは機種依存で、ノッチ機でも
/// 33pt しかない（`safeAreaInsets.top`=32・実測 33）。ウィンドウはこの高さに収める
/// 必要がある — 帯より下へ伸ばすと、その矩形ぶんだけメニューバー下のクリックを
/// 奪ってしまう。
///
/// そこで bar では:
/// - 吹き出し（`!`）と `z` を**出さない**。どちらもグリフ（`!` / `z`）と同じ情報なので、
///   狭い帯ではグリフ 1 個に集約するのが素直（dock では元デザインどおり全部出す）。
/// - バッジを体の外へ垂らさず、右下に**重ねる**。
///
/// これで 体 20 + 上 12 = 32pt に収まり、33pt のメニューバーにちょうど入る。
///
/// 内訳は **グリフを浮かせる 8pt（`glyph_offset(Bar).1`）＋ 縦アニメの 4pt
/// （`BAR_MAX_ANIM_AMP`）**。`bar_headroom_is_the_glyph_overhang_plus_the_animation_budget`
/// がこの等式を固定している。
pub const BAR_HEADROOM: f64 = 12.0;

/// bar で体が上へ飛び出す最大量（pt）。`face_anim` の縦アニメの中で最も大きい
/// 判断待ちの hop の頂点（`anim_scale = 1.0` のとき 4.0）。
///
/// **`layout::bar_fit` がアニメを絞る割合の分母になる**。ここを `BAR_HEADROOM`（12）に
/// すると、24pt のメニューバー（余地 4pt）で倍率が 1/3 になり、bob の振幅が 0.67pt
/// ＝**事実上の静止**になる。絞る目的は「窓の上端で切らないこと」なので、
/// 分母は確保すべき余地そのもの＝この振幅でなければならない。4pt 空いていれば
/// 4pt 跳べる、が正しい対応で、24pt でも 33pt でも 1.0 になり見た目は変わらない。
pub const BAR_MAX_ANIM_AMP: f64 = 4.0;

/// この配置で吹き出し（`!`）と `z` を出すか。bar では出さない（上記の理由）。
pub fn has_flourishes(size: Size) -> bool {
    !size.is_bar()
}

/// 生き物どうしの間隔（pt）。元: bar `gap:9px` / dock `gap:16px`
pub fn flock_gap(size: Size) -> f64 {
    if size.is_bar() {
        9.0
    } else {
        16.0
    }
}
/// 外側グローの CALayer shadowRadius（pt）。元: `box-shadow: 0 0 13px -2px`
/// （CSS blur 13 ≒ CALayer radius 6.5、spread -2 を引いて 5.5 前後）。
pub const GLOW_RADIUS: f64 = 5.5;
pub const GLOW_OPACITY: f32 = 0.95;
/// 内側グロー（`inset 0 0 9px -4px`）を近似するストローク幅と濃さ。
pub const INNER_GLOW_W: f64 = 3.0;
pub const INNER_GLOW_ALPHA: f64 = 0.38;

/// パネル線の滲み（SVG では近似できないので `ccsessionsd` 固有）。
pub const DETAIL_GLOW: f64 = 2.5;
pub const DETAIL_GLOW_OPACITY: f32 = 0.55;

// ---------------------------------------------------------------------------
// 付属パーツ（グリフ・バッジ・吹き出し・名前・経過時間）
// ---------------------------------------------------------------------------

/// グリフ（体の右上に浮く記号）。元: `top:-9px; right:-6px; 11px bold mono`
pub fn glyph_font(size: Size) -> f64 {
    if size.is_bar() {
        10.0
    } else {
        11.0
    }
}
/// グリフのはみ出し量 (右, 上)。bar では上への逃げを `BAR_HEADROOM` 内に収める。
pub fn glyph_offset(size: Size) -> (f64, f64) {
    if size.is_bar() {
        (5.0, 8.0)
    } else {
        (6.0, 9.0)
    }
}

/// エージェント数バッジ。元: `bottom:-5px; right:-5px; min 14x14; radius 7; 9px`
pub fn badge_h(size: Size) -> f64 {
    if size.is_bar() {
        12.0
    } else {
        14.0
    }
}
pub fn badge_min_w(size: Size) -> f64 {
    badge_h(size)
}
pub const BADGE_FONT: f64 = 9.0;
/// バッジのはみ出し量 (右, 下)。bar では下へ垂らさず体に重ねる（帯に収めるため）。
pub fn badge_offset(size: Size) -> (f64, f64) {
    if size.is_bar() {
        (3.0, 0.0)
    } else {
        (5.0, 5.0)
    }
}

/// 吹き出し（判断待ちの `!`）。元: `min-width:14px; height:15px; radius 6 6 6 2`
pub const BUBBLE_H: f64 = 15.0;
pub const BUBBLE_MIN_W: f64 = 14.0;
pub const BUBBLE_FONT: f64 = 10.0;
/// 体の上端からの距離。元: `top: bar ? -13 : -16`
pub fn bubble_top(size: Size) -> f64 {
    if size.is_bar() {
        13.0
    } else {
        16.0
    }
}

/// dock 表示の名前ラベルと経過時間。元: 名前 6.5px `Press Start 2P` / 経過 8px mono
pub const NAME_FONT: f64 = 8.0;
pub const DUR_FONT: f64 = 8.0;
pub const NAME_MAX_W: f64 = 60.0;
/// 体と名前ラベルの間隔。元: `gap:7px`
pub const NAME_GAP: f64 = 7.0;

// ---------------------------------------------------------------------------
// アニメーション仕様
// ---------------------------------------------------------------------------

/// 体に付ける主アニメ。`reduce_motion` のときは `None` を使う。
#[derive(Debug, Clone, PartialEq)]
pub enum FaceAnim {
    /// 上下に揺れる（作業中）。元 `bob 1.2s`: 0 → -3px → 0
    Bob { amp: f64, half_secs: f64 },
    /// 跳ねる（判断待ち）。元 `hop 1.5s`: 30% で -8px、46% で -1px、62% で 0
    Hop {
        keys: [f64; 5],
        values: [f64; 5],
        secs: f64,
    },
    /// 横に漂う（エージェント待ち）。元 `drift 3s`: 0 → +2.5px → 0
    Drift { amp: f64, half_secs: f64 },
    /// ゆっくり明滅（エラー）。元 `errBreath 2.2s`。**グリッチは廃止**し、
    /// 目に優しい呼吸にする（明度とグローの半径だけを往復させる）。
    Breath {
        opacity: (f32, f32),
        glow: (f64, f64),
        half_secs: f64,
    },
    /// 動かない（アイドル・完了）。
    None,
}

/// 状態から体のアニメを決める。元デザインの `faceAnim` の写経。
/// 値は **CALayer 座標（y 上向き）** に変換済み — CSS の `-3px`（上）は `+3.0`。
///
/// bar では上下の振れ幅を小さくする。帯の高さがメニューバーぶんしかないので、
/// 元の 8pt ジャンプだと体がグリフに食い込み、窓の上端で切れて見える。
///
/// `anim_scale` は**振れ幅の倍率**で、呼び出し側（`creature`）が 2 つの都合を
/// 掛け合わせて渡す:
/// 1. bar で上へ飛び出せる余地（`layout::BarFit::anim_scale`）。低いメニューバーでは
///    体がグリフに食い込んで窓の上端で切れるので、そのぶん絞る。
/// 2. コンパクト表示の縮小率（`layout::Squeeze::scale`）。体が 0.6 倍なのに跳躍だけ
///    等倍だと、比率が壊れるうえ縮んだ余白を突き抜ける。
///
/// **dock でも倍率を掛ける**（かつて bar 限定だった）。dock は `BarFit::ROOMY` で
/// 縮小も無ければ 1.0 になるので、既存の見た目は変わらない。
pub fn face_anim(s: SessionState, size: Size, anim_scale: f64) -> FaceAnim {
    let bar = size.is_bar();
    let v = anim_scale;
    match s {
        SessionState::Working => FaceAnim::Bob {
            amp: if bar { 2.0 * v } else { 3.0 * v },
            half_secs: 0.6,
        },
        SessionState::WaitUser => FaceAnim::Hop {
            keys: [0.0, 0.30, 0.46, 0.62, 1.0],
            values: if bar {
                [0.0, 4.0 * v, 0.5 * v, 0.0, 0.0]
            } else {
                [0.0, 8.0 * v, 1.0 * v, 0.0, 0.0]
            },
            secs: 1.5,
        },
        SessionState::WaitAgent => FaceAnim::Drift {
            amp: 2.5 * v,
            half_secs: 1.5,
        },
        SessionState::Error => FaceAnim::Breath {
            opacity: (0.82, 1.0),
            glow: (3.0, 9.0),
            half_secs: 1.1,
        },
        SessionState::Idle | SessionState::Done => FaceAnim::None,
    }
}

/// 瞬き（作業中の目）。元 `eyeblink 4s`: 95% の瞬間だけ scaleY 0.12。
pub const BLINK_KEYS: [f64; 4] = [0.0, 0.90, 0.95, 1.0];
pub const BLINK_VALUES: [f64; 4] = [1.0, 1.0, 0.12, 1.0];
pub const BLINK_SECS: f64 = 4.0;

/// `z` の浮遊（アイドル）。元 `floatZ 2.8s`:
/// 0% (0,0) scale .7 opacity 0 → 25% opacity 1 → 100% (+7,+11) scale 1 opacity 0。
/// （CSS の `translate(7px,-11px)` は上方向なので y は正）
pub const FLOAT_Z_SECS: f64 = 2.8;
pub const FLOAT_Z_TO: (f64, f64) = (7.0, 11.0);
pub const FLOAT_Z_SCALE: (f64, f64) = (0.7, 1.0);
pub const FLOAT_Z_OPACITY_KEYS: [f64; 3] = [0.0, 0.25, 1.0];
pub const FLOAT_Z_OPACITY: [f32; 3] = [0.0, 1.0, 0.0];

/// 吹き出しの上下揺れ。元 `bubblePop 1.5s`: 0 → -2px → 0（上へ 2px）
pub const BUBBLE_POP_AMP: f64 = 2.0;
pub const BUBBLE_POP_HALF_SECS: f64 = 0.75;

/// カード内の agent ドット。元 `jitter 1.2s`（作業中）/ `softPulse 2.2s`（エラー）
pub const DOT_SIZE: f64 = 7.0;
pub const JITTER_AMP: f64 = 1.0;
pub const JITTER_SECS: f64 = 1.2;
pub const SOFT_PULSE_OPACITY: (f32, f32) = (0.55, 1.0);
pub const SOFT_PULSE_HALF_SECS: f64 = 1.1;

// ---------------------------------------------------------------------------
// ホバーカード
// ---------------------------------------------------------------------------

/// カードの内寸。元: `padding: 8px 10px; radius 9; 名前 11px / ja 9.5px / dur 10px`
pub const CARD_PAD_X: f64 = 10.0;
pub const CARD_PAD_Y: f64 = 8.0;
pub const CARD_RADIUS: f64 = 9.0;
pub const CARD_TITLE_FONT: f64 = 11.0;
pub const CARD_JA_FONT: f64 = 9.5;
pub const CARD_DUR_FONT: f64 = 10.0;
pub const CARD_AGENT_FONT: f64 = 10.5;
pub const CARD_ROLE_FONT: f64 = 9.5;
pub const CARD_TITLE_H: f64 = 15.0;
pub const CARD_ROW_H: f64 = 15.0;

/// セッションタイトルの行（プロジェクト名の下に出る 2 行目）。
/// 名前より一段小さく・薄くして、主役はあくまで状態と名前のままにする。
pub const CARD_SUBTITLE_FONT: f64 = 9.5;
pub const CARD_SUBTITLE_H: f64 = 13.0;
/// タイトル行の上に空ける隙間（名前と詰まりすぎないように）。
pub const CARD_SUBTITLE_GAP: f64 = 2.0;
/// タイトルの最大表示幅（pt）。**これが無いとカードが際限なく横に伸びる** —
/// タイトルは自動生成の日本語 1 文で、長さを制御できない。超える分は省略する。
pub const CARD_SUBTITLE_MAX_W: f64 = 240.0;
/// 体からカードまでの距離（bar のみ使用）。元: bar `top:30px`
pub fn card_offset(size: Size) -> f64 {
    if size.is_bar() {
        4.0
    } else {
        26.0
    }
}

/// dock 配置でカードを背景パネルの上端からどれだけ浮かせるか（pt）。
/// パネルの角丸の縁に被らない最小限の隙間。
pub const CARD_PANEL_GAP: f64 = 8.0;

// ---------------------------------------------------------------------------
// 帯・パネル
// ---------------------------------------------------------------------------

/// bar 帯の高さ。元: `height:26px`。メニューバー高より小さければバーの中に収まる。
pub const BAR_BAND_H: f64 = 26.0;
/// dock パネルの内側余白。元: `padding:13px 19px 11px`
pub const DOCK_PAD_X: f64 = 19.0;
pub const DOCK_PAD_TOP: f64 = 13.0;
pub const DOCK_PAD_BOTTOM: f64 = 11.0;
pub const DOCK_RADIUS: f64 = 17.0;
/// dock パネルの画面下端からの距離。元: `bottom:20px`
pub const DOCK_BOTTOM_MARGIN: f64 = 20.0;
