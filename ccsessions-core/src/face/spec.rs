//! `FaceSpec` — 顔 1 つぶんのデータと、そこから描画用の値を解く純関数。
//!
//! `faces/*.toml` を読んだ結果がこの型で、`ccsessionsd` の CALayer 組み立ても
//! `ccsessions face render` の SVG も**この同じ解決関数を通る**。2 経路が同じ数値を
//! 使うからこそ SVG が CALayer の忠実なプレビューになる。
//!
//! # 座標系
//! `outline` / `polygon` / `details` の座標はすべて**体の矩形に対する 0..1 の比率**で、
//! **左下原点・y は上向き**（CALayer と同じ）。pt で持つのは `size` / `gap` /
//! `radius` / `corners_pt` / 目の `size` だけ。
//!
//! # 顔が決められないもの
//! 色・アニメ・グリフは顔ごとに変えられない。状態の読み取りやすさ
//! （シアン＝作業中、琥珀＝判断待ち…）が壊れるため。顔が状態を表現する余地は
//! **目の形・開き具合・向き**に閉じている。

use crate::face::palette::{self, Rgb};
use crate::face::{outline, seg_to, Corners, Outline, Seg, Size};
use crate::session::SessionState;

// ---------------------------------------------------------------------------
// 補助
// ---------------------------------------------------------------------------

/// bar / dock で別の値を持つもの。TOML では `{ bar = ..., dock = ... }`。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BySize<T> {
    pub bar: T,
    pub dock: T,
}

impl<T: Copy> BySize<T> {
    pub fn get(&self, size: Size) -> T {
        if size.is_bar() {
            self.bar
        } else {
            self.dock
        }
    }
}

/// 顔の検証・パースで見つかった問題の種類。
///
/// **ここが唯一の定義**。各バリアントの doc comment がその規則の意味で、
/// 中身は `face::validate` が実装している。`as_str` の値は `ccsessions face check`
/// の出力と Web UI にそのまま出るので、**読んで意味が分かる名前**にすること
/// （番号だと出力を見た人が対応表を引く羽目になる）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProblemCode {
    /// 輪郭が閉じていない、または体の矩形からはみ出す。
    Outline,
    /// `half = true` の輪郭が左右対称になっていない。
    Symmetry,
    /// 体の寸法が不正（bar の高さが上限超え、または非正・非有限）。
    BodySize,
    /// 両目と間隔の合計が体の幅に収まらない。
    EyesTooWide,
    /// 目が輪郭の内側に収まらない。
    EyesOutsideBody,
    /// 状態ごとに目の見え方が変わらず、状態を読み分けられない。
    StatesLookAlike,
    /// パネル線が輪郭の内側に収まらない。
    DetailOutsideBody,
    /// bar のパネル線が dock より多い（狭い bar では間引く）。
    BarDetailsNotThinned,
    /// `id` が命名規則に合わない、または既にある顔と衝突する。
    Id,
    /// **警告のみ**。bar で 6 匹並べた幅がノッチ右の見込み空きを超える。
    NotchWidth,
    /// TOML として読めない、または顔として組み立てられない。
    Parse,
}

impl ProblemCode {
    /// CLI と Web UI に出る表示名。
    pub fn as_str(self) -> &'static str {
        match self {
            ProblemCode::Outline => "outline",
            ProblemCode::Symmetry => "symmetry",
            ProblemCode::BodySize => "body-size",
            ProblemCode::EyesTooWide => "eyes-too-wide",
            ProblemCode::EyesOutsideBody => "eyes-outside-body",
            ProblemCode::StatesLookAlike => "states-look-alike",
            ProblemCode::DetailOutsideBody => "detail-outside-body",
            ProblemCode::BarDetailsNotThinned => "bar-details-not-thinned",
            ProblemCode::Id => "id",
            ProblemCode::NotchWidth => "notch-width",
            ProblemCode::Parse => "parse",
        }
    }
}

impl std::fmt::Display for ProblemCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 顔の検証・パースで見つかった問題 1 件。
///
/// 投稿者が 1 回の実行で全部直せるよう、バリデータは最初の 1 件で止めずに
/// 集めて返す。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    pub code: ProblemCode,
    pub message: String,
}

impl Problem {
    pub fn new(code: ProblemCode, message: impl Into<String>) -> Self {
        Problem {
            code,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for Problem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}] {}", self.code, self.message)
    }
}

/// この顔がどこから来たか。同 id の衝突時にどちらを採るかの判断と、
/// `ccsessions face list` の表示に使う。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// リポジトリに同梱（`include_str!`）。
    Builtin,
    /// ユーザディレクトリ（`~/.config/ccsessions/faces/*.toml`）。
    User(std::path::PathBuf),
}

impl Source {
    pub fn is_builtin(&self) -> bool {
        matches!(self, Source::Builtin)
    }
}

// ---------------------------------------------------------------------------
// 体の寸法
// ---------------------------------------------------------------------------

/// 体の矩形（pt）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BodySize {
    pub w: f64,
    pub h: f64,
}

// ---------------------------------------------------------------------------
// 輪郭
// ---------------------------------------------------------------------------

/// 輪郭の書き方。角丸長方形系と自由なパスの 2 系統。
#[derive(Debug, Clone, PartialEq)]
pub enum OutlineSpec {
    /// CSS の `border-radius` 相当（egg / round / squircle / bean）。
    Corners(CornerSpec),
    /// SVG の `d` サブセットで書いた自由なシルエット（ビルダーの `Silhouette` 系）。
    /// `segs` の座標は 0..1 比率。
    Path {
        /// `true` なら `segs` は**右半分だけ**で、左半分は `u → 1-u` の鏡像で作る。
        half: bool,
        start: (f64, f64),
        segs: Vec<Seg>,
    },
}

/// 角丸の指定。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CornerSpec {
    /// 体の寸法に対する比率。CSS と同じ順（左上・右上・右下・左下）で `[水平, 垂直]`。
    /// 水平半径は `w` に、垂直半径は `h` に掛かる。
    Ratio([[f64; 2]; 4]),
    /// pt 固定。**bar と dock で角丸の見た目を揃えたいとき**に使う（squircle）。
    /// 体の半分（`w/2` / `h/2`）でクランプする。
    Pt(BySize<f64>),
    /// カプセル（左右が半円）。4 隅とも半径が**縦横とも `h/2`**。
    ///
    /// **比率でも pt でも表せないので専用の形になっている**:
    /// - 比率だと水平半径が `h/2` なので比率は `h/(2w)` になり、bar `10/28 = 0.357` と
    ///   dock `17/44 = 0.386` で値が変わって 1 つの記述で両立しない
    /// - pt 固定だと `creature.rs` が枠線幅ぶん内側に縮めた輪郭を要求したときに
    ///   追従できない（`h` が 20 → 18.5 になっても半径 10 のままになり、
    ///   カプセルでなくなる）
    Capsule,
}

// ---------------------------------------------------------------------------
// 目
// ---------------------------------------------------------------------------

/// 目の描き方。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EyeShape {
    /// 角丸矩形（egg / round / squircle / bean）。
    Rounded,
    /// 多角形（スリットやくさびのように、弧では作れない目）。
    Polygon,
}

/// 目の色。顔が選べるのはこの 4 つだけ。
///
/// 任意の色を指定させないのは、色と状態の対応（シアン＝作業中、琥珀＝判断待ち…）が
/// 読み取りやすさの核だから。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EyeColor {
    Eye,
    EyeClosed,
    EyeError,
    White,
}

impl EyeColor {
    pub fn rgb(self) -> Rgb {
        match self {
            EyeColor::Eye => palette::EYE,
            EyeColor::EyeClosed => palette::EYE_CLOSED,
            EyeColor::EyeError => palette::EYE_ERROR,
            EyeColor::White => (1.0, 1.0, 1.0),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            EyeColor::Eye => "eye",
            EyeColor::EyeClosed => "eye_closed",
            EyeColor::EyeError => "eye_error",
            EyeColor::White => "white",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "eye" => Some(EyeColor::Eye),
            "eye_closed" => Some(EyeColor::EyeClosed),
            "eye_error" => Some(EyeColor::EyeError),
            "white" => Some(EyeColor::White),
            _ => None,
        }
    }
}

/// 状態ごとの目の上書き。**書かなかった状態には既定ルールが適用される**ので、
/// 投稿者はシルエットだけ書けば全 6 状態が成立する。
///
/// 上書きは既定ルールを**完全に置き換える**（マージしない）。ある状態に
/// `[eyes.states.X]` を 1 つでも書いたら、その状態は書いた内容 ＋ ここの既定値で決まる。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EyeOverride {
    pub w_scale: f64,
    pub h_scale: f64,
    pub dx: f64,
    pub color: EyeColor,
    pub glow: f64,
    pub blink: bool,
}

impl Default for EyeOverride {
    fn default() -> Self {
        EyeOverride {
            w_scale: 1.0,
            h_scale: 1.0,
            dx: 0.0,
            color: EyeColor::Eye,
            glow: 0.0,
            blink: false,
        }
    }
}

/// 目の定義。
#[derive(Debug, Clone, PartialEq)]
pub struct EyesSpec {
    pub shape: EyeShape,
    /// 顔の中での縦位置（体の高さに対する比率。0.5 = 中央）。
    /// あごの長い顔は 0.58 前後まで上げて目を上寄りにする。
    pub v: f64,
    /// 両目の間隔（pt）。
    pub gap: BySize<f64>,
    /// 基準の [w, h]（pt）。
    pub size: BySize<[f64; 2]>,
    /// `Rounded` のときの角丸半径（pt）。`Polygon` では使わない。
    pub radius: f64,
    /// `Polygon` のときの閉多角形。座標は**目の矩形 w×h に対する 0..1**。
    /// 右目を書き、左目は `u → 1-u` の鏡像。
    pub polygon: Option<Vec<[f64; 2]>>,
    /// 状態ごとの上書き。`SessionState::ORDER` と同じ並びで持つ
    /// （`SessionState` に `Ord` を要求せずに引けるようにするため）。
    pub states: [Option<EyeOverride>; 6],
}

/// 片目の見た目（解決済み）。`dx` は体の中心から見た水平オフセット（横目の表現）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EyeSpec {
    pub w: f64,
    pub h: f64,
    pub radius: f64,
    pub color: Rgb,
    /// 白い滲み（判断待ちの見開き目だけ）。0.0 なら無し。
    pub glow: f64,
    /// 中心からの水平ずらし（pt）。エージェント待ちの「横目」。
    pub dx: f64,
    /// 中心からの垂直ずらし（pt）。**状態ではなく顔で決まる** — 目が顔のまん中に
    /// 無い顔用（`eyes.v` から解く）。
    pub dy: f64,
    /// 瞬きアニメを付けるか（作業中のみ）。
    pub blink: bool,
}

/// 顔の `id` として使える文字列か。`^[a-z0-9][a-z0-9-]*$` の 32 文字以内。
///
/// `config.toml` の `design` の形の検査にも使う。**実在するかどうかは見ない** —
/// ユーザ顔の存在は設定のパース時点では分からないので、そちらはレジストリ解決時
/// （`Registry::resolve`）の担当。
pub fn is_valid_id(id: &str) -> bool {
    if id.is_empty() || id.len() > 32 {
        return false;
    }
    let mut chars = id.chars();
    let first = chars.next().expect("空でないことは上で確かめた");
    if !first.is_ascii_lowercase() && !first.is_ascii_digit() {
        return false;
    }
    chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// `SessionState` を `states` 配列の添字に変換する。`SessionState::ORDER` と同じ並び。
pub fn state_index(s: SessionState) -> usize {
    SessionState::ORDER
        .iter()
        .position(|&x| x == s)
        .expect("SessionState::ORDER は全 variant を含む")
}

// ---------------------------------------------------------------------------
// パネル線
// ---------------------------------------------------------------------------

/// 顔のパネル線 1 本（開いた折れ線）。座標は 0..1 比率。
#[derive(Debug, Clone, PartialEq)]
pub struct DetailSpec {
    pub name: String,
    /// この配置でだけ描く。**bar は線を間引くのが定石**
    /// （18×20pt に 7 本引くと潰れて塊になる — 実測）。
    pub sizes: Vec<Size>,
    pub points: Vec<[f64; 2]>,
}

// ---------------------------------------------------------------------------
// FaceSpec
// ---------------------------------------------------------------------------

/// 顔 1 つぶんの定義。`faces/*.toml` 1 ファイルがこれに対応する。
#[derive(Debug, Clone, PartialEq)]
pub struct FaceSpec {
    pub id: String,
    pub label: String,
    pub label_en: Option<String>,
    pub author: Option<String>,
    pub size: BySize<BodySize>,
    pub outline: OutlineSpec,
    pub eyes: EyesSpec,
    pub details: Vec<DetailSpec>,
    pub source: Source,
}

impl FaceSpec {
    /// 体の矩形（pt）。
    pub fn body_size(&self, size: Size) -> (f64, f64) {
        let s = self.size.get(size);
        (s.w, s.h)
    }

    /// 角丸プロファイルを体の寸法に対して解決する。`OutlineSpec::Path` の顔では
    /// 使われない（`outline_of` が分岐する）。
    pub fn corners(&self, w: f64, h: f64, size: Size) -> Corners {
        match &self.outline {
            OutlineSpec::Corners(CornerSpec::Ratio(c)) => [
                (c[0][0] * w, c[0][1] * h),
                (c[1][0] * w, c[1][1] * h),
                (c[2][0] * w, c[2][1] * h),
                (c[3][0] * w, c[3][1] * h),
            ],
            OutlineSpec::Corners(CornerSpec::Pt(pt)) => {
                // 旧 `theme::corners` の squircle と同じクランプ。
                let r = pt.get(size);
                let r = (r.min(w * 0.5), r.min(h * 0.5));
                [r, r, r, r]
            }
            // 旧 `theme::corners` の bean。**渡された `h` に追従する**のが要点。
            OutlineSpec::Corners(CornerSpec::Capsule) => {
                let r = (h * 0.5, h * 0.5);
                [r, r, r, r]
            }
            // パス系に角丸を求められたら squircle 相当へ倒しておく
            // （呼び出し側で panic させる価値はない — 旧 `theme::corners` と同じ判断）。
            OutlineSpec::Path { .. } => {
                let r: f64 = if size.is_bar() { 7.0 } else { 10.0 };
                let r = (r.min(w * 0.5), r.min(h * 0.5));
                [r, r, r, r]
            }
        }
    }

    /// 体の輪郭を作る。ここが唯一の入口。
    ///
    /// `w` / `h` を明示的に受けるのは、`creature.rs` が枠線幅の半分だけ内側に縮めた
    /// 輪郭を要求するため（ストロークがパス上に中心を置くので、そうしないと枠が
    /// 体の矩形からはみ出す）。
    pub fn outline_of(&self, w: f64, h: f64, size: Size) -> Outline {
        match &self.outline {
            OutlineSpec::Corners(_) => outline(w, h, self.corners(w, h, size)),
            OutlineSpec::Path { half, start, segs } => path_outline(*half, *start, segs, w, h),
        }
    }

    /// 体の寸法そのままの輪郭。検証・SVG から使う。
    pub fn body_outline(&self, size: Size) -> Outline {
        let (w, h) = self.body_size(size);
        self.outline_of(w, h, size)
    }

    /// 両目の間隔（pt）。
    pub fn eye_gap(&self, size: Size) -> f64 {
        self.eyes.gap.get(size)
    }

    /// 状態と配置から目の仕様を決める。
    ///
    /// `[eyes.states.*]` を書いた状態はその上書きで、書かなかった状態は
    /// `shape` ごとの既定ルール（`default_eye`）で決まる。
    pub fn eye(&self, state: SessionState, size: Size) -> EyeSpec {
        let (_, bh) = self.body_size(size);
        let [w0, h0] = self.eyes.size.get(size);
        // 多角形の目は `eye_shape` のパスで描くので角丸半径は使わない。
        let r0 = match self.eyes.shape {
            EyeShape::Rounded => self.eyes.radius,
            EyeShape::Polygon => 0.0,
        };
        let dy = (self.eyes.v - 0.5) * bh;

        match self.eyes.states[state_index(state)] {
            Some(o) => EyeSpec {
                w: w0 * o.w_scale,
                h: h0 * o.h_scale,
                radius: r0,
                color: o.color.rgb(),
                glow: o.glow,
                dx: o.dx,
                dy,
                blink: o.blink,
            },
            None => default_eye(self.eyes.shape, state, w0, h0, r0, dy),
        }
    }

    /// 目を多角形で描く顔なら、その閉多角形（目の矩形 `w`×`h` のローカル座標）。
    /// `None` なら角丸矩形で描く。
    ///
    /// `mirrored` は左目用（内と外が逆になる）。
    pub fn eye_shape(&self, w: f64, h: f64, mirrored: bool) -> Option<Vec<(f64, f64)>> {
        let poly = self.eyes.polygon.as_ref()?;
        Some(
            poly.iter()
                .map(|&[u, v]| ((if mirrored { 1.0 - u } else { u }) * w, v * h))
                .collect(),
        )
    }

    /// 顔のパネル線。開いた折れ線の集まりで、座標は体の矩形ローカル（pt）。
    /// 線画を持たない顔では空。
    pub fn face_details(&self, w: f64, h: f64, size: Size) -> Vec<Vec<(f64, f64)>> {
        self.details
            .iter()
            .filter(|d| d.sizes.contains(&size))
            .map(|d| d.points.iter().map(|&[u, v]| (u * w, v * h)).collect())
            .collect()
    }

    /// メニューやギャラリーに出す表示名。
    pub fn display_label(&self) -> &str {
        &self.label
    }
}

// ---------------------------------------------------------------------------
// 解決の下請け
// ---------------------------------------------------------------------------

/// `kind = "path"` の輪郭を pt へ起こす。
///
/// `half` なら右半分を**逆順に辿りながら u を反転**して左半分にする。逆走なので
/// ベジェの制御点も入れ替える。
fn path_outline(half: bool, start: (f64, f64), raw: &[Seg], w: f64, h: f64) -> Outline {
    let s = |(u, v): (f64, f64)| (u * w, v * h);
    let m = |(u, v): (f64, f64)| ((1.0 - u) * w, v * h);

    let mut segs = Vec::with_capacity(if half { raw.len() * 2 } else { raw.len() });
    for seg in raw {
        segs.push(match *seg {
            Seg::Line { to } => Seg::Line { to: s(to) },
            Seg::Cubic { c1, c2, to } => Seg::Cubic {
                c1: s(c1),
                c2: s(c2),
                to: s(to),
            },
        });
    }

    if half {
        // 逆走のために各手の**始点**を控える（`heads[i]` が `raw[i]` の始点）。
        let mut heads = vec![start];
        for seg in raw {
            heads.push(seg_to(*seg));
        }
        for i in (0..raw.len()).rev() {
            segs.push(match raw[i] {
                Seg::Line { .. } => Seg::Line { to: m(heads[i]) },
                Seg::Cubic { c1, c2, .. } => Seg::Cubic {
                    c1: m(c2),
                    c2: m(c1),
                    to: m(heads[i]),
                },
            });
        }
    }

    Outline {
        start: s(start),
        segs,
    }
}

/// `[eyes.states.*]` を書かなかった状態の既定ルール。
///
/// **投稿者がシルエットだけ書けば 6 状態すべてが成立する**ための土台で、
/// 組込みの egg / round / squircle / bean は `[eyes.states.*]` を一行も書いていない。
///
/// `Rounded` の `wait_user`（正円化）と `idle`（高さ 2pt の横線）だけは
/// `w_scale` / `h_scale` の語彙では表せないので、ルールとしてコードに持つ —
/// 前者は高さが「元の h」ではなく「元の w + 1」になり bar と dock で倍率が違い、
/// 後者は絶対値 2.0 だから。
fn default_eye(
    shape: EyeShape,
    state: SessionState,
    w: f64,
    h: f64,
    radius: f64,
    dy: f64,
) -> EyeSpec {
    let base = EyeSpec {
        w,
        h,
        radius,
        color: palette::EYE,
        glow: 0.0,
        dx: 0.0,
        dy,
        blink: false,
    };
    match (shape, state) {
        (_, SessionState::Working) => EyeSpec {
            blink: true,
            ..base
        },
        (_, SessionState::WaitAgent) => EyeSpec { dx: 1.5, ..base },
        (_, SessionState::Done) => base,
        (_, SessionState::Error) => EyeSpec {
            color: palette::EYE_ERROR,
            ..base
        },

        // 見開き：正円 + 白い滲み
        (EyeShape::Rounded, SessionState::WaitUser) => EyeSpec {
            w: w + 1.0,
            h: w + 1.0,
            radius: (w + 1.0) / 2.0,
            color: (1.0, 1.0, 1.0),
            glow: 4.0,
            ..base
        },
        // 閉じ目：横線
        (EyeShape::Rounded, SessionState::Idle) => EyeSpec {
            w: w + 1.0,
            h: 2.0,
            radius: 1.0,
            color: palette::EYE_CLOSED,
            ..base
        },

        // 多角形は「正円化」が使えないので、縦の見開き / 絞りに翻訳する。
        (EyeShape::Polygon, SessionState::WaitUser) => EyeSpec {
            h: h * 1.45,
            color: (1.0, 1.0, 1.0),
            glow: 4.0,
            ..base
        },
        (EyeShape::Polygon, SessionState::Idle) => EyeSpec {
            h: h * 0.4,
            color: palette::EYE_CLOSED,
            ..base
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `half = true` の展開が閉じた左右対称の輪郭になる
    /// （逆走時に制御点を入れ替えている根拠の固定）。
    #[test]
    fn half_path_mirrors_into_a_closed_symmetric_outline() {
        let start = (0.5, 0.0);
        let raw = vec![
            Seg::Line { to: (0.8, 0.0) },
            Seg::Cubic {
                c1: (0.9, 0.2),
                c2: (1.0, 0.6),
                to: (0.5, 1.0),
            },
        ];
        let o = path_outline(true, start, &raw, 10.0, 20.0);

        // 鏡像は `(1 - u) * w` なので丸め誤差が乗る。厳密比較はしない。
        let near = |a: (f64, f64), b: (f64, f64), what: &str| {
            assert!(
                (a.0 - b.0).abs() < 1e-9 && (a.1 - b.1).abs() < 1e-9,
                "{what}: {a:?} != {b:?}"
            );
        };

        assert_eq!(o.start, (5.0, 0.0));
        assert_eq!(o.segs.len(), 4, "右半分 2 手 + 左半分 2 手");
        // 最後の手は始点に戻る。
        near(seg_to(*o.segs.last().unwrap()), (5.0, 0.0), "始点に戻る");
        // 逆走なので制御点が入れ替わっている。
        match o.segs[2] {
            Seg::Cubic { c1, c2, to } => {
                near(c1, (0.0, 12.0), "元の c2 が鏡像で c1 になる");
                near(c2, (1.0, 4.0), "元の c1 が鏡像で c2 になる");
                near(to, (2.0, 0.0), "元の手の始点の鏡像へ戻る");
            }
            other => panic!("3 手目は Cubic のはず: {other:?}"),
        }
    }

    /// `half = false` なら書いたパスがそのまま使われる。
    #[test]
    fn full_path_is_used_as_written() {
        let raw = vec![
            Seg::Line { to: (1.0, 0.0) },
            Seg::Line { to: (1.0, 1.0) },
            Seg::Line { to: (0.0, 0.0) },
        ];
        let o = path_outline(false, (0.0, 0.0), &raw, 10.0, 10.0);
        assert_eq!(o.segs.len(), 3);
        assert_eq!(seg_to(o.segs[1]), (10.0, 10.0));
    }

    /// pt 指定の角丸は体の半分でクランプされる（旧 squircle / bean と同じ規則）。
    #[test]
    fn corners_pt_is_clamped_to_half_the_body() {
        let f = face_with_outline(OutlineSpec::Corners(CornerSpec::Pt(BySize {
            bar: 10.0,
            dock: 17.0,
        })));
        // bar: 28×20 に 10pt → 水平は 14 でクランプされないが垂直は 10 ちょうど。
        assert_eq!(f.corners(28.0, 20.0, Size::Bar), [(10.0, 10.0); 4]);
        // 体より大きい半径はクランプされる。
        assert_eq!(f.corners(12.0, 8.0, Size::Bar), [(6.0, 4.0); 4]);
    }

    /// 比率指定の角丸は体の寸法に掛かる。
    #[test]
    fn corners_ratio_scales_with_the_body() {
        let f = face_with_outline(OutlineSpec::Corners(CornerSpec::Ratio([
            [0.50, 0.58],
            [0.50, 0.58],
            [0.48, 0.42],
            [0.48, 0.42],
        ])));
        let c = f.corners(22.0, 20.0, Size::Bar);
        assert_eq!(c[0], (11.0, 11.6));
        assert_eq!(c[2], (22.0 * 0.48, 20.0 * 0.42));
    }

    /// `Rounded` の既定ルールが旧 `theme::eye`（角丸系）と一致する。
    #[test]
    fn rounded_default_states_match_the_legacy_rules() {
        let f = face_with_eyes(EyeShape::Rounded, [3.0, 4.0], [4.0, 6.0], 2.0, None);

        // bar
        let bar = Size::Bar;
        assert!(f.eye(SessionState::Working, bar).blink);
        let wu = f.eye(SessionState::WaitUser, bar);
        assert_eq!((wu.w, wu.h, wu.radius), (4.0, 4.0, 2.0), "正円化");
        assert_eq!(wu.glow, 4.0);
        assert_eq!(f.eye(SessionState::WaitAgent, bar).dx, 1.5);
        let idle = f.eye(SessionState::Idle, bar);
        assert_eq!((idle.w, idle.h, idle.radius), (4.0, 2.0, 1.0), "横線");
        assert_eq!(idle.color, palette::EYE_CLOSED);
        assert_eq!(f.eye(SessionState::Error, bar).color, palette::EYE_ERROR);

        // dock は正円の直径が w+1 = 5 になり、半径も 2.5 へ動く。
        let wu_d = f.eye(SessionState::WaitUser, Size::Dock);
        assert_eq!((wu_d.w, wu_d.h, wu_d.radius), (5.0, 5.0, 2.5));
        let idle_d = f.eye(SessionState::Idle, Size::Dock);
        assert_eq!((idle_d.w, idle_d.h, idle_d.radius), (5.0, 2.0, 1.0));
    }

    /// `Polygon` の既定は縦の開き具合に翻訳される（正円化は使えない）。
    #[test]
    fn polygon_default_states_open_and_squint_vertically() {
        let f = face_with_eyes(
            EyeShape::Polygon,
            [5.5, 3.0],
            [9.0, 4.6],
            2.0,
            Some(vec![[0.0, 0.0], [0.0, 0.62], [1.0, 1.0], [0.94, 0.38]]),
        );
        let bar = Size::Bar;
        let base = f.eye(SessionState::Done, bar).h;
        assert!(
            f.eye(SessionState::WaitUser, bar).h > base,
            "見開いていない"
        );
        assert!(f.eye(SessionState::Idle, bar).h < base, "絞っていない");
        // 多角形の目は角丸半径を使わない。
        assert_eq!(f.eye(SessionState::Done, bar).radius, 0.0);
    }

    /// 上書きは既定ルールを完全に置き換える（マージしない）。
    #[test]
    fn an_override_replaces_the_default_rule_entirely() {
        let mut f = face_with_eyes(EyeShape::Rounded, [3.0, 4.0], [4.0, 6.0], 2.0, None);
        // 既定では Idle は横線（4, 2.0）だが、上書きすれば倍率だけで決まる。
        f.eyes.states[state_index(SessionState::Idle)] = Some(EyeOverride {
            h_scale: 0.5,
            color: EyeColor::EyeClosed,
            ..EyeOverride::default()
        });
        let e = f.eye(SessionState::Idle, Size::Bar);
        assert_eq!(
            (e.w, e.h),
            (3.0, 2.0),
            "w は倍率 1.0 のまま、h は 4.0 * 0.5"
        );
        assert_eq!(e.color, palette::EYE_CLOSED);
    }

    /// `eyes.v` が目の縦位置（dy）になる。
    #[test]
    fn eye_v_becomes_a_vertical_offset() {
        let mut f = face_with_eyes(EyeShape::Rounded, [3.0, 4.0], [4.0, 6.0], 2.0, None);
        assert_eq!(f.eye(SessionState::Done, Size::Bar).dy, 0.0, "既定は中央");
        f.eyes.v = 0.58;
        // bar の体高は下のヘルパで 20.0。
        assert!((f.eye(SessionState::Done, Size::Bar).dy - 1.6).abs() < 1e-9);
    }

    /// パネル線は `sizes` で配置ごとに間引ける。
    #[test]
    fn details_are_filtered_by_size() {
        let mut f = face_with_outline(OutlineSpec::Corners(CornerSpec::Ratio([[0.5, 0.5]; 4])));
        f.details = vec![
            DetailSpec {
                name: "brow".into(),
                sizes: vec![Size::Bar, Size::Dock],
                points: vec![[0.2, 0.7], [0.8, 0.7]],
            },
            DetailSpec {
                name: "forehead".into(),
                sizes: vec![Size::Dock],
                points: vec![[0.4, 0.8], [0.6, 0.8]],
            },
        ];
        assert_eq!(f.face_details(10.0, 10.0, Size::Bar).len(), 1);
        assert_eq!(f.face_details(10.0, 10.0, Size::Dock).len(), 2);
        // 比率が pt に変換されている。
        assert_eq!(f.face_details(10.0, 10.0, Size::Bar)[0][0], (2.0, 7.0));
    }

    /// 目の多角形は左右が鏡像。
    #[test]
    fn eye_polygon_mirrors_for_the_left_eye() {
        let f = face_with_eyes(
            EyeShape::Polygon,
            [5.5, 3.0],
            [9.0, 4.6],
            2.0,
            Some(vec![[0.0, 0.0], [1.0, 1.0]]),
        );
        let r = f.eye_shape(10.0, 4.0, false).unwrap();
        let l = f.eye_shape(10.0, 4.0, true).unwrap();
        assert_eq!(r[0], (0.0, 0.0));
        assert_eq!(l[0], (10.0, 0.0));
        // 角丸で描く顔はパスを持たない。
        let rounded = face_with_eyes(EyeShape::Rounded, [3.0, 4.0], [4.0, 6.0], 2.0, None);
        assert!(rounded.eye_shape(10.0, 4.0, false).is_none());
    }

    // ---- テスト用のヘルパ -------------------------------------------------

    fn face_with_outline(outline: OutlineSpec) -> FaceSpec {
        FaceSpec {
            id: "test".into(),
            label: "テスト".into(),
            label_en: None,
            author: None,
            size: BySize {
                bar: BodySize { w: 22.0, h: 20.0 },
                dock: BodySize { w: 36.0, h: 34.0 },
            },
            outline,
            eyes: EyesSpec {
                shape: EyeShape::Rounded,
                v: 0.5,
                gap: BySize {
                    bar: 3.0,
                    dock: 5.0,
                },
                size: BySize {
                    bar: [3.0, 4.0],
                    dock: [4.0, 6.0],
                },
                radius: 2.0,
                polygon: None,
                states: [None; 6],
            },
            details: Vec::new(),
            source: Source::Builtin,
        }
    }

    fn face_with_eyes(
        shape: EyeShape,
        bar: [f64; 2],
        dock: [f64; 2],
        radius: f64,
        polygon: Option<Vec<[f64; 2]>>,
    ) -> FaceSpec {
        let mut f = face_with_outline(OutlineSpec::Corners(CornerSpec::Ratio([[0.5, 0.5]; 4])));
        f.eyes.shape = shape;
        f.eyes.size = BySize { bar, dock };
        f.eyes.radius = radius;
        f.eyes.polygon = polygon;
        f
    }
}
