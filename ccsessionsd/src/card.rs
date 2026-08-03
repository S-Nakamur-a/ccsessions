//! ホバーカード — 生き物にカーソルを乗せたときに出る詳細パネル。
//!
//! 1 行目: セッション名 / 状態（設定の言語）/ 経過時間（右寄せ）
//! 2 行目: セッションタイトル（**あるときだけ**。無ければこの行ごと出ない）
//! 以降: そのセッションが走らせている agent（状態ドット + 名前 + 役割）
//!
//! ホバーは頻繁には変わらないので、**カードは切り替えのたびに作り直す**
//! （生き物と違いアニメの位相を保つ必要が無く、差分更新の複雑さに見合わない）。

use objc2::rc::Retained;
use objc2_quartz_core::{CALayer, CAShapeLayer, CATransaction};

use ccsessions_core::face::FaceSpec;
use ccsessions_core::session::SessionState;

use crate::anim;
use crate::ffi::{as_path, cgcolor, path_from_outline, rect, set_glow, solid_layer, text_layer};
use crate::text::{ellipsize, text_width};
use crate::theme::{self, Size};

/// カードに出す 1 セッションぶんの情報。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CardView {
    pub name: String,
    /// Claude Code のセッションタイトル。無いセッションもある（`Session::title`）。
    pub title: Option<String>,
    /// 状態そのもの。**色を引くために持つ**（`theme::accent`）。
    pub state: SessionState,
    /// 画面に出す状態名。**言語はここで解決済み**にしてある。
    ///
    /// 描画の奥まで設定を持ち回らずに済み、`CardView` の比較（`PartialEq`）だけで
    /// 「言語が変わったから作り直す」も自然に効く。解決するのは `flock::card_view_of`。
    pub state_label: &'static str,
    pub dur: String,
    pub agents: Vec<AgentRow>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentRow {
    pub name: String,
    pub role: String,
    pub state: SessionState,
}

/// カードのレイヤと、その大きさ。
pub struct Card {
    pub layer: Retained<CALayer>,
    pub width: f64,
    pub height: f64,
}

/// タイトル行の各要素の間隔（pt）。元デザインの `gap:8px`。
const TITLE_GAP: f64 = 8.0;
/// 経過時間の前に最低限空ける距離（元は `margin-left:auto` の右寄せ）。
const DUR_GAP: f64 = 14.0;
/// agent 行のドットとテキストの間隔。元デザインの `gap:6px`。
const ROW_GAP: f64 = 6.0;

/// カードを組む。`size` は bar / dock（背景色がわずかに違う）。
/// `face` は色を引くためだけに要る。**カードの縁とドットは生き物と同じ色**でなければ
/// ならず（同じ状態を指しているので）、顔が `[colors.<状態>]` を持っていれば
/// そちらが真実になる。
pub fn build(v: &CardView, size: Size, scale: f64, reduce_motion: bool, face: &FaceSpec) -> Card {
    let accent = face.accent(v.state);

    // --- 大きさを見積もる -------------------------------------------------
    let name_w = text_width(&v.name, theme::CARD_TITLE_FONT);
    let ja_w = text_width(v.state_label, theme::CARD_JA_FONT);
    let dur_w = text_width(&v.dur, theme::CARD_DUR_FONT);
    let title_w = name_w + TITLE_GAP + ja_w + DUR_GAP + dur_w;

    // セッションタイトルは長さを制御できないので、幅を測る前に切っておく。
    let subtitle = v
        .title
        .as_deref()
        .map(|t| ellipsize(t, theme::CARD_SUBTITLE_FONT, theme::CARD_SUBTITLE_MAX_W));
    let subtitle_w = subtitle
        .as_deref()
        .map_or(0.0, |t| text_width(t, theme::CARD_SUBTITLE_FONT));
    let subtitle_h = if subtitle.is_some() {
        theme::CARD_SUBTITLE_GAP + theme::CARD_SUBTITLE_H
    } else {
        0.0
    };

    let rows_w = v
        .agents
        .iter()
        .map(|a| {
            theme::DOT_SIZE
                + ROW_GAP
                + text_width(&a.name, theme::CARD_AGENT_FONT)
                + ROW_GAP
                + text_width(&a.role, theme::CARD_ROLE_FONT)
        })
        .fold(0.0_f64, f64::max);

    let inner_w = title_w.max(rows_w).max(subtitle_w);
    let width = inner_w + theme::CARD_PAD_X * 2.0;
    let height = theme::CARD_PAD_Y * 2.0
        + theme::CARD_TITLE_H
        + subtitle_h
        + v.agents.len() as f64 * theme::CARD_ROW_H;

    // --- 背景（角丸 + 枠 + 影） -------------------------------------------
    let layer = CALayer::new();
    layer.setFrame(rect(0.0, 0.0, width, height));
    let bg = if size.is_bar() {
        theme::CARD_BG
    } else {
        theme::CARD_BG_DOCK
    };
    layer.setBackgroundColor(Some(&cgcolor((bg.0, bg.1, bg.2), bg.3)));
    layer.setCornerRadius(theme::CARD_RADIUS);
    layer.setBorderWidth(1.0);
    let b = theme::CARD_BORDER;
    layer.setBorderColor(Some(&cgcolor((b.0, b.1, b.2), b.3)));
    // 元デザインの `box-shadow: 0 14px 32px -12px #000` — 下に落ちる濃い影。
    layer.setShadowColor(Some(&cgcolor((0.0, 0.0, 0.0), 1.0)));
    layer.setShadowRadius(10.0);
    layer.setShadowOpacity(0.55);
    layer.setShadowOffset(objc2_foundation::NSSize {
        width: 0.0,
        height: -6.0,
    });

    // --- タイトル行 --------------------------------------------------------
    let title_y = height - theme::CARD_PAD_Y - theme::CARD_TITLE_H;
    let name_l = text_layer(
        &v.name,
        theme::CARD_TITLE_FONT,
        theme::CARD_TEXT,
        1.0,
        true,
        scale,
    );
    name_l.setFrame(rect(
        theme::CARD_PAD_X,
        title_y,
        name_w + 2.0,
        theme::CARD_TITLE_H,
    ));
    layer.addSublayer(&name_l);

    let ja_l = text_layer(
        v.state_label,
        theme::CARD_JA_FONT,
        accent,
        1.0,
        false,
        scale,
    );
    ja_l.setFrame(rect(
        theme::CARD_PAD_X + name_w + TITLE_GAP,
        title_y - 0.5,
        ja_w + 2.0,
        theme::CARD_TITLE_H,
    ));
    layer.addSublayer(&ja_l);

    // 経過時間は右端に揃える（元デザインの `margin-left:auto`）。
    let dur_l = text_layer(&v.dur, theme::CARD_DUR_FONT, accent, 1.0, true, scale);
    dur_l.setFrame(rect(
        width - theme::CARD_PAD_X - dur_w - 2.0,
        title_y,
        dur_w + 2.0,
        theme::CARD_TITLE_H,
    ));
    layer.addSublayer(&dur_l);

    // --- セッションタイトル行（あるときだけ） ------------------------------
    if let Some(t) = &subtitle {
        let sub_y = title_y - theme::CARD_SUBTITLE_GAP - theme::CARD_SUBTITLE_H;
        let sub_l = text_layer(
            t,
            theme::CARD_SUBTITLE_FONT,
            theme::CARD_TEXT_DIM,
            1.0,
            false,
            scale,
        );
        sub_l.setFrame(rect(
            theme::CARD_PAD_X,
            sub_y,
            subtitle_w + 2.0,
            theme::CARD_SUBTITLE_H,
        ));
        layer.addSublayer(&sub_l);
    }

    // --- agent 行 ----------------------------------------------------------
    // タイトル行があるぶんだけ下げる（無ければ従来どおり名前行の直下から）。
    let rows_top = title_y - subtitle_h;
    for (i, a) in v.agents.iter().enumerate() {
        let y = rows_top - theme::CARD_ROW_H * (i as f64 + 1.0);
        add_agent_row(&layer, a, y, scale, reduce_motion, face);
    }

    Card {
        layer,
        width,
        height,
    }
}

/// agent 1 行（状態ドット + 名前 + 役割）を `y` の高さに敷く。
fn add_agent_row(
    layer: &CALayer,
    a: &AgentRow,
    y: f64,
    scale: f64,
    reduce_motion: bool,
    face: &FaceSpec,
) {
    let c = face.accent(a.state);

    // 状態ドット。作業中は小刻みに震え、エラーはゆっくり明滅する（元デザインの dotStyle）。
    let dot = solid_layer(c, 1.0, theme::DOT_SIZE / 2.0);
    let dot_alpha = match a.state {
        SessionState::Idle => 0.5,
        SessionState::Done => 0.85,
        _ => 1.0,
    };
    dot.setOpacity(dot_alpha as f32);
    dot.setFrame(rect(
        theme::CARD_PAD_X,
        y + (theme::CARD_ROW_H - theme::DOT_SIZE) / 2.0,
        theme::DOT_SIZE,
        theme::DOT_SIZE,
    ));
    set_glow(&dot, c, 3.0, 0.9);
    if !reduce_motion {
        match a.state {
            SessionState::Working => anim::jitter(&dot, theme::JITTER_AMP, theme::JITTER_SECS),
            SessionState::Error => {
                anim::soft_pulse(&dot, theme::SOFT_PULSE_OPACITY, theme::SOFT_PULSE_HALF_SECS)
            }
            _ => {}
        }
    }
    layer.addSublayer(&dot);

    let nx = theme::CARD_PAD_X + theme::DOT_SIZE + ROW_GAP;
    let nw = text_width(&a.name, theme::CARD_AGENT_FONT);
    let n = text_layer(
        &a.name,
        theme::CARD_AGENT_FONT,
        theme::CARD_AGENT,
        1.0,
        false,
        scale,
    );
    n.setFrame(rect(nx, y, nw + 2.0, theme::CARD_ROW_H));
    layer.addSublayer(&n);

    if a.role.is_empty() {
        return;
    }
    let r = text_layer(
        &a.role,
        theme::CARD_ROLE_FONT,
        theme::CARD_TEXT_DIM,
        1.0,
        false,
        scale,
    );
    r.setFrame(rect(
        nx + nw + ROW_GAP,
        y - 0.5,
        text_width(&a.role, theme::CARD_ROLE_FONT) + 2.0,
        theme::CARD_ROW_H,
    ));
    layer.addSublayer(&r);
}

/// dock 配置のパネル背景（生き物の群れを載せる角丸カード）。
///
/// 元デザインは `backdrop-filter: blur(9px)` を使うが、CALayer 単体では背景ぼかしが
/// できない。`NSVisualEffectView` を敷く手もあるが、常駐オーバーレイでのぼかしは
/// 電力を食うので、**半透明の暗色 + 薄い枠 + 影**で代替する（見た目の差は小さく、
/// 常時表示のコストの方が効く）。
/// `alpha` は背景の不透明度（カーソルが乗っているかで変わる）。
pub fn dock_panel(width: f64, height: f64, alpha: f64) -> Retained<CAShapeLayer> {
    let panel = CAShapeLayer::new();
    panel.setFrame(rect(0.0, 0.0, width, height));

    let r = theme::DOCK_RADIUS;
    let o = theme::outline(width, height, [(r, r); 4]);
    let p = path_from_outline(&o, (0.0, 0.0));
    panel.setPath(Some(as_path(&p)));

    set_dock_panel_alpha(&panel, alpha, 0.0);
    let b = theme::DOCK_PANEL_BORDER;
    panel.setStrokeColor(Some(&cgcolor((b.0, b.1, b.2), b.3)));
    panel.setLineWidth(1.0);

    panel.setShadowColor(Some(&cgcolor((0.0, 0.0, 0.0), 1.0)));
    panel.setShadowRadius(12.0);
    panel.setShadowOpacity(0.6);
    panel.setShadowOffset(objc2_foundation::NSSize {
        width: 0.0,
        height: -7.0,
    });
    panel
}

/// dock パネルの背景の濃さを差し替える。`secs` はフェードの時間（秒）。
///
/// **`secs = 0.0` は「アニメ無しで即座に」**。パネルを作り直した直後の貼り直しは
/// 必ずこちらで呼ぶこと — `fillColor` は `action(forKey:)` が nil を返すので
/// 「暗黙アニメは掛からない」と誤解しやすいが、**実際には既定 0.25 秒でフェードする**
/// （実測）。止めないと、セッションが 1 匹増減してレイヤを組み直すたびに背景が
/// ふわっと点灯して見える。
///
/// フェードは CoreAnimation がレンダーサーバ側で回すので、毎フレーム叩くタイマは要らない
/// （`anim.rs` の設計方針と同じ）。
pub fn set_dock_panel_alpha(panel: &CAShapeLayer, alpha: f64, secs: f64) {
    CATransaction::begin();
    if secs > 0.0 {
        CATransaction::setAnimationDuration(secs);
    } else {
        CATransaction::setDisableActions(true);
    }
    panel.setFillColor(Some(&cgcolor(theme::DOCK_PANEL_BG, alpha)));
    CATransaction::commit();
}
