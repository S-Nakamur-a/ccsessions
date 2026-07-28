//! **パーツの表**。キャラクタービルダーが並べる選択肢はここが唯一の定義。
//!
//! # パーツを 1 つ足すには
//! 下の表に**行を 1 つ足すだけ**。UI（`ccsessions/src/builder_ui/`）は
//! `/api/parts` が返すこの表を列挙して描くので、**JS も HTML も触らなくてよい**。
//! カテゴリごと足すときも `LINES` に `LineCategory` を 1 つ増やすだけで、
//! 眉・耳・アクセサリのように「顔に線を足す」種類のものは全部その形に収まる。
//!
//! # なぜ手描き 30 枚ではなく機械生成なのか
//! `faces/*.toml` の顔は 1 つ 1 つ手で数値を詰めて作ってある（`faces/README.md`
//! を見れば、1 つの顔にどれだけの判断が要るか分かる）。それを
//! 5 カテゴリ × 30 = 150 個ぶんやるのは現実的でないし、やったところで
//! **組み合わせたときに破綻しない保証が無い**。
//!
//! そこでこの表は「**少数の素形 × 数値**」で持つ:
//!
//! - 輪郭 … 角丸プロファイル / カプセル / 経由点から起こすシルエット（`shape::smooth_path`）
//! - 目   … まぶた 2 本の弧 / くさび / 角つき丸目（`shape::eye_polygon` ほか）
//! - 線   … 弧・波・流し・への字・かぎ形・輪・点、板・門型・縦棒・角波（`shape::Curve`）
//!
//! 幅を**その高さで顔がどれだけ広いか**（`shape::half_width_at`）に比例させて
//! いるので、細あごの顔に載せた口は自動的に小さくなる。これが
//! 「30×30 のどの組み合わせでも検証を通る」を支えている
//! （番人は `builder::tests::every_part_composes_into_a_valid_face`）。
//!
//! # 人間でないパーツ
//! 素形の後半（`Plate` / `Bracket` / `Stroke` / `Teeth`、目の `Wedge` / `Horn`）と
//! 側面カテゴリは、**機械・けもの寄りの顔を作れるようにするため**にある。
//! 生き物の顔は曲線でできているが、機械の顔は**直角・閉じた板・左右対の継ぎ目**で
//! できていて、弧と波をいくら足しても後者にはならない。
//!
//! ただし**特定の顔専用のパーツは置かない** — 専用の数値は他の輪郭に載せると
//! 比率が合わずに浮くので、汎用の素形×数値のまま「同系統が作れる」ところで止める
//! （番人は `builder::tests::the_builder_can_express_non_human_faces`）。
//!
//! # 左右 1 対のパーツ
//! `pair(...)` で作った行は `[[details]]` が **2 本**（`<cat>-r` / `<cat>-l`）出る。
//! 左は右の鏡像を取るだけなので左右対称が構造的に保たれる（`mod.rs::line_details`）。
//! `off` で顔の端に寄せるが、これも**半幅に比例する**ので細い顔では自動的に
//! 内側へ来る（`shape::place`）。

use crate::face::builder::shape::{self, Curve};

// ---------------------------------------------------------------------------
// 体の寸法
// ---------------------------------------------------------------------------

/// bar の体の高さ（pt）。**組込みの顔すべてが 20**で、上限 22 に対して
/// ノッチ機のグリフぶん（12pt）を足しても 32 < 33 に収まる安全な値
/// （`faces/README.md` §3）。ビルダーはここを可変にしない — 高さを触ると
/// メニューバーの不変条件を投稿者に押し付けることになる。
pub const BAR_H: f64 = 20.0;
/// dock の体の高さ（pt）。組込み顔と同じ。
pub const DOCK_H: f64 = 34.0;

/// bar 幅から dock 幅を出す倍率。組込み顔の実測（18→30 / 24→40）に合わせる。
const DOCK_W_RATIO: f64 = 5.0 / 3.0;

/// bar の体幅から dock の体幅を決める。
pub fn dock_w(bar_w: f64) -> f64 {
    (bar_w * DOCK_W_RATIO).round()
}

// ---------------------------------------------------------------------------
// 輪郭（face）
// ---------------------------------------------------------------------------

/// 輪郭の作り方。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Form {
    /// 角丸長方形。`[水平比率, 垂直比率]` を上下 2 組だけで指定する
    /// （左右非対称な顔は作らせない — 生き物が傾いて見えるため）。
    Corners { top: [f64; 2], bottom: [f64; 2] },
    /// 左右が半円。角丸は自動で h/2。
    Capsule,
    /// 経由点から起こす自由なシルエット。右半分だけ持ち、左は鏡像。
    Silhouette(Sil),
}

/// シルエットの経由点（すべて**半幅**＝中心から縁までの比率 0..0.5）。
///
/// 高さの取り方は固定してある（あご → 0.42 → 0.70 → 0.90 → 頭頂）。
/// 高さまで可変にすると破綻する組み合わせが一気に増えるうえ、
/// 幅 4 つだけでも輪郭の顔つき（面長・えら張り・逆三角）は十分に描き分けられる。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sil {
    /// あご先の高さ。0 が下端。もみあげがあるときは持ち上げる。
    pub chin: f64,
    /// あご幅（もみあげがあるときは切れ込みの内側の幅）。
    pub jaw: f64,
    /// 頬幅（v = 0.42）。
    pub cheek: f64,
    /// こめかみ幅（v = 0.70）。
    pub temple: f64,
    /// 冠の幅（v = 0.90）。
    pub crown: f64,
    /// もみあげ。`Some((切れ込みの高さ, 先端の半幅, 先端の高さ))`。
    /// **顔の外側に髪を出す唯一の手段**で、「凹み 1 個で済ませる」形にしてある
    /// （房を頭から離すと bar の幅では隙間が塗り潰れる）。
    pub burn: Option<(f64, f64, f64)>,
}

/// 輪郭パーツ 1 つ。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FacePart {
    pub id: &'static str,
    pub label: &'static str,
    /// bar の体幅（pt）。dock は `dock_w` で決まる。
    pub w: f64,
    /// 目の縦位置（体高に対する比率）。あごが長い顔は上寄せにする。
    pub eye_v: f64,
    pub form: Form,
}

const fn corners(
    id: &'static str,
    label: &'static str,
    w: f64,
    eye_v: f64,
    top: [f64; 2],
    bottom: [f64; 2],
) -> FacePart {
    FacePart {
        id,
        label,
        w,
        eye_v,
        form: Form::Corners { top, bottom },
    }
}

#[allow(clippy::too_many_arguments)]
const fn sil(
    id: &'static str,
    label: &'static str,
    w: f64,
    eye_v: f64,
    chin: f64,
    jaw: f64,
    cheek: f64,
    temple: f64,
    crown: f64,
) -> FacePart {
    FacePart {
        id,
        label,
        w,
        eye_v,
        form: Form::Silhouette(Sil {
            chin,
            jaw,
            cheek,
            temple,
            crown,
            burn: None,
        }),
    }
}

#[allow(clippy::too_many_arguments)]
const fn sil_burn(
    id: &'static str,
    label: &'static str,
    w: f64,
    eye_v: f64,
    chin: f64,
    jaw: f64,
    cheek: f64,
    temple: f64,
    crown: f64,
    burn: (f64, f64, f64),
) -> FacePart {
    FacePart {
        id,
        label,
        w,
        eye_v,
        form: Form::Silhouette(Sil {
            chin,
            jaw,
            cheek,
            temple,
            crown,
            burn: Some(burn),
        }),
    }
}

/// 顔のライン 30 種。**先頭が既定**（`CharacterConfig::default`）。
#[rustfmt::skip]
pub const FACES: &[FacePart] = &[
    // ── 角丸長方形の系統（CSS の border-radius 相当）──────────────────────
    corners("egg", "たまご", 22.0, 0.50, [0.50, 0.58], [0.48, 0.42]),
    corners("round", "まんまる", 22.0, 0.50, [0.50, 0.50], [0.50, 0.50]),
    corners("squircle", "かどまる", 22.0, 0.50, [0.32, 0.35], [0.32, 0.35]),
    corners("box", "しかく", 22.0, 0.50, [0.16, 0.18], [0.16, 0.18]),
    corners("tile", "いた", 24.0, 0.50, [0.07, 0.08], [0.07, 0.08]),
    corners("pear", "しもぶくれ", 22.0, 0.54, [0.34, 0.40], [0.50, 0.50]),
    corners("acorn", "どんぐり", 22.0, 0.48, [0.50, 0.56], [0.28, 0.30]),
    corners("dome", "ドーム", 22.0, 0.52, [0.50, 0.64], [0.18, 0.20]),
    corners("bucket", "バケツ", 22.0, 0.48, [0.20, 0.22], [0.46, 0.52]),
    corners("slim", "おもなが", 18.0, 0.52, [0.50, 0.46], [0.50, 0.42]),
    corners("broad", "よこひろ", 26.0, 0.50, [0.50, 0.52], [0.48, 0.46]),
    corners("lozenge", "ひしがた", 24.0, 0.50, [0.50, 0.44], [0.50, 0.44]),
    // ── カプセル ────────────────────────────────────────────────────────
    FacePart { id: "capsule", label: "カプセル", w: 26.0, eye_v: 0.50, form: Form::Capsule },
    // ── 経由点から起こすシルエット ────────────────────────────────────────
    sil("oval", "たまご（曲線）", 22.0, 0.52, 0.02, 0.20, 0.46, 0.42, 0.28),
    sil("heart", "さかさ三角", 22.0, 0.54, 0.00, 0.14, 0.42, 0.48, 0.31),
    sil("jawline", "えらばり", 23.0, 0.52, 0.00, 0.37, 0.45, 0.42, 0.26),
    sil("diamond", "ひしがた（曲線）", 23.0, 0.50, 0.00, 0.19, 0.49, 0.33, 0.19),
    sil("chin-long", "しゃくれ", 21.0, 0.58, 0.00, 0.13, 0.44, 0.46, 0.30),
    sil("cheeky", "ほおぶくれ", 24.0, 0.52, 0.03, 0.26, 0.49, 0.38, 0.24),
    sil("teardrop", "しずく", 22.0, 0.55, 0.00, 0.12, 0.38, 0.48, 0.34),
    sil("helmetish", "かぶと", 19.0, 0.58, 0.00, 0.30, 0.47, 0.46, 0.30),
    sil("bulb", "でんきゅう", 21.0, 0.54, 0.02, 0.17, 0.36, 0.48, 0.33),
    sil("shield", "たて", 22.0, 0.52, 0.00, 0.24, 0.47, 0.45, 0.22),
    sil("mushroom", "きのこ", 24.0, 0.46, 0.02, 0.26, 0.40, 0.49, 0.36),
    sil("cone", "とんがり", 21.0, 0.46, 0.02, 0.32, 0.44, 0.32, 0.12),
    sil("pebble", "こいし", 23.0, 0.50, 0.04, 0.30, 0.48, 0.40, 0.24),
    // もみあげ付き。輪郭そのものに髪が出る唯一の系統。
    sil_burn("sideburn", "もみあげ", 24.0, 0.56, 0.15, 0.20, 0.47, 0.44, 0.32, (0.26, 0.30, 0.01)),
    sil_burn("sideburn-long", "ながもみあげ", 24.0, 0.58, 0.22, 0.18, 0.46, 0.44, 0.32, (0.30, 0.31, 0.00)),
    sil_burn("sideburn-short", "みじかもみあげ", 23.0, 0.54, 0.10, 0.22, 0.46, 0.43, 0.30, (0.19, 0.32, 0.03)),
    sil_burn("twin-lobe", "ふたふさ", 25.0, 0.56, 0.18, 0.14, 0.44, 0.46, 0.34, (0.28, 0.34, 0.02)),
];

// ---------------------------------------------------------------------------
// 目（eyes）
// ---------------------------------------------------------------------------

/// 目の描き方。
///
/// 後半 2 つは**人間でない目**。`Lids` は目頭・目尻の両方が頂点になるので、
/// 角ばったスリットも「角 1 本の丸目」も作れない。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum EyeForm {
    /// 角丸矩形。`radius` は `min(w, h)` に対する比率（0.5 で完全な丸）。
    Rounded { radius: f64 },
    /// 多角形。上まぶた・下まぶたの 2 本の弧から作る（`shape::eye_polygon`）。
    ///
    /// `inner` / `outer` は目頭・目尻の高さで、`outer > inner` が吊り目。
    /// `upper` / `lower` はまぶたのふくらみ。
    Lids {
        inner: f64,
        outer: f64,
        upper: f64,
        lower: f64,
    },
    /// くさび形。鼻側とこめかみ側の縦辺 2 本で決まる四角形（`shape::wedge_polygon`）。
    /// 兜のスリットのような、角が 4 つとも立った目。
    Wedge {
        inner_lo: f64,
        inner_hi: f64,
        outer_lo: f64,
        outer_hi: f64,
    },
    /// 丸目のこめかみ側だけを角に引き出した形（`shape::horn_polygon`）。
    /// `up` が真で上に角＝吊り。
    Horn { up: bool },
}

/// 目パーツ 1 つ。寸法は **bar の pt**（dock は `DOCK_EYE_RATIO` 倍）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EyePart {
    pub id: &'static str,
    pub label: &'static str,
    pub w: f64,
    pub h: f64,
    /// 両目の間隔（pt）。
    pub gap: f64,
    pub form: EyeForm,
}

/// bar の目の寸法から dock の寸法を出す倍率。
/// 手描きの顔の実測（丸目 3→4 / スリット 5.5→9 / 角つき丸目 4→6.2）の中間。
pub const DOCK_EYE_RATIO: f64 = 1.55;

const fn round_eye(
    id: &'static str,
    label: &'static str,
    w: f64,
    h: f64,
    gap: f64,
    radius: f64,
) -> EyePart {
    EyePart {
        id,
        label,
        w,
        h,
        gap,
        form: EyeForm::Rounded { radius },
    }
}

#[allow(clippy::too_many_arguments)]
const fn lid_eye(
    id: &'static str,
    label: &'static str,
    w: f64,
    h: f64,
    gap: f64,
    inner: f64,
    outer: f64,
    upper: f64,
    lower: f64,
) -> EyePart {
    EyePart {
        id,
        label,
        w,
        h,
        gap,
        form: EyeForm::Lids {
            inner,
            outer,
            upper,
            lower,
        },
    }
}

#[allow(clippy::too_many_arguments)]
const fn wedge_eye(
    id: &'static str,
    label: &'static str,
    w: f64,
    h: f64,
    gap: f64,
    inner_lo: f64,
    inner_hi: f64,
    outer_lo: f64,
    outer_hi: f64,
) -> EyePart {
    EyePart {
        id,
        label,
        w,
        h,
        gap,
        form: EyeForm::Wedge {
            inner_lo,
            inner_hi,
            outer_lo,
            outer_hi,
        },
    }
}

const fn horn_eye(
    id: &'static str,
    label: &'static str,
    w: f64,
    h: f64,
    gap: f64,
    up: bool,
) -> EyePart {
    EyePart {
        id,
        label,
        w,
        h,
        gap,
        form: EyeForm::Horn { up },
    }
}

/// 目 30 種。
#[rustfmt::skip]
pub const EYES: &[EyePart] = &[
    // ── 角丸矩形の系統 ───────────────────────────────────────────────────
    round_eye("bead", "つぶら", 3.0, 3.4, 3.2, 0.50),
    round_eye("bead-small", "ちいさめ", 2.4, 2.6, 3.6, 0.50),
    round_eye("bead-big", "おおきめ", 4.2, 4.4, 3.4, 0.50),
    round_eye("oval-tall", "たてなが", 3.0, 4.6, 3.2, 0.50),
    round_eye("oval-wide", "よこなが", 4.6, 3.0, 2.8, 0.50),
    round_eye("soft-square", "まるしかく", 3.4, 3.6, 3.2, 0.28),
    round_eye("hard-square", "かどしかく", 3.4, 3.6, 3.2, 0.08),
    round_eye("pixel", "ドット", 2.8, 2.8, 3.6, 0.00),
    round_eye("bar-h", "よこ一文字", 5.0, 1.8, 2.6, 0.50),
    round_eye("bar-v", "たてすじ", 1.8, 4.8, 3.6, 0.50),
    round_eye("brick-tall", "たてれんが", 2.6, 5.0, 3.4, 0.18),
    round_eye("brick-wide", "よこれんが", 5.2, 2.6, 2.6, 0.18),
    // ── まぶた 2 本から起こす多角形 ────────────────────────────────────────
    lid_eye("almond", "アーモンド", 4.2, 3.4, 3.0, 0.50, 0.50, 0.45, 0.45),
    lid_eye("almond-up", "つりめ", 4.2, 3.6, 3.0, 0.34, 0.70, 0.42, 0.42),
    lid_eye("almond-down", "たれめ", 4.2, 3.6, 3.0, 0.70, 0.34, 0.42, 0.42),
    lid_eye("leaf", "このは", 4.6, 2.8, 2.8, 0.50, 0.50, 0.30, 0.30),
    lid_eye("leaf-up", "このは（つり）", 4.6, 3.0, 2.8, 0.30, 0.74, 0.26, 0.26),
    lid_eye("leaf-down", "このは（たれ）", 4.6, 3.0, 2.8, 0.74, 0.30, 0.26, 0.26),
    lid_eye("slit", "スリット", 5.2, 2.2, 2.6, 0.50, 0.50, 0.16, 0.16),
    lid_eye("slit-up", "スリット（つり）", 5.2, 2.4, 2.6, 0.32, 0.72, 0.14, 0.14),
    lid_eye("slit-down", "スリット（たれ）", 5.2, 2.4, 2.6, 0.72, 0.32, 0.14, 0.14),
    lid_eye("jito", "ジト目", 4.4, 2.8, 2.8, 0.50, 0.50, 0.10, 0.46),
    lid_eye("halfmoon", "はんげつ", 4.4, 2.8, 2.8, 0.50, 0.50, 0.46, 0.10),
    lid_eye("sharp-up", "きつねめ", 4.4, 3.8, 2.8, 0.22, 0.86, 0.30, 0.30),
    lid_eye("sharp-down", "たぬきめ", 4.4, 3.8, 2.8, 0.86, 0.22, 0.30, 0.30),
    lid_eye("droplet", "しずく", 3.8, 3.8, 3.0, 0.62, 0.40, 0.52, 0.24),
    lid_eye("droplet-up", "さかしずく", 3.8, 3.8, 3.0, 0.40, 0.62, 0.24, 0.52),
    lid_eye("wide-almond", "ひらたアーモンド", 5.2, 3.0, 2.6, 0.50, 0.50, 0.38, 0.38),
    lid_eye("narrow-almond", "ほそアーモンド", 3.2, 3.6, 3.4, 0.50, 0.50, 0.50, 0.50),
    lid_eye("round-poly", "まんまる（多角）", 4.0, 4.0, 3.0, 0.50, 0.50, 0.62, 0.62),
    // ── 人間でない目 ────────────────────────────────────────────────────
    // くさび。角が 4 つとも立っていて、弧では作れない。
    wedge_eye("wedge", "くさび", 5.5, 3.0, 2.5, 0.00, 0.62, 0.38, 1.00),
    wedge_eye("wedge-down", "くさび（たれ）", 5.5, 3.0, 2.5, 0.38, 1.00, 0.00, 0.62),
    wedge_eye("wedge-narrow", "くさび（細）", 5.6, 2.2, 2.4, 0.00, 0.48, 0.52, 1.00),
    wedge_eye("visor", "バイザー", 6.0, 2.4, 2.2, 0.10, 0.90, 0.00, 1.00),
    wedge_eye("slot", "スロット", 5.0, 2.6, 2.6, 0.00, 1.00, 0.00, 1.00),
    wedge_eye("shard", "かけら", 4.4, 4.0, 2.8, 0.00, 0.20, 0.30, 1.00),
    // 角つき丸目。尖りは 1 点だけ。
    horn_eye("horn", "つのつき丸目", 4.0, 4.2, 3.5, true),
    horn_eye("horn-down", "つのつき丸目（たれ）", 4.0, 4.2, 3.5, false),
];

// ---------------------------------------------------------------------------
// 線のカテゴリ（hair / nose / mouth …）
// ---------------------------------------------------------------------------

/// 折れ線で描くパーツ 1 つ。**髪も鼻も口も眉も、全部この型**。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LinePart {
    pub id: &'static str,
    pub label: &'static str,
    pub curve: Curve,
    /// 基準の縦位置（体高に対する比率）。
    pub v: f64,
    /// 幅。**その高さで顔が持つ半幅に対する比率**なので、細い顔に載せれば
    /// 自動的に小さくなる（`shape::half_width_at`）。
    pub w: f64,
    /// 縦の振れ（体高に対する比率）。素形ごとの意味は `Curve` の doc を見る。
    pub amp: f64,
    /// 中心からの横位置（`w` と同じ単位＝半幅に対する比率）。0 で中央。
    /// 耳や頬のパネルのように顔の端に寄せる部品で使う。
    pub off: f64,
    /// **左右 1 対で描く**。真なら `[[details]]` が 2 本（`<cat>-r` / `<cat>-l`）出る。
    /// 左は右を x → 1-x で折り返したものなので、左右対称が構造的に保たれる。
    pub mirror: bool,
}

const fn line(
    id: &'static str,
    label: &'static str,
    curve: Curve,
    v: f64,
    w: f64,
    amp: f64,
) -> LinePart {
    LinePart {
        id,
        label,
        curve,
        v,
        w,
        amp,
        off: 0.0,
        mirror: false,
    }
}

/// **左右 1 対**のパーツ。`off` は右側の中心位置で、左はその鏡像。
/// 耳（`ear-r` / `ear-l`）や頬の継ぎ目（`cheek-r` / `cheek-l`）がこの形。
#[allow(clippy::too_many_arguments)]
const fn pair(
    id: &'static str,
    label: &'static str,
    curve: Curve,
    v: f64,
    off: f64,
    w: f64,
    amp: f64,
) -> LinePart {
    LinePart {
        id,
        label,
        curve,
        v,
        w,
        amp,
        off,
        mirror: true,
    }
}

/// 「描かない」パーツ。**どのカテゴリも先頭にこれを置く**か、外すかは
/// カテゴリの自由（髪と口は「なし」を選べたほうがよく、目と輪郭は選べない）。
const fn none(id: &'static str, label: &'static str) -> LinePart {
    line(id, label, Curve::None, 0.5, 0.0, 0.0)
}

/// 線パーツのカテゴリ。**ここに 1 つ足せば新しいカテゴリが UI に生える**
/// （眉・耳・アクセサリはこの形にそのまま収まる）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LineCategory {
    pub id: &'static str,
    pub label: &'static str,
    /// bar（メニューバー）にも描くか。
    ///
    /// **既定で `false` にしてあるカテゴリが多いのは bar の線を間引くため**。bar は
    /// 18×20pt しかなく、線を何本も引くと潰れて塊になり、シルエットと目という
    /// 「一目で読める部分」まで濁る（`faces/README.md` §5）。
    /// `bar ⊆ dock` が構造的に保たれるので `bar-details-not-thinned` に落ちない。
    pub on_bar: bool,
    pub variants: &'static [LinePart],
}

/// 線カテゴリの一覧。**並びがそのまま `[[details]]` の並び（描画順）になる。**
///
/// 髪だけ `on_bar = true` なのは、前髪 1 本ならシルエットを濁さず、帯の中でも
/// 顔つきの違いが残るから。
#[rustfmt::skip]
pub const LINES: &[LineCategory] = &[
    LineCategory { id: "hair",  label: "髪", on_bar: true,  variants: HAIR },
    LineCategory { id: "nose",  label: "鼻", on_bar: false, variants: NOSE },
    LineCategory { id: "mouth", label: "口", on_bar: false, variants: MOUTH },
    LineCategory { id: "side",  label: "側面", on_bar: false, variants: SIDE },
];

/// 髪 30 種。生え際・前髪のラインとして v = 0.62〜0.88 に置く。
#[rustfmt::skip]
pub const HAIR: &[LinePart] = &[
    none("none", "なし"),
    line("straight", "ぱっつん", Curve::Arc, 0.74, 0.88, 0.010),
    line("straight-high", "ぱっつん（高）", Curve::Arc, 0.82, 0.74, 0.010),
    line("straight-low", "ぱっつん（低）", Curve::Arc, 0.66, 0.92, 0.010),
    line("round", "まるまえがみ", Curve::Arc, 0.74, 0.88, 0.055),
    line("round-deep", "まるまえがみ（深）", Curve::Arc, 0.76, 0.88, 0.100),
    line("bowl", "おかっぱ", Curve::Arc, 0.70, 0.94, 0.075),
    line("dip", "うちまき", Curve::Arc, 0.76, 0.88, -0.055),
    line("dip-deep", "うちまき（深）", Curve::Arc, 0.78, 0.86, -0.095),
    line("widow", "ふじびたい", Curve::Vee, 0.80, 0.70, -0.075),
    line("wave2", "なみ 2", Curve::Wave(2), 0.76, 0.86, 0.050),
    line("wave3", "なみ 3", Curve::Wave(3), 0.76, 0.86, 0.050),
    line("wave4", "なみ 4", Curve::Wave(4), 0.76, 0.86, 0.045),
    line("wave5", "なみ 5", Curve::Wave(5), 0.76, 0.86, 0.040),
    line("spike3", "とげ 3", Curve::Wave(3), 0.74, 0.88, 0.095),
    line("spike4", "とげ 4", Curve::Wave(4), 0.74, 0.88, 0.090),
    line("spike5", "とげ 5", Curve::Wave(5), 0.74, 0.88, 0.085),
    line("spike-tall", "とげ（長）", Curve::Wave(3), 0.72, 0.86, 0.130),
    line("part-r", "わけめ（右）", Curve::Sweep(1), 0.76, 0.86, 0.070),
    line("part-l", "わけめ（左）", Curve::Sweep(-1), 0.76, 0.86, 0.070),
    line("part-r-steep", "わけめ（右・急）", Curve::Sweep(1), 0.76, 0.84, 0.120),
    line("part-l-steep", "わけめ（左・急）", Curve::Sweep(-1), 0.76, 0.84, 0.120),
    line("sweep-r-low", "ながしまえがみ（右）", Curve::Sweep(1), 0.70, 0.92, 0.085),
    line("sweep-l-low", "ながしまえがみ（左）", Curve::Sweep(-1), 0.70, 0.92, 0.085),
    line("crest", "クレスト", Curve::Ring, 0.86, 0.20, 0.045),
    line("crest-wide", "クレスト（大）", Curve::Ring, 0.84, 0.30, 0.060),
    line("band", "ヘアバンド", Curve::Arc, 0.84, 0.92, 0.008),
    line("band-low", "ヘアバンド（低）", Curve::Arc, 0.78, 0.94, 0.008),
    line("tuft", "ちょんまげ", Curve::Vee, 0.90, 0.22, -0.055),
    line("bald-line", "そりこみ", Curve::Sweep(1), 0.82, 0.62, 0.055),
    // ── 人間でない頭 ────────────────────────────────────────────────────
    // 眉として使えるよう低く引く 1 本。
    line("brow", "まゆ", Curve::Arc, 0.72, 0.64, 0.025),
    line("brow-flat", "まゆ（一文字）", Curve::Arc, 0.72, 0.64, 0.004),
    line("gate", "ひたいの枠", Curve::Bracket { down: true }, 0.86, 0.16, 0.085),
    line("gate-wide", "ひたいの枠（広）", Curve::Bracket { down: true }, 0.86, 0.30, 0.075),
    line("plate-crest", "かくクレスト", Curve::Plate { taper: 0.35 }, 0.86, 0.20, 0.050),
    // **髪は bar にも描くので、山は「少なく・高く」**。20pt の帯に細かい歯を
    // 並べると潰れて灰色の塊になり、シルエットと目まで濁る（`faces/README.md` §5）。
    // 番人は `parts_whose_shape_is_vertical_do_not_collapse_at_real_size`。
    line("antenna", "アンテナ", Curve::Bracket { down: true }, 0.90, 0.06, 0.095),
    line("vent-top", "てんめんスリット", Curve::Teeth(3), 0.84, 0.60, 0.095),
    line("notches", "きざみ", Curve::Teeth(4), 0.80, 0.70, 0.088),
];

/// 鼻 30 種。目の少し下（v = 0.36〜0.48）に小さく置く。
#[rustfmt::skip]
pub const NOSE: &[LinePart] = &[
    none("none", "なし"),
    line("dot", "てん", Curve::Dot, 0.42, 0.07, 0.0),
    line("dot-wide", "てん（広）", Curve::Dot, 0.42, 0.12, 0.0),
    line("dash", "よこぼう", Curve::Dot, 0.40, 0.18, 0.0),
    line("vee", "への字", Curve::Vee, 0.42, 0.14, 0.035),
    line("vee-wide", "への字（広）", Curve::Vee, 0.42, 0.22, 0.035),
    line("vee-deep", "への字（深）", Curve::Vee, 0.43, 0.16, 0.060),
    line("caret", "やま", Curve::Vee, 0.40, 0.16, -0.040),
    line("arc", "まる鼻", Curve::Arc, 0.40, 0.14, -0.035),
    line("arc-wide", "まる鼻（広）", Curve::Arc, 0.40, 0.22, -0.035),
    line("arc-up", "そり鼻", Curve::Arc, 0.41, 0.16, 0.040),
    line("hook-l", "かぎ（左）", Curve::Hook(1), 0.40, 0.16, 0.075),
    line("hook-r", "かぎ（右）", Curve::Hook(-1), 0.40, 0.16, 0.075),
    line("hook-l-long", "かぎ（左・長）", Curve::Hook(1), 0.38, 0.16, 0.115),
    line("hook-r-long", "かぎ（右・長）", Curve::Hook(-1), 0.38, 0.16, 0.115),
    line("hook-l-wide", "かぎ（左・広）", Curve::Hook(1), 0.40, 0.26, 0.075),
    line("hook-r-wide", "かぎ（右・広）", Curve::Hook(-1), 0.40, 0.26, 0.075),
    line("bridge", "はなすじ", Curve::Dot, 0.44, 0.03, 0.0),
    line("ring", "まる（輪）", Curve::Ring, 0.41, 0.09, 0.030),
    line("ring-wide", "まる（輪・大）", Curve::Ring, 0.41, 0.14, 0.040),
    line("snout", "ぶた", Curve::Ring, 0.40, 0.17, 0.032),
    line("beak", "くちばし", Curve::Vee, 0.41, 0.10, 0.075),
    line("high-dot", "てん（高）", Curve::Dot, 0.47, 0.07, 0.0),
    line("low-dot", "てん（低）", Curve::Dot, 0.36, 0.07, 0.0),
    line("high-vee", "への字（高）", Curve::Vee, 0.47, 0.14, 0.035),
    line("low-vee", "への字（低）", Curve::Vee, 0.36, 0.14, 0.035),
    line("wave", "ふにゃ", Curve::Wave(2), 0.41, 0.16, 0.030),
    line("slant-r", "ななめ（右）", Curve::Sweep(1), 0.41, 0.13, 0.045),
    line("slant-l", "ななめ（左）", Curve::Sweep(-1), 0.41, 0.13, 0.045),
    line("wide-flat", "ひらたい", Curve::Arc, 0.39, 0.30, -0.012),
    // ── 人間でない鼻 ────────────────────────────────────────────────────
    // 縦の鼻筋。横に寝ていない唯一の系統。
    line("stroke", "たてはなすじ", Curve::Stroke, 0.48, 0.0, 0.055),
    line("stroke-long", "たてはなすじ（長）", Curve::Stroke, 0.50, 0.0, 0.095),
    line("seam", "つぎめ", Curve::Stroke, 0.44, 0.0, 0.030),
    line("bolt", "ボルト", Curve::Plate { taper: 1.0 }, 0.42, 0.05, 0.032),
];

/// 口 30 種。あごと鼻のあいだ（v = 0.22〜0.32）に置く。
#[rustfmt::skip]
pub const MOUTH: &[LinePart] = &[
    none("none", "なし"),
    line("flat", "一文字", Curve::Arc, 0.28, 0.34, 0.006),
    line("flat-wide", "一文字（広）", Curve::Arc, 0.28, 0.52, 0.006),
    line("smile", "わらい", Curve::Arc, 0.29, 0.34, -0.038),
    line("smile-wide", "わらい（広）", Curve::Arc, 0.29, 0.52, -0.038),
    line("smile-big", "おおわらい", Curve::Arc, 0.30, 0.46, -0.075),
    line("grin", "にんまり", Curve::Arc, 0.29, 0.60, -0.055),
    line("frown", "への字口", Curve::Arc, 0.27, 0.34, 0.038),
    line("frown-wide", "への字口（広）", Curve::Arc, 0.27, 0.52, 0.038),
    line("frown-big", "むっつり", Curve::Arc, 0.26, 0.46, 0.075),
    line("vee-down", "への字（折れ）", Curve::Vee, 0.29, 0.30, 0.055),
    line("vee-up", "レの字", Curve::Vee, 0.27, 0.30, -0.055),
    line("wave2", "むにゃ 2", Curve::Wave(2), 0.28, 0.40, 0.035),
    line("wave3", "むにゃ 3", Curve::Wave(3), 0.28, 0.44, 0.030),
    line("cat", "ねこぐち", Curve::Wave(2), 0.28, 0.36, -0.045),
    line("cat-wide", "ねこぐち（広）", Curve::Wave(2), 0.28, 0.52, -0.045),
    line("open", "あいたくち", Curve::Ring, 0.28, 0.18, 0.055),
    line("open-wide", "おおきくあいた", Curve::Ring, 0.28, 0.26, 0.080),
    line("open-small", "ちいさくあいた", Curve::Ring, 0.28, 0.11, 0.035),
    line("dot", "てんくち", Curve::Dot, 0.28, 0.07, 0.0),
    line("smirk-r", "にやり（右）", Curve::Sweep(1), 0.28, 0.32, 0.040),
    line("smirk-l", "にやり（左）", Curve::Sweep(-1), 0.28, 0.32, 0.040),
    line("smirk-r-wide", "にやり（右・広）", Curve::Sweep(1), 0.28, 0.48, 0.045),
    line("smirk-l-wide", "にやり（左・広）", Curve::Sweep(-1), 0.28, 0.48, 0.045),
    line("high-smile", "わらい（高）", Curve::Arc, 0.33, 0.36, -0.038),
    line("low-smile", "わらい（低）", Curve::Arc, 0.23, 0.36, -0.038),
    line("high-flat", "一文字（高）", Curve::Arc, 0.33, 0.36, 0.006),
    line("low-flat", "一文字（低）", Curve::Arc, 0.23, 0.36, 0.006),
    line("zigzag", "ぎざぐち", Curve::Wave(4), 0.28, 0.42, 0.035),
    line("beak", "とがりぐち", Curve::Vee, 0.28, 0.16, -0.070),
    // ── 人間でない口 ────────────────────────────────────────────────────
    // 台形のプレート。角が立つので板に見える。
    line("plate", "プレート", Curve::Plate { taper: 0.64 }, 0.28, 0.40, 0.075),
    line("plate-wide", "プレート（広）", Curve::Plate { taper: 0.72 }, 0.28, 0.54, 0.070),
    line("plate-flat", "プレート（浅）", Curve::Plate { taper: 1.0 }, 0.27, 0.44, 0.035),
    line("grille", "グリル", Curve::Teeth(4), 0.26, 0.38, 0.065),
    line("grille-wide", "グリル（広）", Curve::Teeth(6), 0.26, 0.52, 0.065),
    line("vent", "つうきこう", Curve::Teeth(3), 0.26, 0.30, 0.070),
    line("slot", "スロット", Curve::Bracket { down: false }, 0.27, 0.24, 0.040),
    line("mouth-seam", "つぎめ", Curve::Stroke, 0.28, 0.0, 0.045),
];

/// 側面 16 種。**すべて左右 1 対**（`pair`）で、顔の端に寄せて置く。
///
/// 耳や頬の継ぎ目がこのカテゴリ。中央に 1 本引く他のカテゴリと違い、`off` で
/// 端に寄せるので **顔が細ければ自動で内側に来る**
/// （`shape::place` が `off` も半幅に比例させる）。
///
/// `on_bar = false`。18〜24pt の帯に端の小片を足しても潰れて汚れるだけで、
/// dock でしか意味を持たない。
#[rustfmt::skip]
pub const SIDE: &[LinePart] = &[
    none("none", "なし"),
    // 耳・側頭のパネル。
    pair("ear-panel", "耳パネル", Curve::Plate { taper: 1.0 }, 0.40, 0.80, 0.12, 0.075),
    pair("ear-panel-tall", "耳パネル（長）", Curve::Plate { taper: 1.0 }, 0.42, 0.80, 0.10, 0.115),
    pair("ear-taper", "耳パネル（すぼみ）", Curve::Plate { taper: 0.45 }, 0.40, 0.79, 0.13, 0.080),
    pair("ear-round", "耳（丸）", Curve::Ring, 0.40, 0.78, 0.13, 0.070),
    pair("ear-hook", "耳（かぎ）", Curve::Bracket { down: false }, 0.41, 0.78, 0.12, 0.070),
    // 頬・装甲の継ぎ目。
    pair("cheek-seam", "頬のつぎめ", Curve::Sweep(1), 0.36, 0.76, 0.10, 0.055),
    pair("cheek-seam-long", "頬のつぎめ（長）", Curve::Sweep(1), 0.38, 0.74, 0.14, 0.090),
    pair("jaw-seam", "あごのつぎめ", Curve::Sweep(-1), 0.30, 0.70, 0.12, 0.055),
    pair("temple-seam", "こめかみのつぎめ", Curve::Stroke, 0.60, 0.80, 0.0, 0.060),
    // 機械の小物。
    pair("vent", "はいきこう", Curve::Teeth(3), 0.38, 0.74, 0.14, 0.065),
    pair("hinge", "ヒンジ", Curve::Bracket { down: true }, 0.44, 0.80, 0.09, 0.055),
    pair("bolt", "ボルト", Curve::Plate { taper: 1.0 }, 0.44, 0.82, 0.06, 0.035),
    // 生き物寄りの小物（同じ仕掛けで作れる）。
    pair("blush", "ほお", Curve::Ring, 0.32, 0.72, 0.14, 0.038),
    pair("whisker", "ひげ", Curve::Sweep(1), 0.30, 0.72, 0.16, 0.030),
    pair("scar", "きず", Curve::Stroke, 0.46, 0.76, 0.0, 0.070),
];

// ---------------------------------------------------------------------------
// 引き当て
// ---------------------------------------------------------------------------

/// 輪郭パーツを id で引く。**未知の id は既定（先頭）へ落とす** — 保存した
/// config を後から読むとき、消えたパーツで UI ごと死ぬのを避けるため
/// （`Registry::resolve` と同じ方針）。
pub fn face(id: &str) -> &'static FacePart {
    FACES.iter().find(|p| p.id == id).unwrap_or(&FACES[0])
}

/// 目パーツを id で引く。未知の id は既定へ。
pub fn eyes(id: &str) -> &'static EyePart {
    EYES.iter().find(|p| p.id == id).unwrap_or(&EYES[0])
}

/// 線カテゴリを id で引く。
pub fn category(id: &str) -> Option<&'static LineCategory> {
    LINES.iter().find(|c| c.id == id)
}

/// 線パーツを引く。未知の id はそのカテゴリの先頭（＝「なし」）へ。
pub fn line_part(cat: &LineCategory, id: &str) -> &'static LinePart {
    cat.variants
        .iter()
        .find(|p| p.id == id)
        .unwrap_or(&cat.variants[0])
}

/// シルエットの経由点（右半分）。`shape::smooth_path` にそのまま渡せる。
///
/// **最初と最後は必ず x = 0.5**（`half = true` の折り返し点が中央にあることを
/// 対称性の検査が要求するため）。
pub fn silhouette_points(s: &Sil) -> Vec<(f64, f64)> {
    let u = |half: f64| shape::clamp01(0.5 + half.clamp(0.0, 0.5));
    let mut pts = vec![(0.5, shape::clamp01(s.chin))];
    match s.burn {
        Some((notch_v, tip, tip_v)) => {
            // あご → 切れ込みの内側 → もみあげの先端 → 頬。
            pts.push((u(s.jaw), shape::clamp01(notch_v)));
            pts.push((u(tip), shape::clamp01(tip_v)));
        }
        None => pts.push((u(s.jaw), shape::clamp01(s.chin + 0.14))),
    }
    pts.push((u(s.cheek), 0.42));
    pts.push((u(s.temple), 0.70));
    pts.push((u(s.crown), 0.90));
    pts.push((0.5, 1.0));
    pts
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    /// 中核 5 カテゴリは 30 以上ある（「約 30 パターン」という要件の番人）。
    ///
    /// **`==` ではなく `>=` なのは意図的**。この表は「行を 1 つ足すだけで
    /// パーツが増える」ことが売りなので、`== 30` にすると**行を足すたびに
    /// テストが落ちて設計そのものを罰する**。下限だけを見る。
    /// 後から生えたカテゴリ（側面など）は 8 以上あればよい — 中央に引く線と
    /// 違って置ける場所が端に限られ、無理に 30 まで水増しすると似た小片が並ぶ。
    #[test]
    fn every_category_offers_enough_variants() {
        assert!(FACES.len() >= 30, "顔のライン: {}", FACES.len());
        assert!(EYES.len() >= 30, "目: {}", EYES.len());
        for c in LINES {
            let core = matches!(c.id, "hair" | "nose" | "mouth");
            let min = if core { 30 } else { 8 };
            assert!(
                c.variants.len() >= min,
                "{} のバリエーション数 {} が {min} 未満",
                c.id,
                c.variants.len()
            );
        }
    }

    /// **形が縦方向にしか出ないパーツは、実寸で潰れない高さがある。**
    ///
    /// 弧や流しは「ほぼ平ら」でも横に伸びた線として読める（ぱっつん前髪が
    /// まさにそれ）。しかし**角波・門型・板・輪・縦棒は縦の構造そのものが正体**
    /// なので、潰れると別物になる:
    ///
    /// - 角波が潰れる → ただの一文字。`flat` と見分けがつかない
    /// - 縦棒が潰れる → 何も描いていないのと同じ（横に伸びていないため）
    ///
    /// 検証は「はみ出していないか」しか見ないので、ここは素通りする。
    /// 実際に歯の細かい角波を `amp = 0.022` で置き、**5 倍に拡大しても何も
    /// 見えなかった**ので下限を切ることにした（今の `hair/notches` の前身）。
    ///
    /// 高さは `amp` ではなく**実際に生成した点の広がり**で測る — `Teeth` だけ
    /// `0..amp` で、他は `±amp` なので、`amp` を直に見ると 2 倍ずれる。
    /// 比べる相手はパネル線の太さ（`detail_line_w`）の 2 倍で、
    /// 「上下の線のあいだに隙間が残る」最小値。
    #[test]
    fn parts_whose_shape_is_vertical_do_not_collapse_at_real_size() {
        use crate::face::style::detail_line_w;
        use crate::face::Size;

        // 1 個目で止めず全部挙げる（数値を詰め直すとき往復しないで済む）。
        let mut bad: Vec<String> = Vec::new();
        for c in LINES {
            // bar にも描くカテゴリは狭いほう（bar）で評価する。
            let size = if c.on_bar { Size::Bar } else { Size::Dock };
            let body_h = if c.on_bar { BAR_H } else { DOCK_H };
            let min = detail_line_w(size) * 2.0;

            for p in c.variants {
                let vertical = matches!(
                    p.curve,
                    Curve::Teeth(_)
                        | Curve::Bracket { .. }
                        | Curve::Plate { .. }
                        | Curve::Ring
                        | Curve::Stroke
                );
                if !vertical {
                    continue;
                }
                let pts = shape::curve_points(p.curve, p.amp);
                let lo = pts.iter().fold(f64::INFINITY, |m, q| m.min(q.1));
                let hi = pts.iter().fold(f64::NEG_INFINITY, |m, q| m.max(q.1));
                let tall = (hi - lo) * body_h;
                if tall < min {
                    bad.push(format!(
                        "{}/{} が {tall:.2}pt（最低 {min}pt / {size:?}）",
                        c.id, p.id
                    ));
                }
            }
        }
        assert!(
            bad.is_empty(),
            "実寸で潰れて形が消えるパーツがある:\n  {}",
            bad.join("\n  ")
        );
    }

    /// 対のパーツは端に寄せてあり、`off` と `w` の和が行き過ぎていない。
    ///
    /// 実際にはみ出すかは `builder::tests::every_line_part_fits_on_every_outline`
    /// が全輪郭で見るが、ここで表の書き間違い（`off` を入れ忘れた対パーツなど）を
    /// 早く捕まえる。
    #[test]
    fn paired_parts_are_pushed_to_the_side() {
        for c in LINES {
            for p in c.variants {
                if !p.mirror {
                    continue;
                }
                assert!(p.off > 0.4, "{}/{} が端に寄っていない", c.id, p.id);
                assert!(
                    p.off + p.w <= 1.0,
                    "{}/{} が外に出すぎ（off {} + w {}）",
                    c.id,
                    p.id,
                    p.off,
                    p.w
                );
            }
        }
    }

    /// id が重複していない（重複すると `find` が先勝ちで静かに選べなくなる）。
    #[test]
    fn part_ids_are_unique_within_each_category() {
        let uniq = |ids: Vec<&str>, what: &str| {
            let set: BTreeSet<&str> = ids.iter().copied().collect();
            assert_eq!(set.len(), ids.len(), "{what} の id が重複している");
        };
        uniq(FACES.iter().map(|p| p.id).collect(), "faces");
        uniq(EYES.iter().map(|p| p.id).collect(), "eyes");
        for c in LINES {
            uniq(c.variants.iter().map(|p| p.id).collect(), c.id);
        }
        uniq(LINES.iter().map(|c| c.id).collect(), "categories");
    }

    /// カテゴリ id は顔の TOML の `[[details]]` の name になるので、
    /// `is_valid_id` と同じ制約（英小文字・数字・ハイフン）を満たすこと。
    #[test]
    fn category_ids_are_safe_as_detail_names() {
        for c in LINES {
            assert!(
                crate::face::spec::is_valid_id(c.id),
                "カテゴリ id {:?} が使えない",
                c.id
            );
        }
    }

    /// 未知の id は既定へ落ちる（保存済み config が壊れない）。
    #[test]
    fn unknown_ids_fall_back_to_the_default() {
        assert_eq!(face("no-such-part").id, FACES[0].id);
        assert_eq!(eyes("no-such-part").id, EYES[0].id);
        let hair = category("hair").unwrap();
        assert_eq!(line_part(hair, "no-such-part").id, "none");
        assert!(category("no-such-category").is_none());
    }

    /// シルエットの経由点は箱の中で、両端が中央にある（輪郭と対称性の前提）。
    #[test]
    fn silhouette_points_start_and_end_at_the_centre() {
        for p in FACES {
            let Form::Silhouette(s) = p.form else {
                continue;
            };
            let pts = silhouette_points(&s);
            assert!((pts[0].0 - 0.5).abs() < 1e-9, "{} のあごが中央にない", p.id);
            assert_eq!(*pts.last().unwrap(), (0.5, 1.0), "{} の頭頂", p.id);
            for (x, y) in pts {
                assert!(
                    (0.0..=1.0).contains(&x) && (0.0..=1.0).contains(&y),
                    "{} が箱の外",
                    p.id
                );
            }
        }
    }

    /// bar の体幅は体の高さの上限と同じ考え方で常識的な範囲に収まる。
    #[test]
    fn body_widths_stay_reasonable() {
        for p in FACES {
            assert!((16.0..=28.0).contains(&p.w), "{} の幅 {}", p.id, p.w);
            assert!(dock_w(p.w) > p.w, "{} の dock 幅", p.id);
        }
    }
}
