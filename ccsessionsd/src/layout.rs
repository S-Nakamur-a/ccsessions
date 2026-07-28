//! 群れ（複数の生き物）の並べ方と、それを収める窓の大きさの算術。
//!
//! `geometry.rs` と同じ理由で **純関数だけ**。窓サイズは「生き物の体の合計」ではなく
//! 「はみ出すパーツ（グリフ・バッジ・吹き出し・名前）まで含めた実占有」で決める必要があり、
//! ここを間違えるとパーツがウィンドウ境界で切れる（透明窓は境界でクリップされる）。

use crate::theme::{self, Size};
use ccsessions_core::config::CompactMode;
use ccsessions_core::face::FaceSpec;

// ---------------------------------------------------------------------------
// メニューバー高への適応
// ---------------------------------------------------------------------------

/// メニューバー高に合わせた bar 配置の詰め方。
///
/// **なぜ要るか**: メニューバー高は機種依存で、ノッチ機は 33pt だが**非ノッチ画面
/// （外部モニタ・Air・旧機種）は 24pt しかない**。窓は `geometry::bar_rect` が
/// メニューバー高でクランプするので「帯の外のクリックを奪わない」は守られるが、
/// **中身のレイアウトを 32pt 固定にするとグリフが窓の上端で切れる**（透明窓は
/// 境界でクリップされる）。
///
/// `NSScreen::mainScreen` はフォーカス追従なので、外部モニタにフォーカスを移すだけで
/// 再現する。「他人の Mac の問題」ではない。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BarFit {
    /// 体の上に確保する余白（pt）。0 ならグリフを外に出せない。
    pub headroom: f64,
    /// グリフを体の外（右上に浮かせる）ではなく、**体に重ねて**描くか。
    ///
    /// 狭い帯では要素を体に集約するのがこの設計の一貫した判断で、
    /// バッジが既に同じ手を採っている（`badge_offset(Bar)` の下方向が 0）。
    pub glyph_inside: bool,
    /// 体そのものの倍率。1.0 が既定。**帯が体より低いときだけ 1 未満**になる。
    pub body_scale: f64,
    /// アニメの振れ幅の倍率。上に飛び出せる余地に応じて絞る。
    pub anim_scale: f64,
}

impl BarFit {
    /// dock 配置と、メニューバー高を気にしなくてよい場面で使う既定値。
    pub const ROOMY: BarFit = BarFit {
        headroom: theme::BAR_HEADROOM,
        glyph_inside: false,
        body_scale: 1.0,
        anim_scale: 1.0,
    };
}

/// メニューバー高が測れないときに使う既定値（pt）。
///
/// `menu_bar_height()` は `(frame.max_y - visible.max_y).max(safe_area_top)` なので、
/// **非ノッチ画面でメニューバーが自動非表示だと 0 になる**。そのまま使うと
/// `bar_rect` のクランプが効かず、帯が 32pt のまま画面最上端に置かれる。そこは
/// メニューバーではなくアプリのコンテンツ領域なので、その矩形ぶんのクリックを
/// 奪ってしまう。Big Sur 以降の標準的な高さへ倒して防ぐ。
pub const FALLBACK_MENU_BAR_H: f64 = 24.0;

/// 体の上下に最低限残す余白（pt）。これを割ると体そのものを縮める。
const MIN_BREATHING_ROOM: f64 = 2.0;

/// メニューバー高と体の高さから詰め方を決める。**純関数**。
///
/// 3 段階:
///
/// | `menu_bar_h` | 段階 | 結果 |
/// |---|---|---|
/// | `>= body_h + BAR_HEADROOM` | 1 | 現状どおり（グリフを体の外へ浮かせる） |
/// | `body_h + 2` .. | 2 | グリフを体に重ね、`headroom` を残りぶんに詰める |
/// | それ未満 | 3 | 体そのものを縮める（想定外に低い帯への保険） |
///
/// 段階 1 は**既存の見た目と完全に同一**でなければならない
/// （`bar_fit_at_a_notched_menu_bar_keeps_the_current_look` が番人）。
pub fn bar_fit(menu_bar_h: f64, body_h: f64) -> BarFit {
    let avail = if menu_bar_h > 0.0 {
        menu_bar_h
    } else {
        FALLBACK_MENU_BAR_H
    };

    // 段階 1: グリフを体の外へ浮かせる余地がある。
    if avail >= body_h + theme::BAR_HEADROOM {
        return BarFit::ROOMY;
    }

    // 段階 3: 体すら入らない。体を縮めてから段階 2 と同じ扱いにする。
    let (body_scale, effective_h) = if avail < body_h + MIN_BREATHING_ROOM {
        let scaled = (avail - MIN_BREATHING_ROOM).max(1.0);
        (scaled / body_h, scaled)
    } else {
        (1.0, body_h)
    };

    // 段階 2: グリフを体に重ね、残りを headroom にする。
    let headroom = (avail - effective_h).max(0.0);
    BarFit {
        headroom,
        glyph_inside: true,
        body_scale,
        // 上に飛び出せる余地に応じてアニメの振れ幅を絞る。
        //
        // **分母は縦アニメの最大振幅であって `BAR_HEADROOM` ではない**。段階 2 では
        // グリフを体に重ねてあるので、`headroom` は丸ごとアニメが使える。つまり
        // 「4pt 空いていれば 4pt 跳べる」が正しい対応で、そのとき 24pt でも倍率は
        // 1.0 ＝ 見た目の劣化が無い。`BAR_HEADROOM`(12) で割ると 24pt で 1/3 になり、
        // bob の振幅が 0.67pt ＝事実上の静止になってしまう（絞る目的は「窓の上端で
        // 切らないこと」であって、動きを消すことではない）。
        // 番人は `the_glyph_box_stays_inside_the_window_even_mid_animation`。
        anim_scale: (headroom / theme::BAR_MAX_ANIM_AMP).clamp(0.0, 1.0),
    }
}

// ---------------------------------------------------------------------------
// 使える幅への適応（コンパクト表示）
// ---------------------------------------------------------------------------

/// 群れをどれだけ縮めるか。
///
/// **縮め方は「一様な縮小」だけ**にしてある。体だけを縮めて間隔や目・グリフを
/// 据え置くと顔の比率が崩れるうえ、`bar_fit` が縦方向で保証している不変条件
/// （グリフがアニメの頂点でも窓に収まる）が成り立たなくなる。全部に同じ倍率を
/// 掛けるなら、収まっているレイアウトを縮めた結果もまた収まる — 不等式が
/// 両辺とも `scale` 倍されるだけなので、番人のテストが自動的に効き続ける。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Squeeze {
    /// 体・間隔・余白・付属パーツすべてに掛かる倍率。1.0 が既定。
    pub scale: f64,
    /// コンパクト表示か。補助情報（エージェント数バッジ・dock の名前と経過時間）を
    /// 省く。**倍率とは別に持つ** — `always` では倍率が控えめでも補助情報は省くため。
    pub compact: bool,
}

impl Squeeze {
    /// 縮めない（`compact_flock = "never"` と、収まっているときの `auto`）。
    pub const NONE: Squeeze = Squeeze {
        scale: 1.0,
        compact: false,
    };
}

/// コンパクト表示の基準倍率。
///
/// `always` はこの倍率で固定し、`auto` は「まずこれ、それでも入らなければさらに縮める」。
/// 0.75 は 22pt の体が 16.5pt になる値で、目（3×4pt → 2.25×3pt）がまだ 2 つに
/// 見分けられる下限に近い。
pub const COMPACT_SCALE: f64 = 0.75;

/// これ以上は縮めない下限。
///
/// ここまで縮めても入らないなら**縮小を諦める**（`bar_align` の退避に任せる）。
/// 判読できない粒まで縮めるくらいなら、ノッチの左へ逃げて大きく出す方がまし。
/// 0.55 は egg の体が 12.1pt、目が 1.65×2.2pt で、色と形がぎりぎり読める限界。
pub const MIN_COMPACT_SCALE: f64 = 0.55;

/// **使える幅が測れないとき**に、この匹数を超えたらコンパクトへ切り替える。
///
/// 幅で判断する方が正確なので、そちらが取れるならこの定数は使わない
/// （`bar_fit` の `FALLBACK_MENU_BAR_H` と同じ位置づけ）。6 匹はノッチ右の
/// 空き帯（`geometry` の実測 225pt）に入る最大匹数。
pub const COMPACT_AFTER: usize = 6;

/// 群れの組み方一式。縦（`bar_fit`）と横（コンパクト表示）をまとめて持つ。
///
/// `Flock` はこれを保持し、匹数が変わるたびに `squeeze` を解き直す
/// （縮小率は匹数に依存するため、`BarFit` のように 1 つの値では持てない）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Packing {
    /// メニューバー高への適応（縦）。
    pub fit: BarFit,
    /// 群れに使える水平幅（pt）。測れなければ `None` で、匹数での判断に落ちる。
    pub avail_w: Option<f64>,
    pub compact: CompactMode,
}

impl Packing {
    /// この匹数での縮小率を決める。
    pub fn squeeze_for(&self, count: usize, face: &FaceSpec, size: Size) -> Squeeze {
        squeeze(
            count,
            natural_width(count, face, size, self.fit),
            self.avail_w,
            self.compact,
        )
    }
}

/// 縮める前（倍率 1.0）の群れの幅。`squeeze` がこれと使える幅を比べる。
pub fn natural_width(count: usize, face: &FaceSpec, size: Size, fit: BarFit) -> f64 {
    lay_out(count, face, size, fit, Squeeze::NONE).width
}

/// 縮小率を決める。**純関数**。
///
/// 3 段階（`bar_fit` と同じ構造）:
///
/// | 条件 | 段階 | 結果 |
/// |---|---|---|
/// | `never`、または `auto` で収まっている | 1 | `Squeeze::NONE`（今までどおり） |
/// | 収まらない（または `always`） | 2 | `COMPACT_SCALE` へ縮める |
/// | それでも収まらない | 3 | 収まる倍率まで縮める（`MIN_COMPACT_SCALE` で打ち止め） |
///
/// 幅は一様な倍率に比例する（`lay_out` が全部に同じ倍率を掛ける）ので、
/// 収まる倍率は `avail_w / natural_w` そのもの。
pub fn squeeze(count: usize, natural_w: f64, avail_w: Option<f64>, mode: CompactMode) -> Squeeze {
    if count == 0 || mode == CompactMode::Never {
        return Squeeze::NONE;
    }
    // 段階 1: `auto` で収まっているなら何もしない。
    if mode == CompactMode::Auto && fits(count, natural_w, avail_w) {
        return Squeeze::NONE;
    }
    // 段階 2・3: 基準倍率から始め、それでも入らなければ入る倍率まで詰める。
    let to_fit = match avail_w {
        Some(w) if natural_w > 0.0 => w / natural_w,
        // 幅が測れないときは基準倍率で止める（どこまで縮めれば足りるか分からない）。
        _ => COMPACT_SCALE,
    };
    Squeeze {
        scale: to_fit
            .min(COMPACT_SCALE)
            .clamp(MIN_COMPACT_SCALE, COMPACT_SCALE),
        compact: true,
    }
}

/// 縮めずに収まるか。幅が測れれば幅で、測れなければ匹数で判断する。
fn fits(count: usize, natural_w: f64, avail_w: Option<f64>) -> bool {
    match avail_w {
        Some(w) => natural_w <= w,
        None => count <= COMPACT_AFTER,
    }
}

/// 生き物 1 匹ぶんのスロット。座標は窓ローカル（左下原点・y 上向き）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Slot {
    /// 体の矩形（グリフ・バッジは含まない）。
    pub body_x: f64,
    pub body_y: f64,
    pub body_w: f64,
    pub body_h: f64,
    /// ホバー判定に使う矩形。体より広めに取り、隣と重ならない範囲で当たりを大きくする。
    pub hit_x: f64,
    pub hit_y: f64,
    pub hit_w: f64,
    pub hit_h: f64,
}

/// 群れ全体のレイアウト結果。
#[derive(Debug, Clone, PartialEq)]
pub struct Layout {
    /// 窓の内容サイズ（この大きさの窓を作る）。
    pub width: f64,
    pub height: f64,
    pub slots: Vec<Slot>,
    /// この配置に適用した縮小。`creature` が付属パーツの pt 値に同じ倍率を掛ける。
    pub squeeze: Squeeze,
}

/// 体からはみ出すパーツのぶんだけ確保する余白（pt）。返り値は (左, 右, 上, 下)。
///
/// - 右: グリフのはみ出しと、バッジのはみ出し＋幅の半分の大きい方
/// - 上: bar は `BAR_HEADROOM` 固定（メニューバーに収める制約が先にある）。
///   dock はグリフと吹き出しの大きい方。
/// - 下: バッジのはみ出し。dock ではさらに名前＋経過時間の 2 段ぶん
/// - 左: 右と同じだけ取る。`z` は右上にしか出ないので厳密には不要だが、
///   左右非対称にすると群れの間隔が不揃いに見えるので揃える
fn margins(size: Size, with_labels: bool, fit: BarFit) -> (f64, f64, f64, f64) {
    let (gx, gy) = theme::glyph_offset(size);
    let (bx, by) = theme::badge_offset(size);

    let right = gx.max(bx + theme::badge_min_w(size) / 2.0);
    let top = if size.is_bar() {
        // メニューバー高に合わせて詰める（段階 2 以降はグリフを体に重ねるので
        // ここが `BAR_HEADROOM` より小さくなる）。
        fit.headroom
    } else {
        gy.max(theme::bubble_top(size) + theme::BUBBLE_H - 6.0)
    };
    // bar ではバッジが体に重なる（`badge_offset` の下方向が 0）ので下余白は要らない。
    // ここを 0 にして初めて 体 20 + 上 12 = 32pt がメニューバー 33pt に収まる。
    let mut bottom = if size.is_bar() {
        0.0
    } else {
        by + theme::badge_h(size) / 2.0
    };
    if with_labels {
        // dock は体の下に「名前」と「経過時間」の 2 段が付く。
        bottom += theme::NAME_GAP + theme::NAME_FONT + 2.0 + theme::DUR_FONT;
    }
    (right, right, top, bottom)
}

/// 群れを横一列に並べる。
///
/// `count` が 0 のときも 1x1 の窓を返す（tao の窓はサイズ 0 を受け付けないため）。
/// 呼び出し側は `count == 0` なら窓自体を隠す。
///
/// `sq` はコンパクト表示の縮小。**体・間隔・余白のすべてに同じ倍率が掛かる**ので、
/// 縮めても縦の不変条件（メニューバーに収まる／グリフが切れない）は保たれる。
pub fn lay_out(count: usize, face: &FaceSpec, size: Size, fit: BarFit, sq: Squeeze) -> Layout {
    let (bw, bh) = face.body_size(size);
    // 帯が体より低い環境（段階 3）では体そのものを縮める。
    let (bw, bh) = if size.is_bar() {
        (bw * fit.body_scale, bh * fit.body_scale)
    } else {
        (bw, bh)
    };
    // コンパクト表示はその上から群れ全体を一様に縮める。
    let (bw, bh) = (bw * sq.scale, bh * sq.scale);
    let gap = theme::flock_gap(size) * sq.scale;
    // コンパクトでは dock の名前・経過時間を出さないので、その 2 段ぶんの余白も要らない。
    let with_labels = !size.is_bar() && !sq.compact;
    let (ml, mr, mt, mb) = margins(size, with_labels, fit);
    let (ml, mr, mt, mb) = (ml * sq.scale, mr * sq.scale, mt * sq.scale, mb * sq.scale);

    if count == 0 {
        return Layout {
            width: 1.0,
            height: 1.0,
            slots: Vec::new(),
            squeeze: sq,
        };
    }

    let n = count as f64;
    let width = ml + bw * n + gap * (n - 1.0) + mr;
    let height = mb + bh + mt;

    let mut slots = Vec::with_capacity(count);
    for i in 0..count {
        let body_x = ml + (bw + gap) * i as f64;
        let body_y = mb;
        // 当たり判定は体 + 左右それぞれ gap の半分まで（隣と重ならない最大）。
        let pad = gap / 2.0;
        slots.push(Slot {
            body_x,
            body_y,
            body_w: bw,
            body_h: bh,
            hit_x: body_x - pad,
            hit_y: 0.0,
            hit_w: bw + pad * 2.0,
            hit_h: height,
        });
    }

    Layout {
        width,
        height,
        slots,
        squeeze: sq,
    }
}

/// dock パネル（背景の角丸カード）の矩形を、群れのレイアウトから求める。
/// 元デザインの `padding:13px 19px 11px` を反映する。
pub fn dock_panel(layout: &Layout) -> (f64, f64) {
    (
        layout.width + theme::DOCK_PAD_X * 2.0,
        layout.height + theme::DOCK_PAD_TOP + theme::DOCK_PAD_BOTTOM,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use ccsessions_core::face::Registry;

    /// テスト用に組込み顔を 1 つ借りる。
    fn face(id: &str) -> std::sync::Arc<FaceSpec> {
        Registry::builtin()
            .get(id)
            .unwrap_or_else(|| panic!("{id} が無い"))
            .clone()
    }

    /// bar のレイアウトを「現状どおり」の詰め方で組む（多くのテストの既定）。
    fn bar(count: usize, id: &str) -> Layout {
        lay_out(count, &face(id), Size::Bar, BarFit::ROOMY, Squeeze::NONE)
    }

    #[test]
    fn empty_flock_is_a_minimal_window() {
        let l = bar(0, "egg");
        assert!(l.slots.is_empty());
        assert_eq!((l.width, l.height), (1.0, 1.0));
    }

    /// 体・間隔・余白の合計が窓幅になり、最後のスロットが右余白の内側に収まる。
    #[test]
    fn width_accounts_for_bodies_gaps_and_margins() {
        let l = bar(3, "egg");
        let (bw, _) = face("egg").body_size(Size::Bar);
        let gap = theme::flock_gap(Size::Bar);
        let (ml, mr, _, _) = margins(Size::Bar, false, BarFit::ROOMY);
        assert_eq!(l.width, ml + bw * 3.0 + gap * 2.0 + mr);
        let last = l.slots.last().unwrap();
        assert!(last.body_x + last.body_w <= l.width - mr + 0.001);
    }

    /// 体は窓の内側に収まり、上にはグリフ／吹き出しぶんの余白がある。
    /// bar はバッジを体に重ねるので下余白は 0、dock は名前 2 段ぶん空く。
    #[test]
    fn body_fits_within_vertical_margins() {
        let egg = face("egg");
        for size in [Size::Bar, Size::Dock] {
            let l = lay_out(2, &egg, size, BarFit::ROOMY, Squeeze::NONE);
            let s = l.slots[0];
            assert!(s.body_y >= 0.0, "体が窓の下端より下にある");
            assert!(s.body_y + s.body_h < l.height, "上余白が無い");
        }
        assert_eq!(bar(2, "egg").slots[0].body_y, 0.0);
        assert!(lay_out(2, &egg, Size::Dock, BarFit::ROOMY, Squeeze::NONE).slots[0].body_y > 0.0);
    }

    // ---- bar_fit（メニューバー高への適応）---------------------------------

    /// **ノッチ機（33pt）では今までと完全に同じ。**
    ///
    /// `bar_fit` を入れたことで既存の見た目が変わっていないことの番人。
    /// ここが落ちたら、詰め方の配線をやめて元へ戻すこと。
    #[test]
    fn bar_fit_at_a_notched_menu_bar_keeps_the_current_look() {
        let fit = bar_fit(33.0, 20.0);
        assert_eq!(fit, BarFit::ROOMY);
        assert_eq!(fit.headroom, theme::BAR_HEADROOM);
        assert!(!fit.glyph_inside, "ノッチ機ではグリフを外へ浮かせたまま");
        assert_eq!(fit.body_scale, 1.0, "体を縮めない");
        assert_eq!(fit.anim_scale, 1.0, "アニメの振れ幅を絞らない");

        // 組込み顔すべてで段階 1 のまま。
        for f in Registry::builtin().all() {
            let (_, h) = f.body_size(Size::Bar);
            assert_eq!(bar_fit(33.0, h), BarFit::ROOMY, "{} が段階 1 でない", f.id);
        }
    }

    /// **非ノッチ（24pt）ではグリフを体に重ね、headroom は 4pt。**
    #[test]
    fn bar_fit_at_a_plain_menu_bar_tucks_the_glyph_inside() {
        let fit = bar_fit(24.0, 20.0);
        assert!(fit.glyph_inside, "グリフを体に重ねていない");
        assert_eq!(fit.headroom, 4.0);
        assert_eq!(fit.body_scale, 1.0, "体はまだ縮めない");
        // グリフを体に重ねた結果、余地 4pt が丸ごと縦アニメに使える。跳ぶ量も 4pt
        // なのでちょうど収まり、**振れ幅は絞らない**（絞る必要が無い）。
        assert_eq!(
            fit.anim_scale, 1.0,
            "24pt でアニメを絞る必要は無い（絞ると bob が静止して見える）"
        );
    }

    /// **アニメを絞るのは帯が 24pt より低いときだけ**で、絞り方は
    /// 「跳べる量が余地に一致する」ように決まる。
    ///
    /// `anim_scale` の分母を `BAR_HEADROOM`(12) に戻すと 24pt が 1/3 になるので、
    /// ここが最初に落ちる。
    #[test]
    fn the_animation_is_scaled_to_exactly_the_room_above_the_body() {
        // 余地 4pt = 跳ぶ量 4pt → 等倍。
        assert_eq!(bar_fit(24.0, 20.0).anim_scale, 1.0);
        // 余地 2pt → 半分だけ跳ぶ。
        assert_eq!(bar_fit(22.0, 20.0).anim_scale, 0.5);
        // 余地が最大振幅を超えても 1.0 を上限にする（段階 1 と同じ見た目）。
        assert_eq!(bar_fit(30.0, 20.0).anim_scale, 1.0);

        // 絞ったあとの跳躍が余地を超えない、が本質。
        for menu_bar_h in [18.0_f64, 20.0, 22.0, 24.0, 26.0] {
            let fit = bar_fit(menu_bar_h, 20.0);
            assert!(
                theme::BAR_MAX_ANIM_AMP * fit.anim_scale <= fit.headroom + 1e-9,
                "{menu_bar_h}pt で跳躍 {:.2} が余地 {:.2} を超える",
                theme::BAR_MAX_ANIM_AMP * fit.anim_scale,
                fit.headroom
            );
        }
    }

    /// `BAR_HEADROOM` の内訳（グリフを浮かせる量 ＋ 縦アニメの予算）を固定する。
    ///
    /// この等式が崩れると、段階 1（ノッチ機）で hop の頂点がグリフを窓の外へ押し出す。
    /// `face_anim` の hop の頂点が `BAR_MAX_ANIM_AMP` であることも併せて押さえる。
    #[test]
    fn bar_headroom_is_the_glyph_overhang_plus_the_animation_budget() {
        let (_, goy) = theme::glyph_offset(Size::Bar);
        assert_eq!(goy + theme::BAR_MAX_ANIM_AMP, theme::BAR_HEADROOM);

        // bar の縦アニメで最も高く上がるのは判断待ちの hop。
        let peak = match theme::face_anim(
            ccsessions_core::session::SessionState::WaitUser,
            Size::Bar,
            1.0,
        ) {
            theme::FaceAnim::Hop { values, .. } => values.iter().copied().fold(0.0, f64::max),
            other => panic!("判断待ちが Hop でなくなった: {other:?}"),
        };
        assert_eq!(peak, theme::BAR_MAX_ANIM_AMP);
    }

    /// **全顔が、サポートする全メニューバー高に収まる。**
    ///
    /// 帯がメニューバーを超えると、その矩形ぶんメニューバー下のクリックを奪う。
    /// **顔をデータで足せるようにした以上、この番人はレジストリ全体をループ
    /// しなければ意味がない**（旧実装はデザイン名のハードコード配列だった）。
    ///
    /// 33pt = ノッチ機、24pt = 非ノッチ画面（外部モニタ・Air・旧機種）。
    #[test]
    fn every_face_fits_inside_every_supported_menu_bar() {
        for &menu_bar_h in &[24.0_f64, 33.0] {
            for f in Registry::builtin().all() {
                let (_, body_h) = f.body_size(Size::Bar);
                let fit = bar_fit(menu_bar_h, body_h);
                let l = lay_out(6, f, Size::Bar, fit, Squeeze::NONE);
                assert!(
                    l.height <= menu_bar_h + 1e-9,
                    "{} の bar レイアウトが {:.1}pt でメニューバー {menu_bar_h}pt を超える",
                    f.id,
                    l.height
                );
                // 組込み顔は体を縮められずに収まること（縮むと生き物が小さくなる）。
                assert_eq!(
                    fit.body_scale, 1.0,
                    "{} が {menu_bar_h}pt で体を縮められている",
                    f.id
                );
            }
        }
    }

    /// バリデータの上限ぎりぎり（22pt）の顔でも、どちらのメニューバーにも収まる。
    ///
    /// `validate::MAX_BAR_BODY_H` がこの数値の根拠であることの固定。
    #[test]
    fn a_face_at_the_validator_height_limit_still_fits() {
        for &menu_bar_h in &[24.0_f64, 33.0] {
            let fit = bar_fit(menu_bar_h, ccsessions_core::face::validate::MAX_BAR_BODY_H);
            let height =
                ccsessions_core::face::validate::MAX_BAR_BODY_H * fit.body_scale + fit.headroom;
            assert!(
                height <= menu_bar_h + 1e-9,
                "上限の顔が {menu_bar_h}pt に収まらない: {height}"
            );
            assert_eq!(fit.body_scale, 1.0, "上限の顔で体が縮んでいる");
        }
    }

    /// メニューバーが測れない（0）ときは既定値へ倒す。
    #[test]
    fn an_unmeasurable_menu_bar_falls_back_to_a_sane_height() {
        let fit = bar_fit(0.0, 20.0);
        assert_eq!(fit, bar_fit(FALLBACK_MENU_BAR_H, 20.0));
        let l = lay_out(6, &face("egg"), Size::Bar, fit, Squeeze::NONE);
        assert!(l.height <= FALLBACK_MENU_BAR_H, "既定値にも収まっていない");
    }

    /// 帯が体より低い極端な環境では体そのものを縮める（段階 3）。
    #[test]
    fn a_menu_bar_shorter_than_the_body_shrinks_it() {
        let fit = bar_fit(16.0, 20.0);
        assert!(fit.body_scale < 1.0, "体を縮めていない");
        let l = lay_out(3, &face("egg"), Size::Bar, fit, Squeeze::NONE);
        assert!(
            l.height <= 16.0 + 1e-9,
            "縮めても収まっていない: {}",
            l.height
        );
    }

    /// **グリフの箱が、アニメで跳ねた最大位置でも窓に収まる。**
    ///
    /// レイアウトの検査は体の矩形しか見ないので、「体は収まっているのに
    /// グリフだけ切れる」はここでしか捕まえられない。
    #[test]
    fn the_glyph_box_stays_inside_the_window_even_mid_animation() {
        for &menu_bar_h in &[24.0_f64, 33.0] {
            for f in Registry::builtin().all() {
                let (_, body_h) = f.body_size(Size::Bar);
                let fit = bar_fit(menu_bar_h, body_h);
                let l = lay_out(1, f, Size::Bar, fit, Squeeze::NONE);
                let s = l.slots[0];

                // `creature.rs` と同じ式でグリフの箱の上端を求める。
                let gfont = theme::glyph_font(Size::Bar);
                let (_, goy) = theme::glyph_offset(Size::Bar);
                let glyph_top = if fit.glyph_inside {
                    // 体に重ねるので体の上端を超えない。
                    s.body_y + s.body_h
                } else {
                    s.body_y + s.body_h + goy
                };
                // 最も跳ねる状態（判断待ちの hop）の振れ幅を足す。
                // `BAR_MAX_ANIM_AMP` が実際の hop の頂点と一致することは
                // `bar_headroom_is_the_glyph_overhang_plus_the_animation_budget` が押さえる。
                let hop = theme::BAR_MAX_ANIM_AMP * fit.anim_scale;
                let top = glyph_top + hop;
                assert!(
                    top <= l.height + 1e-9,
                    "{} が {menu_bar_h}pt でグリフを切る: 上端 {top:.1} > 窓 {:.1}（font {gfont}）",
                    f.id,
                    l.height
                );
            }
        }
    }

    // ---- squeeze（使える幅への適応 = コンパクト表示）----------------------

    /// `bar` レイアウトの「使える幅」の目安。ノッチ右の空き帯（実測 225pt）から
    /// ノッチとの隙間を引いた値で、`geometry::bar_available_width` が返すのと同じ。
    const RIGHT_GAP: f64 = 217.0;

    /// ノッチ右の空き帯を使える幅として、指定のモードで組む。
    fn packing(mode: CompactMode) -> Packing {
        Packing {
            fit: BarFit::ROOMY,
            avail_w: Some(RIGHT_GAP),
            compact: mode,
        }
    }

    /// **`never` は何があっても縮めない（0.1.0 の挙動そのまま）。**
    #[test]
    fn never_keeps_the_flock_at_full_size() {
        for count in [0, 1, 6, 7, 50] {
            let w = natural_width(count, &face("egg"), Size::Bar, BarFit::ROOMY);
            assert_eq!(
                squeeze(count, w, Some(RIGHT_GAP), CompactMode::Never),
                Squeeze::NONE,
                "{count} 匹で縮んでいる"
            );
        }
    }

    /// **（後方互換の要）`auto` は収まっているうちは何もしない。**
    ///
    /// ここが崩れると、今まで等倍で見えていたユーザの群れが黙って縮む。
    #[test]
    fn auto_does_nothing_while_the_flock_still_fits() {
        let egg = face("egg");
        for count in 0..=COMPACT_AFTER {
            let w = natural_width(count, &egg, Size::Bar, BarFit::ROOMY);
            assert!(
                w <= RIGHT_GAP,
                "{count} 匹（{w:.1}pt）が空き帯に入らない前提が崩れている"
            );
            assert_eq!(
                squeeze(count, w, Some(RIGHT_GAP), CompactMode::Auto),
                Squeeze::NONE,
                "{count} 匹で縮んでいる"
            );
            // レイアウトも完全に同一。
            assert_eq!(
                lay_out(count, &egg, Size::Bar, BarFit::ROOMY, Squeeze::NONE),
                lay_out(
                    count,
                    &egg,
                    Size::Bar,
                    BarFit::ROOMY,
                    squeeze(count, w, Some(RIGHT_GAP), CompactMode::Auto)
                )
            );
        }
    }

    /// **収まらなくなったら縮み、縮めた結果は必ず使える幅に収まる。**
    ///
    /// 「縮めたのにまだはみ出す」が一番たちの悪い失敗なので、下限に達しない限りは
    /// 収まることを匹数を振って確かめる。
    #[test]
    fn auto_shrinks_until_the_flock_fits() {
        let egg = face("egg");
        for count in (COMPACT_AFTER + 1)..=12 {
            let w = natural_width(count, &egg, Size::Bar, BarFit::ROOMY);
            let sq = squeeze(count, w, Some(RIGHT_GAP), CompactMode::Auto);
            assert!(sq.compact, "{count} 匹で切り替わっていない");
            assert!(sq.scale < 1.0, "{count} 匹で縮んでいない");
            if sq.scale > MIN_COMPACT_SCALE {
                let l = lay_out(count, &egg, Size::Bar, BarFit::ROOMY, sq);
                assert!(
                    l.width <= RIGHT_GAP + 1e-9,
                    "{count} 匹が縮めても収まらない: {:.1} > {RIGHT_GAP}",
                    l.width
                );
            }
        }
    }

    /// **しきい値ちょうど（6 匹）では切り替わらず、その次（7 匹）で切り替わる。**
    #[test]
    fn the_threshold_is_the_last_count_that_still_fits() {
        let egg = face("egg");
        let at = |n: usize| {
            squeeze(
                n,
                natural_width(n, &egg, Size::Bar, BarFit::ROOMY),
                Some(RIGHT_GAP),
                CompactMode::Auto,
            )
        };
        assert!(!at(COMPACT_AFTER).compact, "6 匹で縮んでいる");
        assert!(at(COMPACT_AFTER + 1).compact, "7 匹で縮んでいない");
    }

    /// **下限より下へは縮めない。**
    ///
    /// 判読できない粒になるくらいなら縮小を諦め、`bar_align` の退避に任せる。
    #[test]
    fn the_squeeze_never_goes_below_the_floor() {
        let egg = face("egg");
        for count in [20, 50, 200] {
            let w = natural_width(count, &egg, Size::Bar, BarFit::ROOMY);
            let sq = squeeze(count, w, Some(RIGHT_GAP), CompactMode::Auto);
            assert_eq!(sq.scale, MIN_COMPACT_SCALE, "{count} 匹で下限を割った");
        }
        // 使える幅が 0 でも下限で止まる（0 除算・負の倍率を作らない）。
        assert_eq!(
            squeeze(3, 100.0, Some(0.0), CompactMode::Auto).scale,
            MIN_COMPACT_SCALE
        );
    }

    /// **`always` は収まっていても基準倍率で縮める。**
    #[test]
    fn always_compacts_even_a_single_creature() {
        let egg = face("egg");
        let w = natural_width(1, &egg, Size::Bar, BarFit::ROOMY);
        let sq = squeeze(1, w, Some(RIGHT_GAP), CompactMode::Always);
        assert_eq!(sq.scale, COMPACT_SCALE);
        assert!(sq.compact);
        // ただし 0 匹は縮めようがない（窓自体を隠す）。
        assert_eq!(
            squeeze(0, 1.0, Some(RIGHT_GAP), CompactMode::Always),
            Squeeze::NONE
        );
    }

    /// **使える幅が測れないときは匹数で判断する。**
    ///
    /// `bar_fit` が `FALLBACK_MENU_BAR_H` へ倒すのと同じ考え方。
    #[test]
    fn an_unmeasurable_width_falls_back_to_a_count_threshold() {
        let egg = face("egg");
        let at = |n: usize| {
            squeeze(
                n,
                natural_width(n, &egg, Size::Bar, BarFit::ROOMY),
                None,
                CompactMode::Auto,
            )
        };
        assert!(!at(COMPACT_AFTER).compact);
        assert!(at(COMPACT_AFTER + 1).compact);
        // どこまで縮めれば足りるか分からないので基準倍率で止める。
        assert_eq!(at(COMPACT_AFTER + 1).scale, COMPACT_SCALE);
        assert_eq!(at(100).scale, COMPACT_SCALE);
    }

    /// **（bar の体の高さの上限をコンパクト表示でも守る）縮めた群れもメニューバーに収まり、
    /// グリフがアニメの頂点で切れない。**
    ///
    /// 一様な倍率なら不等式の両辺が同じだけ縮むので理屈の上では自明だが、
    /// 「体だけ縮めて余白は据え置き」への改悪をここが捕まえる。
    #[test]
    fn a_compact_flock_still_fits_inside_every_supported_menu_bar() {
        for &menu_bar_h in &[24.0_f64, 33.0] {
            for f in Registry::builtin().all() {
                let (_, body_h) = f.body_size(Size::Bar);
                let fit = bar_fit(menu_bar_h, body_h);
                for count in [1_usize, 6, 7, 12, 30] {
                    for mode in [CompactMode::Auto, CompactMode::Always] {
                        let packing = Packing {
                            fit,
                            avail_w: Some(RIGHT_GAP),
                            compact: mode,
                        };
                        let sq = packing.squeeze_for(count, f, Size::Bar);
                        let l = lay_out(count, f, Size::Bar, fit, sq);
                        assert!(
                            l.height <= menu_bar_h + 1e-9,
                            "{} × {count} 匹 × {:?} が {menu_bar_h}pt を超える: {:.1}",
                            f.id,
                            mode,
                            l.height
                        );

                        // `creature.rs` と同じ式でグリフの箱の上端を求める
                        // （付属パーツにも同じ倍率が掛かる前提の固定）。
                        let s = l.slots[0];
                        let (_, goy) = theme::glyph_offset(Size::Bar);
                        let glyph_top = if fit.glyph_inside {
                            s.body_y + s.body_h
                        } else {
                            s.body_y + s.body_h + goy * sq.scale
                        };
                        let hop = theme::BAR_MAX_ANIM_AMP * fit.anim_scale * sq.scale;
                        assert!(
                            glyph_top + hop <= l.height + 1e-9,
                            "{} × {count} 匹 × {:?} でグリフが切れる: {:.2} > {:.2}",
                            f.id,
                            mode,
                            glyph_top + hop,
                            l.height
                        );
                    }
                }
            }
        }
    }

    /// 縮小は一様（幅・高さ・スロット位置がすべて同じ倍率）。
    ///
    /// `creature.rs` が付属パーツに `squeeze.scale` を掛けてよい根拠。
    #[test]
    fn squeezing_scales_the_whole_layout_uniformly() {
        let egg = face("egg");
        let sq = Squeeze {
            scale: 0.5,
            compact: true,
        };
        let full = lay_out(4, &egg, Size::Bar, BarFit::ROOMY, Squeeze::NONE);
        let half = lay_out(4, &egg, Size::Bar, BarFit::ROOMY, sq);
        assert!((half.width - full.width * 0.5).abs() < 1e-9);
        assert!((half.height - full.height * 0.5).abs() < 1e-9);
        for (a, b) in full.slots.iter().zip(&half.slots) {
            assert!((b.body_x - a.body_x * 0.5).abs() < 1e-9);
            assert!((b.body_w - a.body_w * 0.5).abs() < 1e-9);
            assert!((b.hit_w - a.hit_w * 0.5).abs() < 1e-9);
        }
    }

    /// コンパクトの dock は名前・経過時間を出さないので、その 2 段ぶん背が低い。
    #[test]
    fn a_compact_dock_drops_the_label_rows() {
        let egg = face("egg");
        let sq = Squeeze {
            scale: 1.0,
            compact: true,
        };
        let full = lay_out(3, &egg, Size::Dock, BarFit::ROOMY, Squeeze::NONE);
        let compact = lay_out(3, &egg, Size::Dock, BarFit::ROOMY, sq);
        // 名前（8pt）＋隙間（7pt）＋行間（2pt）＋経過時間（8pt）ぶん低くなる。
        let labels = theme::NAME_GAP + theme::NAME_FONT + 2.0 + theme::DUR_FONT;
        assert_eq!(compact.height, full.height - labels);
        // 下に残るのはバッジのはみ出しぶんだけ（バッジ自体は出さないが、
        // 余白は一様な縮小を保つために残す — `squeeze` は幅が倍率に比例することを
        // 前提にしているので、余白を条件で足し引きすると成り立たなくなる）。
        assert_eq!(
            compact.slots[0].body_y,
            theme::badge_offset(Size::Dock).1 + theme::badge_h(Size::Dock) / 2.0
        );
    }

    /// 0 匹はどのモードでも 1x1 の窓（縮小の対象にしない）。
    #[test]
    fn an_empty_flock_is_unaffected_by_compacting() {
        for mode in [CompactMode::Auto, CompactMode::Always, CompactMode::Never] {
            let packing = packing(mode);
            let egg = face("egg");
            let sq = packing.squeeze_for(0, &egg, Size::Bar);
            let l = lay_out(0, &egg, Size::Bar, BarFit::ROOMY, sq);
            assert!(l.slots.is_empty());
            assert_eq!((l.width, l.height), (1.0, 1.0));
        }
    }

    /// dock は使える幅が広いので、現実的な匹数では縮まない。
    #[test]
    fn the_dock_does_not_compact_at_realistic_counts() {
        let egg = face("egg");
        // 実測値: 14 インチノッチ機の visibleFrame は 1455pt。パネルの左右余白を引いた値。
        let packing = Packing {
            fit: BarFit::ROOMY,
            avail_w: Some(1455.0 - theme::DOCK_PAD_X * 2.0),
            compact: CompactMode::Auto,
        };
        for count in 1..=12 {
            assert_eq!(
                packing.squeeze_for(count, &egg, Size::Dock),
                Squeeze::NONE,
                "dock の {count} 匹が縮んでいる"
            );
        }
    }

    /// dock は名前＋経過時間のぶん、bar より背が高い。
    #[test]
    fn dock_is_taller_than_bar() {
        let egg = face("egg");
        let b = lay_out(1, &egg, Size::Bar, BarFit::ROOMY, Squeeze::NONE);
        let d = lay_out(1, &egg, Size::Dock, BarFit::ROOMY, Squeeze::NONE);
        assert!(d.height > b.height);
    }

    /// 当たり判定は隣どうしで重ならず、かつ隙間なく敷き詰まる。
    #[test]
    fn hit_boxes_tile_without_overlap() {
        let l = lay_out(4, &face("egg"), Size::Dock, BarFit::ROOMY, Squeeze::NONE);
        for w in l.slots.windows(2) {
            let (a, b) = (w[0], w[1]);
            assert!(
                (a.hit_x + a.hit_w - b.hit_x).abs() < 0.001,
                "当たり判定が重なるか隙間がある"
            );
        }
    }

    /// 当たり判定は窓の高さ全体をカバーする（体の上下のパーツにも反応させる）。
    #[test]
    fn hit_box_spans_the_full_height() {
        let l = bar(1, "egg");
        assert_eq!(l.slots[0].hit_y, 0.0);
        assert_eq!(l.slots[0].hit_h, l.height);
    }
}
