//! 生き物 1 匹の CALayer ツリー。元デザインの 1 スロットに対応する。
//!
//! # レイヤ構成
//! ```text
//! root ─ スロット全体（当たり判定と同じ矩形）。位置は flock が決める
//!  ├ face ─ 体。bob / hop / drift / breath はここに掛ける
//!  │  ├ body        CAShapeLayer  塗り + 枠 + 外向きグロー
//!  │  ├ inner_host  CALayer(mask=体の形) ─ inner_stroke  内側グローの近似
//!  │  ├ details     CAShapeLayer  顔のパネル線（線画を持つ顔のみ。他は空）
//!  │  ├ eye_l / eye_r
//!  │  ├ glyph       体の右上に浮く記号
//!  │  └ badge + badge_text   右下のエージェント数
//!  ├ bubble + bubble_text    判断待ちの「!」（face とは別に揺れるので兄弟）
//!  ├ zmark                   アイドルの「z」（同上）
//!  ├ name / dur              dock 配置のみ。体の下 2 段
//! ```
//!
//! # なぜ「作り直さず apply する」か
//! 状態は数分おきに変わるが、レイヤを毎回作り直すとアニメの位相がリセットされ、
//! 群れ全体が不自然に同期して揺れる。ここでは**レイヤは使い回し**、色・形・文字と
//! アニメだけを差し替える。アニメは状態が実際に変わったときだけ貼り直す。

use std::cell::RefCell;

use objc2::rc::Retained;
use objc2_core_graphics::CGPath;
use objc2_quartz_core::{CALayer, CAShapeLayer, CATextLayer};

use std::sync::Arc;

use ccsessions_core::face::FaceSpec;
use ccsessions_core::session::SessionState;

use crate::anim;
use crate::ffi::{
    as_path, center_text, cgcolor, path_from_outline, path_from_polygon, path_from_strokes, rect,
    set_glow, set_text, set_text_color, solid_layer, text_layer, truncate_end,
};
use crate::layout::{BarFit, Slot, Squeeze};
use crate::text::text_width;
use crate::theme::{self, FaceAnim, Size};

/// 生き物 1 匹ぶんの表示状態。`flock` が `Session` から組んで渡す。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct View {
    pub state: SessionState,
    /// バッジに出すエージェント数。
    pub agents: usize,
    /// dock で体の下に出す短縮名。
    pub short: String,
    /// 経過時間の表示（`1h15m` など）。
    pub dur: String,
    /// 状態記号（`›` `!` `⋯` `z` `✓` `×`）を出すか。設定の `show_glyphs`。
    ///
    /// **レイアウトには影響しない** — グリフを消しても窓の大きさと体の位置は
    /// 変えない。消したり出したりするたびに群れが横にずれると、切り替えが
    /// 「表示の間引き」ではなく「別のレイアウト」に見えてしまう。
    pub show_glyph: bool,
}

pub struct Creature {
    root: Retained<CALayer>,
    face: Retained<CALayer>,
    body: Retained<CAShapeLayer>,
    inner_stroke: Retained<CAShapeLayer>,
    /// 内側グローを体の形で切り抜くマスク。
    ///
    /// `CALayer.mask` は CoreAnimation 側で retain される（strong プロパティ）ので
    /// 本来は保持しなくても生きるが、生成した所有権をここに置いておく方が
    /// 「誰が生かしているか」がコードから読めるので明示的に持つ。1 匹あたり
    /// ポインタ 1 個ぶんのコストしかない。
    _inner_mask: Retained<CAShapeLayer>,
    /// 顔のパネル線（`[[details]]` を持つ顔のみ中身がある）。
    details: Retained<CAShapeLayer>,
    /// 目は角丸矩形（背景色 + cornerRadius）でも多角形（path）でも描けるよう
    /// CAShapeLayer にしてある。どちらで描くかは `theme::eye_shape` が決める。
    eye_l: Retained<CAShapeLayer>,
    eye_r: Retained<CAShapeLayer>,
    glyph: Retained<CATextLayer>,
    badge: Retained<CALayer>,
    badge_text: Retained<CATextLayer>,
    bubble: Retained<CAShapeLayer>,
    bubble_text: Retained<CATextLayer>,
    zmark: Retained<CATextLayer>,
    /// dock 配置のみ存在する。
    name: Option<Retained<CATextLayer>>,
    dur: Option<Retained<CATextLayer>>,

    /// この生き物が使っている顔。輪郭・目・パネル線はここから解く。
    face_spec: Arc<FaceSpec>,
    size: Size,
    /// メニューバー高に合わせた詰め方（bar のみ意味を持つ）。
    fit: BarFit,
    /// コンパクト表示の縮小。**体だけでなく付属パーツの pt 値すべてに掛ける** —
    /// 体を縮めて目やグリフを据え置くと顔の比率が崩れ、しかも縮んだ余白を
    /// 突き抜ける（`layout::Squeeze` の doc 参照）。
    sq: Squeeze,
    body_w: f64,
    body_h: f64,
    /// 直前に適用した表示状態。差分が無ければ何もしない（位相リセットを避ける）。
    last: RefCell<Option<(View, bool)>>,
}

impl Creature {
    /// レイヤツリーを組む。`slot` は窓ローカルの配置、`scale` は Retina 倍率、
    /// `sq` はコンパクト表示の縮小（`slot` の体には適用済みなので、ここで使うのは
    /// **体からはみ出すパーツの pt 値**を揃えるため）。
    pub fn build(
        slot: Slot,
        face: Arc<FaceSpec>,
        size: Size,
        fit: BarFit,
        scale: f64,
        sq: Squeeze,
    ) -> Self {
        let (bw, bh) = (slot.body_w, slot.body_h);
        // 付属パーツの pt 値に掛ける倍率。
        let s = sq.scale;

        let root = CALayer::new();
        root.setFrame(rect(slot.hit_x, slot.hit_y, slot.hit_w, slot.hit_h));

        // 体のコンテナ。グリフ・バッジが外へはみ出すのでクリップしない。
        let face_layer = CALayer::new();
        face_layer.setFrame(rect(slot.body_x - slot.hit_x, slot.body_y, bw, bh));
        face_layer.setMasksToBounds(false);

        // --- 体の輪郭 ------------------------------------------------------
        // CAShapeLayer のストロークはパス上に中心が乗るので、枠線幅の半分だけ内側に
        // 縮めた輪郭を描く。こうしないと枠が体の矩形からはみ出し、群れの間隔が詰まって見える。
        let border_w = theme::BORDER_W * s;
        let inset = border_w / 2.0;
        let o = face.outline_of(bw - inset * 2.0, bh - inset * 2.0, size);
        let cg = path_from_outline(&o, (inset, inset));

        let body = CAShapeLayer::new();
        body.setFrame(rect(0.0, 0.0, bw, bh));
        body.setPath(Some(as_path(&cg)));
        body.setLineWidth(border_w);
        body.setShadowPath(Some(as_path(&cg) as &CGPath));
        body.setAllowsEdgeAntialiasing(true);
        face_layer.addSublayer(&body);

        // --- 内側グロー ----------------------------------------------------
        // CSS の `inset 0 0 9px -4px` に相当。体の形でマスクした上に太いストロークを
        // 置くと、外半分が切り取られて内側だけが滲む。
        let inner_host = CALayer::new();
        inner_host.setFrame(rect(0.0, 0.0, bw, bh));
        let mask = CAShapeLayer::new();
        mask.setFrame(rect(0.0, 0.0, bw, bh));
        mask.setPath(Some(as_path(&cg)));
        mask.setFillColor(Some(&cgcolor((1.0, 1.0, 1.0), 1.0)));
        // SAFETY: mask に CALayer サブクラスを渡すのは CALayer.mask の契約どおり。
        unsafe { inner_host.setMask(Some(&mask)) };

        let inner_stroke = CAShapeLayer::new();
        inner_stroke.setFrame(rect(0.0, 0.0, bw, bh));
        inner_stroke.setPath(Some(as_path(&cg)));
        // CAShapeLayer の fillColor は既定が黒。塗りたくないので明示的に消す。
        inner_stroke.setFillColor(None);
        inner_stroke.setLineWidth(theme::INNER_GLOW_W * 2.0 * s);
        inner_host.addSublayer(&inner_stroke);
        face_layer.addSublayer(&inner_host);

        // --- 顔のパネル線 ----------------------------------------------------
        // `[[details]]` を持つ顔だけが線画を出す。形は体の寸法だけで決まるので、一度組めば
        // あとは色を差し替えるだけでよい（`apply` でパスを触らない）。
        let details = CAShapeLayer::new();
        details.setFrame(rect(0.0, 0.0, bw, bh));
        details.setFillColor(None);
        details.setLineWidth(theme::detail_line_w(size) * s);
        let strokes = face.face_details(bw, bh, size);
        if strokes.is_empty() {
            details.setHidden(true);
        } else {
            details.setPath(Some(as_path(&path_from_strokes(&strokes))));
        }
        face_layer.addSublayer(&details);

        // --- 目 ------------------------------------------------------------
        let eye_l = CAShapeLayer::new();
        let eye_r = CAShapeLayer::new();
        face_layer.addSublayer(&eye_l);
        face_layer.addSublayer(&eye_r);

        // --- グリフ（右上） ------------------------------------------------
        let gfont = theme::glyph_font(size) * s;
        let (gox, goy) = theme::glyph_offset(size);
        let (gox, goy) = (gox * s, goy * s);
        let glyph = text_layer("", gfont, theme::EYE, 1.0, true, scale);
        center_text(&glyph);
        let gw = gfont * 1.4;
        let gh = gfont * 1.25;
        // 低いメニューバーではグリフを体の外へ浮かせると窓の上端で切れるので、
        // バッジと同じ要領で**体の右上に重ねる**（`layout::bar_fit` 段階 2 以降）。
        let (gx, gy) = if fit.glyph_inside {
            (bw + 3.0 * s - gw, bh - gh)
        } else {
            (bw + gox - gw, bh + goy - gh)
        };
        glyph.setFrame(rect(gx, gy, gw, gh));
        face_layer.addSublayer(&glyph);

        // --- バッジ（右下・エージェント数） --------------------------------
        let badge = solid_layer(theme::INK, 1.0, theme::badge_h(size) * s / 2.0);
        badge.setBorderWidth(1.0);
        let badge_text = text_layer("", theme::BADGE_FONT * s, theme::EYE, 1.0, true, scale);
        center_text(&badge_text);
        badge.addSublayer(&badge_text);
        face_layer.addSublayer(&badge);

        // --- 判断待ちの吹き出し（root 直下：face とは別に揺れる） ----------
        let bubble = CAShapeLayer::new();
        let bubble_text = text_layer(
            "!",
            theme::BUBBLE_FONT * s,
            theme::BUBBLE_INK,
            1.0,
            true,
            scale,
        );
        center_text(&bubble_text);
        bubble.addSublayer(&bubble_text);
        bubble.setHidden(true);
        root.addSublayer(&bubble);

        // --- アイドルの `z` ------------------------------------------------
        let zfont = theme::BUBBLE_FONT * s;
        let zmark = text_layer("z", zfont, theme::Z_COLOR, 1.0, true, scale);
        center_text(&zmark);
        let zw = zfont * 1.4;
        let zh = zfont * 1.25;
        zmark.setFrame(rect(
            (slot.body_x - slot.hit_x) + bw + 3.0 * s - zw,
            slot.body_y + bh + theme::bubble_top(size) * s - zh,
            zw,
            zh,
        ));
        zmark.setHidden(true);
        root.addSublayer(&zmark);

        root.addSublayer(&face_layer);

        // --- dock だけの名前と経過時間 -------------------------------------
        // コンパクト表示では出さない。縮小率を掛けると 8pt のラベルが 5pt 前後に
        // なって読めず、読めない文字列のために 2 段ぶんの高さを使うことになる
        // （`layout::lay_out` も `with_labels = false` で同じ判断をしている）。
        let (name, dur) = if size.is_bar() || sq.compact {
            (None, None)
        } else {
            let name_font = theme::NAME_FONT * s;
            let dur_font = theme::DUR_FONT * s;
            let name_gap = theme::NAME_GAP * s;
            let w = slot.hit_w.max(theme::NAME_MAX_W * s);
            let x = (slot.hit_w - w) / 2.0;

            let nh = name_font * 1.3;
            let n = text_layer("", name_font, theme::EYE, 1.0, true, scale);
            center_text(&n);
            truncate_end(&n);
            n.setFrame(rect(x, slot.body_y - name_gap - nh, w, nh));
            root.addSublayer(&n);

            let dh = dur_font * 1.3;
            let d = text_layer("", dur_font, theme::EYE, 0.8, false, scale);
            center_text(&d);
            d.setFrame(rect(x, slot.body_y - name_gap - nh - 2.0 * s - dh, w, dh));
            root.addSublayer(&d);

            (Some(n), Some(d))
        };

        Creature {
            root,
            face: face_layer,
            body,
            inner_stroke,
            _inner_mask: mask,
            details,
            eye_l,
            eye_r,
            glyph,
            badge,
            badge_text,
            bubble,
            bubble_text,
            zmark,
            name,
            dur,
            face_spec: face,
            size,
            fit,
            sq,
            body_w: bw,
            body_h: bh,
            last: RefCell::new(None),
        }
    }

    /// 群れのコンテナへ差し込むための root レイヤ。
    pub fn layer(&self) -> &CALayer {
        &self.root
    }

    /// 表示状態を反映する。前回と同じなら何もしない。
    ///
    /// 群れ全員のこれがポーリングのたびに呼ばれるので、**比較のために `View` を
    /// 複製しない**（文字列 2 本の確保が毎回 匹数ぶん走る）。
    pub fn apply(&self, v: &View, reduce_motion: bool) {
        // 借用はこのブロックで閉じる（末尾の `borrow_mut` と重なると実行時に落ちる）。
        let state_changed = {
            let last = self.last.borrow();
            match last.as_ref() {
                Some((prev, rm)) if prev == v && *rm == reduce_motion => return,
                Some((prev, rm)) => prev.state != v.state || *rm != reduce_motion,
                None => true,
            }
        };

        let accent = theme::accent(v.state);

        self.paint_body(v.state, accent);
        self.paint_eyes(v.state);
        self.paint_glyph(v, accent);
        self.paint_badge(v, accent);
        self.paint_flourishes(v.state, accent);
        self.paint_labels(v, accent);

        // 状態が変わったときだけアニメを貼り直す。同じ状態のまま経過時間だけ
        // 更新されるケース（10 秒ごとに起きる）で触ると、群れ全体の位相が揃ってしまう。
        if state_changed {
            self.reattach_animations(v.state, reduce_motion);
        }

        *self.last.borrow_mut() = Some((v.clone(), reduce_motion));
    }

    /// 体の塗り・枠・グローと、顔のパネル線。**形は `build` で決まっている**ので
    /// ここで触るのは色だけ。
    fn paint_body(&self, state: SessionState, accent: theme::Rgb) {
        let s = self.sq.scale;
        self.body
            .setFillColor(Some(&cgcolor(theme::face_fill(state), 1.0)));
        self.body.setStrokeColor(Some(&cgcolor(accent, 1.0)));
        set_glow(
            &self.body,
            accent,
            theme::GLOW_RADIUS * s,
            theme::GLOW_OPACITY,
        );
        self.inner_stroke
            .setStrokeColor(Some(&cgcolor(accent, theme::INNER_GLOW_ALPHA)));
        self.face.setOpacity(theme::face_opacity(state));

        self.details
            .setStrokeColor(Some(&cgcolor(accent, theme::DETAIL_ALPHA)));
        set_glow(
            &self.details,
            accent,
            theme::DETAIL_GLOW * s,
            theme::DETAIL_GLOW_OPACITY,
        );
    }

    /// 目。**顔が状態を表現できる唯一の場所**なので、寸法・色・グローが状態ごとに動く。
    fn paint_eyes(&self, state: SessionState) {
        let s = self.sq.scale;
        // `FaceSpec::eye` は体が等倍である前提の pt を返すので、コンパクト表示では
        // ここで同じ倍率を掛ける（掛けないと縮んだ体から目がはみ出す）。
        let e = self.face_spec.eye(state, self.size);
        let (ew, eh) = (e.w * s, e.h * s);
        let gap = self.face_spec.eye_gap(self.size) * s;
        let total = ew * 2.0 + gap;
        let left = (self.body_w - total) / 2.0 + e.dx * s;
        let y = (self.body_h - eh) / 2.0 + e.dy * s;
        for (i, eye) in [&self.eye_l, &self.eye_r].into_iter().enumerate() {
            eye.setFrame(rect(left + (ew + gap) * i as f64, y, ew, eh));
            // 多角形の目（スリットやくさび）はパスで、それ以外は角丸矩形で描く。
            // 状態ごとに目の寸法が変わるので、スリットのパスはここで組み直す。
            match self.face_spec.eye_shape(ew, eh, i == 0) {
                Some(pts) => {
                    eye.setPath(Some(as_path(&path_from_polygon(&pts))));
                    eye.setFillColor(Some(&cgcolor(e.color, 1.0)));
                    eye.setBackgroundColor(None);
                }
                None => {
                    eye.setCornerRadius(e.radius * s);
                    eye.setBackgroundColor(Some(&cgcolor(e.color, 1.0)));
                }
            }
            if e.glow > 0.0 {
                set_glow(eye, (1.0, 1.0, 1.0), e.glow * s, 0.9);
            } else {
                eye.setShadowOpacity(0.0);
            }
        }
    }

    /// 状態記号（`›` `!` `⋯` `z` `✓` `×`）。
    ///
    /// 隠すだけで中身は更新しておく（次に出すときに 1 フレーム古い記号が
    /// 見えないように）。
    fn paint_glyph(&self, v: &View, accent: theme::Rgb) {
        self.glyph.setHidden(!v.show_glyph);
        set_text(&self.glyph, v.state.glyph());
        set_text_color(&self.glyph, accent, 1.0);
        set_glow(&self.glyph, accent, 4.0 * self.sq.scale, 0.9);
    }

    fn paint_badge(&self, v: &View, accent: theme::Rgb) {
        let s = self.sq.scale;
        // エージェントが 0 のときは出さない（元デザインは常時 1 以上を想定しているが、
        // 実運用では「エージェントなしで自走中」が普通なので、0 は非表示が素直）。
        //
        // **コンパクト表示でも出さない**。9pt の数字は縮めると読めず、読めない数字は
        // 体の右下を汚すだけになる。エージェント数はホバーカードで見られる
        // （狭いところでは要素を減らして本体に集約する、という bar と同じ判断）。
        if v.agents == 0 || self.sq.compact {
            self.badge.setHidden(true);
        } else {
            self.badge.setHidden(false);
            let label = v.agents.to_string();
            set_text(&self.badge_text, &label);
            let bh = theme::badge_h(self.size) * s;
            let (box_, boy) = theme::badge_offset(self.size);
            let (box_, boy) = (box_ * s, boy * s);
            let badge_font = theme::BADGE_FONT * s;
            let bwid =
                (text_width(&label, badge_font) + 6.0 * s).max(theme::badge_min_w(self.size) * s);
            self.badge
                .setFrame(rect(self.body_w + box_ - bwid, -boy, bwid, bh));
            self.badge.setBorderColor(Some(&cgcolor(accent, 1.0)));
            let th = badge_font * 1.3;
            self.badge_text
                .setFrame(rect(0.0, (bh - th) / 2.0 - 0.5, bwid, th));
        }
    }

    /// 判断待ちの吹き出しとアイドルの `z`。
    ///
    /// **bar では出さない。** 帯の高さに収める必要があり、どちらもグリフと同じ情報を
    /// 持つので、狭いところではグリフ 1 個に集約する（`has_flourishes`）。
    fn paint_flourishes(&self, state: SessionState, accent: theme::Rgb) {
        let flourishes = theme::has_flourishes(self.size);
        let show_bubble = flourishes && state == SessionState::WaitUser;
        self.bubble.setHidden(!show_bubble);
        if show_bubble {
            self.layout_bubble(accent);
        }
        self.zmark
            .setHidden(!flourishes || state != SessionState::Idle);
    }

    /// dock で体の下に出す名前と経過時間（bar とコンパクト表示では存在しない）。
    fn paint_labels(&self, v: &View, accent: theme::Rgb) {
        if let Some(n) = &self.name {
            set_text(n, &v.short);
            set_text_color(n, accent, 1.0);
        }
        if let Some(d) = &self.dur {
            set_text(d, &v.dur);
            set_text_color(d, accent, 0.8);
        }
    }

    /// 吹き出しの形と位置を組む。CSS の `border-radius: 6px 6px 6px 2px`
    /// （左下だけ尖る吹き出し）を輪郭ベジェで再現する。
    fn layout_bubble(&self, accent: theme::Rgb) {
        let s = self.sq.scale;
        let font = theme::BUBBLE_FONT * s;
        let w = (text_width("!", font) + 8.0 * s).max(theme::BUBBLE_MIN_W * s);
        let h = theme::BUBBLE_H * s;
        // root 座標。体の水平中心に合わせ、体の上端から bubble_top だけ上へ。
        let face_frame = self.face.frame();
        let x = face_frame.origin.x + (self.body_w - w) / 2.0;
        let y = face_frame.origin.y + self.body_h + theme::bubble_top(self.size) * s - h;
        self.bubble.setFrame(rect(x, y, w, h));

        // CSS の順は 左上・右上・右下・左下。左下だけ 2px で尖らせる。
        let corners = [
            (6.0 * s, 6.0 * s),
            (6.0 * s, 6.0 * s),
            (6.0 * s, 6.0 * s),
            (2.0 * s, 2.0 * s),
        ];
        let o = theme::outline(w, h, corners);
        let p = path_from_outline(&o, (0.0, 0.0));
        self.bubble.setPath(Some(as_path(&p)));
        self.bubble.setFillColor(Some(&cgcolor(accent, 1.0)));
        set_glow(&self.bubble, accent, 5.0 * s, 0.9);

        let th = font * 1.3;
        self.bubble_text
            .setFrame(rect(0.0, (h - th) / 2.0 - 0.5, w, th));
    }

    /// 状態に対応するアニメを貼り直す。`reduce_motion` なら全部剥がす。
    fn reattach_animations(&self, state: SessionState, reduce_motion: bool) {
        for l in [
            &self.face,
            self.body.as_ref() as &CALayer,
            self.eye_l.as_ref() as &CALayer,
            self.eye_r.as_ref() as &CALayer,
            self.bubble.as_ref() as &CALayer,
            self.zmark.as_ref() as &CALayer,
        ] {
            anim::clear(l);
        }
        // 剥がしたあと、掛かっていた変形が残らないよう明示的に等倍へ戻す。
        self.face.setOpacity(theme::face_opacity(state));

        if reduce_motion {
            return;
        }

        // 振れ幅は「上へ飛び出せる余地」と「群れの縮小率」の両方で絞る。
        // 縮小率を掛けないと、縮んだ体が等倍のまま跳ねて縮んだ余白を突き抜ける
        // （`layout::a_compact_flock_still_fits_inside_every_supported_menu_bar` が番人）。
        let anim_scale = self.fit.anim_scale * self.sq.scale;
        match theme::face_anim(state, self.size, anim_scale) {
            FaceAnim::Bob { amp, half_secs } => anim::bob(&self.face, amp, half_secs),
            FaceAnim::Drift { amp, half_secs } => anim::drift(&self.face, amp, half_secs),
            FaceAnim::Hop { keys, values, secs } => anim::hop(&self.face, &keys, &values, secs),
            FaceAnim::Breath {
                opacity,
                glow,
                half_secs,
            } => anim::breath(&self.face, &self.body, opacity, glow, half_secs),
            FaceAnim::None => {}
        }

        if self.face_spec.eye(state, self.size).blink {
            anim::blink(
                &self.eye_l,
                &theme::BLINK_KEYS,
                &theme::BLINK_VALUES,
                theme::BLINK_SECS,
            );
            anim::blink(
                &self.eye_r,
                &theme::BLINK_KEYS,
                &theme::BLINK_VALUES,
                theme::BLINK_SECS,
            );
        }
        let flourishes = theme::has_flourishes(self.size);
        if flourishes && state == SessionState::WaitUser {
            anim::bubble_pop(
                &self.bubble,
                theme::BUBBLE_POP_AMP * self.sq.scale,
                theme::BUBBLE_POP_HALF_SECS,
            );
        }
        if flourishes && state == SessionState::Idle {
            let (zx, zy) = theme::FLOAT_Z_TO;
            anim::float_z(
                &self.zmark,
                (zx * self.sq.scale, zy * self.sq.scale),
                theme::FLOAT_Z_SCALE,
                &theme::FLOAT_Z_OPACITY_KEYS,
                &theme::FLOAT_Z_OPACITY,
                theme::FLOAT_Z_SECS,
            );
        }
    }
}
