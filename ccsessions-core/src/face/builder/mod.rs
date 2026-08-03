//! キャラクタービルダー — パーツの組み合わせから顔を作る層。
//!
//! ゲームのキャラクリエイター風の Web UI（`ccsessions ui` の「キャラクター」）の中身。
//! **顔の形式は増やさない** — 出てくるのは今までどおり `faces/*.toml` で、
//! ビルダーは「手で数値を詰める代わりに選ぶ」ための入口にすぎない。
//!
//! ```text
//!   CharacterConfig (JSON)      ← UI の状態そのもの。保存・読み込みの単位
//!         │  compose
//!         ▼
//!   Draft → TOML テキスト        ← 保存されるファイルそのもの（emit.rs）
//!         │  parse::parse
//!         ▼
//!   FaceSpec → validate / svg   ← 既存の検証とプレビューをそのまま通る
//! ```
//!
//! # 3 つの設計判断
//!
//! **1. TOML テキストを唯一の中間形にする。** `FaceSpec` を直接組まないのは、
//! プレビューと保存されるファイルが食い違う余地を消すため（`emit.rs` の doc）。
//!
//! **2. 色は「状態ごとに」選ばせる。** 守りたい一線は「色を変えられないこと」
//! ではなく**「生き物を見れば状態が分かること」**なので、6 状態に一律で効く色は
//! 用意せず、状態ごとに枠・面・目を選ばせる。全状態を同じ色で塗ることもできるが、
//! それは 6 回選んだ結果であって、UI が黙ってそうするわけではない。
//! 書かなかった状態は既定パレットのままなので、色を触らなければ見た目は変わらない。
//!
//! **3. どの組み合わせでも壊れないことを構造で担保する。** 30×30×30… の
//! 組み合わせを人手で確認するのは無理なので、
//! - パネル線の幅は**その高さで顔がどれだけ広いか**に比例させ（`shape::half_width_at`）、
//! - 目は**検証器そのものを使って**収まるまで自動で縮め（`fit_eyes`）、
//! - bar のパネル線は dock の部分集合になるよう構造的に縛る。
//!
//! 番人は `tests::every_part_composes_into_a_valid_face`。

pub mod emit;
pub mod parts;
pub mod shape;

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::face::builder::emit::{DetailDraft, Draft, OutlineDraft};
use crate::face::builder::parts::{EyeForm, Form};
use crate::face::palette;
use crate::face::spec::{is_valid_id, Problem, ProblemCode, Source, StateColors};
use crate::face::{validate, FaceSpec};
use crate::session::SessionState;

/// 設定ファイルのバージョン。**互換性を壊す変更を入れたら上げる。**
pub const CONFIG_VERSION: u32 = 1;

/// 顔が読めなくなったときに使う最後の砦の id。
const FALLBACK_ID: &str = "draft";

// ---------------------------------------------------------------------------
// CharacterConfig
// ---------------------------------------------------------------------------

/// パーツごとの微調整。**意味はカテゴリで変わる**ので、UI 側もカテゴリ別に
/// 出すつまみを選ぶ。
///
/// | フィールド | 顔 | 目 | 線（髪・鼻・口） | 線（左右 1 対） |
/// |---|---|---|---|---|
/// | `scale` | 体の幅 | 目の大きさ | 線の幅 | 線の幅 |
/// | `dy` | — | 縦位置（比率で加算） | 縦位置 | 縦位置 |
/// | `dx` | — | — | 横位置 | **左右の開き** |
/// | `gap` | — | 両目の間隔 | — | — |
/// | `bar` | — | — | bar にも描くか（未指定ならカテゴリの既定） | 同左 |
///
/// **対のパーツだけ `dx` の意味が違う**（横位置ではなく左右の開き）のは、
/// 両方を同じ向きに動かすと生き物が傾いて見えるため。実装上は右側にだけ
/// `dx` を掛けてから鏡像を取るので、対称性が崩れようがない（`line_details`）。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Tweak {
    pub dx: f64,
    pub dy: f64,
    pub scale: f64,
    pub gap: f64,
    pub bar: Option<bool>,
}

impl Default for Tweak {
    fn default() -> Self {
        Tweak {
            dx: 0.0,
            dy: 0.0,
            scale: 1.0,
            gap: 1.0,
            bar: None,
        }
    }
}

impl Tweak {
    /// 常識的な範囲へ丸める。**UI から任意の数が来る前提**で、
    /// 極端な値のせいで「何をどう直せば検証を通るのか分からない顔」を作らせない。
    fn clamped(self) -> Tweak {
        Tweak {
            dx: self.dx.clamp(-0.25, 0.25),
            dy: self.dy.clamp(-0.25, 0.25),
            scale: if self.scale.is_finite() {
                self.scale.clamp(0.3, 2.0)
            } else {
                1.0
            },
            gap: if self.gap.is_finite() {
                self.gap.clamp(0.2, 3.0)
            } else {
                1.0
            },
            bar: self.bar,
        }
    }
}

/// プレビューの見え方。**顔の一部ではない**（保存はするが TOML には出ない）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Preview {
    /// `SessionState` の名前。色とアニメはここで決まる。
    pub state: String,
    /// "bar" か "dock"。
    pub size: String,
}

impl Default for Preview {
    fn default() -> Self {
        Preview {
            state: "working".into(),
            size: "dock".into(),
        }
    }
}

/// キャラクター 1 体ぶんの設定。**UI の状態と 1:1**。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct CharacterConfig {
    pub version: u32,
    /// 顔の id（`[a-z0-9-]+`）。ファイル名にもなる。
    pub id: String,
    /// 表示名（TOML の `label`）。
    pub name: String,
    pub author: Option<String>,
    /// カテゴリ id → パーツ id。
    ///
    /// **添字ではなく文字列で持つ**のが要点。番号にすると、パーツを 1 つ挿した
    /// だけで既存の保存済みキャラクターが全部別人になる。
    pub parts: BTreeMap<String, String>,
    /// カテゴリ id → 微調整。書かなかったカテゴリは既定値。
    pub tweaks: BTreeMap<String, Tweak>,
    /// 状態名（`"working"` など）→ その状態の色。
    ///
    /// **書いた状態だけが顔の TOML に出る。** 触っていない状態は既定パレットの
    /// ままなので、色を選ばずに作った顔の見た目は 0.2.0 と変わらない。
    pub colors: BTreeMap<String, StateColorCfg>,
    /// 旧「目の色」（`"eye"` / `"eye_closed"` / `"eye_error"` / `"white"`）。
    ///
    /// **読むためだけに残してある。** 0.2.0 までのビルダーで保存した顔を開き直したとき、
    /// 選んでいた目の色が黙って消えないようにする（`legacy_eye_colors`）。
    /// 新しく書き出すことはない。
    pub eye_color: String,
    pub preview: Preview,
}

/// 1 状態ぶんの色。**空文字は「指定なし」**（＝既定パレットに任せる）。
///
/// `Option<String>` ではなく空文字にしてあるのは、画面が「色を消す」を空文字の
/// 送信で表すため。どちらか一方の表現に寄せておかないと、`null` と `""` の
/// 両方を受ける羽目になる。読めない値も指定なしとして扱う（`parse_color`）ので、
/// ここに何が入っていても顔は組み上がる。
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct StateColorCfg {
    /// 枠・グロー・状態記号・バッジ・名前・ホバーカードの縁。
    pub accent: String,
    /// 体の面の塗り。
    pub fill: String,
    /// 目。
    pub eye: String,
}

impl Default for CharacterConfig {
    fn default() -> Self {
        let mut parts = BTreeMap::new();
        // 「最初から顔らしい顔が出ている」ほうが手が動くので、
        // 素の輪郭 1 つではなく髪・鼻・口の付いた組み合わせを初期値にする。
        parts.insert("face".to_string(), "egg".to_string());
        parts.insert("eyes".to_string(), "bead".to_string());
        parts.insert("hair".to_string(), "round".to_string());
        parts.insert("nose".to_string(), "dot".to_string());
        parts.insert("mouth".to_string(), "smile".to_string());
        CharacterConfig {
            version: CONFIG_VERSION,
            id: "my-face".into(),
            name: "わたしの顔".into(),
            author: None,
            parts,
            tweaks: BTreeMap::new(),
            colors: BTreeMap::new(),
            eye_color: String::new(),
            preview: Preview::default(),
        }
    }
}

impl CharacterConfig {
    /// カテゴリのパーツ id。未指定ならそのカテゴリの既定（先頭）。
    pub fn part(&self, category: &str) -> &str {
        self.parts.get(category).map(String::as_str).unwrap_or("")
    }

    /// 状態の色（読めない値と空文字は「指定なし」に潰し済み）。
    pub fn state_colors(&self, state: SessionState) -> StateColors {
        let c = self.colors.get(state.as_str());
        let pick = |f: fn(&StateColorCfg) -> &str| {
            c.map(f)
                .filter(|v| !v.is_empty())
                .and_then(palette::parse_hex)
        };
        let mut out = StateColors {
            accent: pick(|c| &c.accent),
            fill: pick(|c| &c.fill),
            eye: pick(|c| &c.eye),
        };
        // 旧「目の色」を引き継ぐ。**新しい指定が無い状態にだけ**当てる。
        if out.eye.is_none() {
            out.eye = self.legacy_eye_color(state);
        }
        out
    }

    /// 0.2.0 までの `eye_color`（4 値の名前）を、その状態の目の色に読み替える。
    ///
    /// 当時も**既定の色が `eye` である 3 状態にしか効いていなかった**
    /// （見開きの白・アイドルの暗色・エラーの赤は塗り替えない、という判断）ので、
    /// 読み替えも同じ 3 状態に閉じる。そうしないと、開き直した瞬間に顔が変わる。
    fn legacy_eye_color(&self, state: SessionState) -> Option<crate::face::palette::Rgb> {
        if !matches!(
            state,
            SessionState::Working | SessionState::WaitAgent | SessionState::Done
        ) {
            return None;
        }
        match self.eye_color.as_str() {
            "" | "eye" => None,
            other => crate::face::EyeColor::parse(other).map(|c| c.rgb()),
        }
    }

    /// カテゴリの微調整（常識的な範囲へ丸め済み）。
    pub fn tweak(&self, category: &str) -> Tweak {
        self.tweaks
            .get(category)
            .copied()
            .unwrap_or_default()
            .clamped()
    }

    /// JSON から読む。
    pub fn from_json(s: &str) -> Result<Self, String> {
        serde_json::from_str(s).map_err(|e| format!("設定を読めません: {e}"))
    }

    /// 人が読める JSON にする（ダウンロード用）。
    pub fn to_json_pretty(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".into())
    }
}

// ---------------------------------------------------------------------------
// compose
// ---------------------------------------------------------------------------

/// 組み立ての結果。**失敗しない** — 問題があっても「何か」は描けるようにして、
/// UI が真っ白にならないようにする（`Registry::resolve` と同じ方針）。
#[derive(Debug, Clone)]
pub struct Composed {
    pub spec: FaceSpec,
    /// 保存されるファイルそのもの。
    pub toml: String,
    /// 検証とパースの問題。空なら保存してよい。
    pub problems: Vec<Problem>,
    /// `notch-width`（bar で 6 匹並べるとノッチ右に入らない）。警告なので保存は妨げない。
    pub warning: Option<Problem>,
    /// 目を自動で縮めた倍率。1.0 なら無調整。
    pub eye_fit: f64,
}

/// 目を収めるために縮める回数と 1 回ぶんの倍率。
///
/// 0.93^10 ≈ 0.48 まで縮む。ここまでやって収まらない顔は、目ではなく
/// 輪郭のほうがおかしい（`outline` が別に報告する）。
const EYE_FIT_STEPS: usize = 10;
const EYE_FIT_SHRINK: f64 = 0.93;

/// 設定からキャラクターを組み立てる。
pub fn compose(cfg: &CharacterConfig) -> Composed {
    // 1. 目が収まる倍率を、検証器そのものを使って探す。
    //    パネル線は目の収まりに無関係なので、この段階では付けない。
    let mut fit = 1.0;
    let mut draft = build(cfg, fit, &[]);
    for _ in 0..EYE_FIT_STEPS {
        let bad = match validate::validate(&draft.1) {
            Ok(()) => false,
            Err(ps) => ps.iter().any(|p| {
                p.code == ProblemCode::EyesOutsideBody || p.code == ProblemCode::EyesTooWide
            }),
        };
        if !bad {
            break;
        }
        fit *= EYE_FIT_SHRINK;
        draft = build(cfg, fit, &[]);
    }

    // 2. 決まった輪郭に合わせてパネル線を置く。
    let details = line_details(cfg, &draft.1);
    let (toml, spec, mut problems) = build(cfg, fit, &details);

    // パースできなかった場合、検証はフォールバックの顔に対して走っているので
    // そちらの結果は当てにならない。**保存しようとしているテキストの問題**
    // （id 等）だけを報告する。
    if problems.is_empty() {
        problems = validate::validate(&spec).err().unwrap_or_default();
    }
    let warning = validate::notch_width_warning(&spec);
    Composed {
        spec,
        toml,
        problems,
        warning,
        eye_fit: fit,
    }
}

/// 設定 → TOML テキスト → `FaceSpec`。返すのは
/// （保存されるテキスト, 描ける顔, パースの問題）。
///
/// パースに失敗したら id と label だけ差し替えて**描けるところまで戻す**
/// （`id` の綴りが不正なだけでプレビューが消えると、何を直せばいいのか
/// 分からなくなる）。返すテキストは差し替える前のもの — UI が保存しようと
/// しているのはそちらで、問題もそちらに対して報告する。
fn build(
    cfg: &CharacterConfig,
    eye_fit: f64,
    details: &[DetailDraft],
) -> (String, FaceSpec, Vec<Problem>) {
    let draft = draft_of(cfg, eye_fit, details);
    let text = emit::to_toml(&draft);
    let problems = match crate::face::parse::parse(&text, Source::Builtin) {
        Ok(spec) => return (text, spec, Vec::new()),
        Err(ps) => ps,
    };

    let mut safe = draft;
    safe.id = FALLBACK_ID.to_string();
    if safe.label.trim().is_empty() {
        safe.label = "（名前なし）".to_string();
    }
    let fallback = emit::to_toml(&safe);
    match crate::face::parse::parse(&fallback, Source::Builtin) {
        Ok(spec) => (text, spec, problems),
        Err(_) => {
            unreachable!("id と label を既定へ戻しても組み立てられない顔がある（emit のバグ）")
        }
    }
}

fn draft_of(cfg: &CharacterConfig, eye_fit: f64, details: &[DetailDraft]) -> Draft {
    let fp = parts::face(cfg.part("face"));
    let ep = parts::eyes(cfg.part("eyes"));
    let ft = cfg.tweak("face");
    let et = cfg.tweak("eyes");

    // ── 体の寸法 ──
    let bar_w = (fp.w * ft.scale).clamp(12.0, 30.0);
    let dock_w = parts::dock_w(bar_w);

    // ── 輪郭 ──
    let outline = match fp.form {
        Form::Corners { top, bottom } => OutlineDraft::Corners([top, top, bottom, bottom]),
        Form::Capsule => OutlineDraft::Capsule,
        Form::Silhouette(s) => {
            let pts = parts::silhouette_points(&s);
            let segs = shape::smooth_path(&pts);
            OutlineDraft::Path {
                half: true,
                d: emit::path_d(pts[0], &segs),
            }
        }
    };

    // ── 目 ──
    let scale = et.scale * eye_fit;
    let (ew, eh) = (ep.w * scale, ep.h * scale);
    let gap = ep.gap * et.gap * scale;
    let (eye_polygon, radius) = match ep.form {
        EyeForm::Rounded { radius } => (None, radius * ew.min(eh)),
        EyeForm::Lids {
            inner,
            outer,
            upper,
            lower,
        } => (Some(shape::eye_polygon(inner, outer, upper, lower)), 0.0),
        EyeForm::Wedge {
            inner_lo,
            inner_hi,
            outer_lo,
            outer_hi,
        } => (
            Some(shape::wedge_polygon(inner_lo, inner_hi, outer_lo, outer_hi)),
            0.0,
        ),
        EyeForm::Horn { up } => (Some(shape::horn_polygon(up)), 0.0),
    };
    let eye_v = (fp.eye_v + et.dy).clamp(0.2, 0.8);

    // ── 状態ごとの色 ──
    // 選んでいない状態は空のまま ＝ `[colors.*]` に行が出ず、既定パレットで描かれる。
    let mut colors = [StateColors::default(); 6];
    for state in SessionState::ORDER {
        colors[crate::face::spec::state_index(state)] = cfg.state_colors(state);
    }

    Draft {
        id: cfg.id.trim().to_string(),
        label: cfg.name.trim().to_string(),
        author: cfg.author.clone(),
        bar: (bar_w, parts::BAR_H),
        dock: (dock_w, parts::DOCK_H),
        outline,
        eye_polygon,
        eye_v,
        eye_gap: (gap, gap * parts::DOCK_EYE_RATIO),
        eye_size: (
            [ew, eh],
            [ew * parts::DOCK_EYE_RATIO, eh * parts::DOCK_EYE_RATIO],
        ),
        eye_radius: radius,
        colors,
        details: details.to_vec(),
        notes: notes_of(cfg),
        config_json: emit::inline_json(cfg),
    }
}

/// 先頭コメントに載せる「どのパーツで出来ているか」。
fn notes_of(cfg: &CharacterConfig) -> Vec<String> {
    let mut v = vec![format!(
        "顔のライン: {} / 目: {}",
        parts::face(cfg.part("face")).label,
        parts::eyes(cfg.part("eyes")).label
    )];
    let list: Vec<String> = parts::LINES
        .iter()
        // パーツ名は日本語しか無い（画面に出さないので訳していない）。この行は
        // 生成した TOML の先頭コメントなので、カテゴリ名も日本語側で揃える。
        .map(|c| {
            format!(
                "{}: {}",
                c.label.ja,
                parts::line_part(c, cfg.part(c.id)).label
            )
        })
        .collect();
    v.push(list.join(" / "));
    v
}

/// 線カテゴリをパネル線に落とす。
///
/// 幅はその高さでの顔の半幅に比例させるので、輪郭を替えても線が顔からはみ出さない
/// （`shape::place`）。
///
/// `mirror` のパーツは **`<cat>-r` と `<cat>-l` の 2 本**になる。左は右で計算した
/// 点を x → 1-x で折り返したもので、**別々に計算しない** — 同じ数式を 2 回通すと
/// 丸めで左右が 1 単位ずれ、生き物が傾いて見えるため。
fn line_details(cfg: &CharacterConfig, spec: &FaceSpec) -> Vec<DetailDraft> {
    // 輪郭の断面表は 1 回だけ作る（`Profile` の doc 参照）。
    let profile = shape::Profile::of(spec);
    let mut out = Vec::new();
    for cat in parts::LINES {
        let part = parts::line_part(cat, cfg.part(cat.id));
        let t = cfg.tweak(cat.id);
        let raw = shape::curve_points(part.curve, part.amp);
        if raw.len() < 2 {
            continue; // 「なし」
        }
        let v = shape::clamp01(part.v + t.dy);
        let w = (part.w * t.scale).clamp(0.0, 1.0);
        let on_bar = t.bar.unwrap_or(cat.on_bar);
        let mut points = shape::place(&profile, v, part.off, w, &raw);
        if t.dx != 0.0 {
            shape::shift_x(&profile, &mut points, t.dx);
        }
        if !part.mirror {
            out.push(DetailDraft {
                name: cat.id.to_string(),
                on_bar,
                points,
            });
            continue;
        }
        let flipped = points.iter().map(|p| [1.0 - p[0], p[1]]).collect();
        out.push(DetailDraft {
            name: format!("{}-r", cat.id),
            on_bar,
            points,
        });
        out.push(DetailDraft {
            name: format!("{}-l", cat.id),
            on_bar,
            points: flipped,
        });
    }
    out
}

// ---------------------------------------------------------------------------
// ランダム生成
// ---------------------------------------------------------------------------

/// 種から決まる乱数（SplitMix64）。**外部 crate を足さないため**の最小実装で、
/// 暗号用途ではない。種を渡す形にしてあるのは、`ccsessions-core` を時計や
/// 環境から切り離しておく（テストが決定的になる）ため。
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn pick<'a, T>(&mut self, xs: &'a [T]) -> &'a T {
        &xs[(self.next() % xs.len() as u64) as usize]
    }
}

/// ランダムなキャラクターを 1 体。**検証を通るものだけを返す。**
///
/// 「気軽に押せるボタン」が壊れた顔を出すと、押すたびに赤いエラーを読む羽目に
/// なって気軽でなくなる。組み合わせは構造的にほぼ全部通るので、実際には
/// 1 回目で決まることがほとんど（この再試行は保険）。
///
/// `keep` に既存の設定を渡すと、id・名前・作者・プレビュー設定は引き継ぐ。
pub fn random(seed: u64, keep: &CharacterConfig) -> CharacterConfig {
    let mut rng = Rng(seed);
    let mut last = keep.clone();
    for _ in 0..64 {
        let mut cfg = CharacterConfig {
            parts: BTreeMap::new(),
            tweaks: BTreeMap::new(),
            // **色は振り直さない**。名前や id と同じで、選んだ人の意思がある値なので
            // 「形だけ振り直す」ボタンで消さない。
            ..keep.clone()
        };
        cfg.parts
            .insert("face".into(), rng.pick(parts::FACES).id.to_string());
        cfg.parts
            .insert("eyes".into(), rng.pick(parts::EYES).id.to_string());
        for cat in parts::LINES {
            cfg.parts
                .insert(cat.id.to_string(), rng.pick(cat.variants).id.to_string());
        }
        if compose(&cfg).problems.is_empty() {
            return cfg;
        }
        last = cfg;
    }
    last
}

// ---------------------------------------------------------------------------
// 保存済みの顔から設定を読み戻す
// ---------------------------------------------------------------------------

/// 生成済みの顔 TOML に埋め込まれたビルダー設定を取り出す。
///
/// 手書きの顔（ビルダーを通していない TOML）には埋まっていないので `None`。
/// **手書きの顔をパーツに逆変換することはできない**（30 種の素形のどれとも
/// 一致しないから）ので、そこは正直に「編集できない」と言う。
pub fn config_from_toml(text: &str) -> Option<CharacterConfig> {
    let json = emit::extract_config(text)?;
    CharacterConfig::from_json(json).ok()
}

/// ファイル名に使える id か（保存前の門番）。パス区切りを含む id を弾く。
pub fn is_saveable_id(id: &str) -> bool {
    is_valid_id(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::face::Size;
    use crate::session::SessionState;

    fn with(pairs: &[(&str, &str)]) -> CharacterConfig {
        let mut cfg = CharacterConfig::default();
        for (k, v) in pairs {
            cfg.parts.insert(k.to_string(), v.to_string());
        }
        cfg
    }

    /// **どのパーツも、既定の顔に載せれば検証を通る。**
    ///
    /// これがビルダーの土台。5 カテゴリ × 30 を 1 つずつ差し替えて全部確かめる。
    #[test]
    fn every_part_composes_into_a_valid_face() {
        let mut cases: Vec<(String, CharacterConfig)> = Vec::new();
        for p in parts::FACES {
            cases.push((format!("face={}", p.id), with(&[("face", p.id)])));
        }
        for p in parts::EYES {
            cases.push((format!("eyes={}", p.id), with(&[("eyes", p.id)])));
        }
        for cat in parts::LINES {
            for p in cat.variants {
                cases.push((format!("{}={}", cat.id, p.id), with(&[(cat.id, p.id)])));
            }
        }
        for (what, cfg) in cases {
            let c = compose(&cfg);
            assert!(
                c.problems.is_empty(),
                "{what} が検証に落ちた: {:#?}\n--- TOML ---\n{}",
                c.problems,
                c.toml
            );
        }
    }

    /// **輪郭 30 種 × 目 30 種の全交差**が通る。組み合わせで壊れやすいのは
    /// この 2 つ（`eyes-outside-body` / `eyes-too-wide`）なので、ここだけは総当たりで見る。
    #[test]
    fn every_outline_and_eye_pairing_is_valid() {
        for f in parts::FACES {
            for e in parts::EYES {
                let cfg = with(&[("face", f.id), ("eyes", e.id)]);
                let c = compose(&cfg);
                assert!(
                    c.problems.is_empty(),
                    "顔 {} × 目 {} が落ちた: {:#?}",
                    f.id,
                    e.id,
                    c.problems
                );
            }
        }
    }

    /// 線パーツは**どの輪郭に載せても**顔の中に収まる。
    #[test]
    fn every_line_part_fits_on_every_outline() {
        for f in parts::FACES {
            for cat in parts::LINES {
                for p in cat.variants {
                    let cfg = with(&[("face", f.id), (cat.id, p.id)]);
                    let c = compose(&cfg);
                    assert!(
                        c.problems.is_empty(),
                        "顔 {} に {}={} を載せたら落ちた: {:#?}",
                        f.id,
                        cat.id,
                        p.id,
                        c.problems
                    );
                }
            }
        }
    }

    /// **人間の顔でないものが、この表の組み合わせで作れる。**
    ///
    /// ビルダーは「顔を足す正規の入口」なので、弧・波・まぶたで書ける範囲しか
    /// 作れないなら表に穴がある。数値の一致までは求めない（汎用の素形×数値と
    /// いう方針を崩さないため）が、**その系統を成り立たせている要素が全部
    /// そろうこと**は見る:
    ///
    /// - けもの … もみあげの輪郭 / 角が 1 本だけの丸目 / 前髪 / 左右対の耳
    /// - 機械   … 兜の輪郭 / 角ばったくさび目 / 眉 / 縦の鼻筋 / 板の口 / 左右対の頬
    ///
    /// このうち「左右対」「角ばった目」「縦の線」「板」は、どれも人間の顔を
    /// 前提にした素形（弧・波・まぶた）からは作れない。
    #[test]
    fn the_builder_can_express_non_human_faces() {
        let beast = with(&[
            ("face", "sideburn"),
            ("eyes", "horn"),
            ("hair", "straight"),
            ("nose", "none"),
            ("mouth", "none"),
            ("side", "ear-panel"),
        ]);
        let machine = with(&[
            ("face", "helmetish"),
            ("eyes", "wedge"),
            ("hair", "brow"),
            ("nose", "stroke"),
            ("mouth", "plate"),
            ("side", "cheek-seam"),
        ]);

        for (what, cfg) in [("けもの系", beast), ("機械系", machine)] {
            let c = compose(&cfg);
            assert!(c.problems.is_empty(), "{what} が落ちた: {:#?}", c.problems);

            // 対の側面パネルが左右 2 本出ている。
            let names: Vec<&str> = c.spec.details.iter().map(|d| d.name.as_str()).collect();
            assert!(
                names.contains(&"side-r") && names.contains(&"side-l"),
                "{what} に左右の側面パネルが無い: {names:?}"
            );

            // 左右が実際に鏡像になっている（片側だけずれていない）。
            let get = |n: &str| {
                c.spec
                    .details
                    .iter()
                    .find(|d| d.name == n)
                    .unwrap_or_else(|| panic!("{n} が無い"))
                    .points
                    .clone()
            };
            let (r, l) = (get("side-r"), get("side-l"));
            assert_eq!(r.len(), l.len(), "{what} の左右で点数が違う");
            for (a, b) in r.iter().zip(&l) {
                assert!(
                    (a[0] - (1.0 - b[0])).abs() < 1e-9 && (a[1] - b[1]).abs() < 1e-9,
                    "{what} の側面パネルが鏡像でない: {a:?} / {b:?}"
                );
            }
        }

        // くさび目は角ばった四角形（弧で近似した紡錘形ではない）。
        let poly = compose(&with(&[("eyes", "wedge")]))
            .spec
            .eyes
            .polygon
            .clone()
            .expect("くさび目が多角形になっていない");
        assert_eq!(poly.len(), 4, "くさびが 4 点でない: {poly:?}");
    }

    /// 生成した TOML は**そのまま `faces/` に置ける**（パースし直せる）。
    #[test]
    fn the_generated_toml_parses_back_into_the_same_face() {
        let c = compose(&CharacterConfig::default());
        let again = crate::face::parse::parse(&c.toml, Source::Builtin).expect("読み直せない");
        assert_eq!(again, c.spec, "TOML と描いている顔が食い違う");
    }

    /// 色も同じ往復を通る。**書き出し（16 進の丸め）で色がずれると
    /// 「画面で見た顔と保存される顔が別物」になる**ので、ここで固定する。
    #[test]
    fn the_generated_toml_round_trips_the_colours_too() {
        let mut colors = BTreeMap::new();
        colors.insert(
            "working".to_string(),
            StateColorCfg {
                accent: "#7f3ac2".into(),
                fill: "#241038".into(),
                eye: "#00ff88".into(),
            },
        );
        // 1 色だけ書いた状態も混ぜる（部分指定が往復すること）。
        colors.insert(
            "error".to_string(),
            StateColorCfg {
                accent: "#ff0044".into(),
                ..StateColorCfg::default()
            },
        );
        let c = compose(&CharacterConfig {
            colors,
            ..CharacterConfig::default()
        });
        let again = crate::face::parse::parse(&c.toml, Source::Builtin).expect("読み直せない");
        assert_eq!(again, c.spec, "色が往復で変わっている\n{}", c.toml);
    }

    /// 設定は TOML に埋め込まれ、読み戻せる（保存 → 再編集の経路）。
    #[test]
    fn the_config_round_trips_through_the_saved_toml() {
        let cfg = with(&[
            ("face", "helmetish"),
            ("eyes", "slit-up"),
            ("mouth", "grin"),
        ]);
        let c = compose(&cfg);
        let back = config_from_toml(&c.toml).expect("設定が埋まっていない");
        assert_eq!(back, cfg);
        // 手書きの顔からは取れない。
        let egg = include_str!("../../../../faces/egg.toml");
        assert!(config_from_toml(egg).is_none());
    }

    /// JSON でも往復する（ダウンロード → 読み込み）。
    #[test]
    fn the_config_round_trips_through_json() {
        let cfg = with(&[("hair", "spike4"), ("nose", "hook-l")]);
        let json = cfg.to_json_pretty();
        assert_eq!(CharacterConfig::from_json(&json).unwrap(), cfg);
    }

    /// 目が大きすぎる組み合わせは自動で縮む（検証器そのものを使うフィット）。
    #[test]
    fn oversized_eyes_are_shrunk_until_they_fit() {
        let mut cfg = with(&[("face", "slim"), ("eyes", "bead-big")]);
        cfg.tweaks.insert(
            "eyes".into(),
            Tweak {
                scale: 2.0,
                ..Tweak::default()
            },
        );
        let c = compose(&cfg);
        assert!(c.eye_fit < 1.0, "縮んでいない");
        assert!(
            c.problems.is_empty(),
            "縮めても収まっていない: {:#?}",
            c.problems
        );
    }

    /// ランダム生成は必ず検証を通る（気軽に押せるボタンであるための条件）。
    #[test]
    fn random_characters_are_always_valid() {
        let base = CharacterConfig::default();
        for seed in 0..40u64 {
            let cfg = random(seed, &base);
            let c = compose(&cfg);
            assert!(
                c.problems.is_empty(),
                "seed {seed} が落ちた: {:#?}\n{:?}",
                c.problems,
                cfg.parts
            );
            // id・名前は引き継ぐ（押すたびに名前が消えると使い物にならない）。
            assert_eq!(cfg.id, base.id);
            assert_eq!(cfg.name, base.name);
        }
        // 種が違えば違う顔が出る。
        assert_ne!(random(1, &base).parts, random(2, &base).parts);
        // 同じ種なら同じ顔（再現できる）。
        assert_eq!(random(7, &base).parts, random(7, &base).parts);
    }

    /// **後方互換の番人**: 0.2.0 までの `eye_color`（4 値）で保存した顔を開き直しても、
    /// 目の色は消えず、当時と同じ 3 状態にだけ効く。
    #[test]
    fn a_face_saved_with_the_old_eye_colour_keeps_it() {
        let cfg = CharacterConfig {
            eye_color: "white".into(),
            ..CharacterConfig::default()
        };
        let c = compose(&cfg);
        assert!(c.problems.is_empty(), "{:#?}", c.problems);
        assert!(
            c.spec.eye(SessionState::Working, Size::Bar).blink,
            "瞬きが消えた"
        );
        assert_eq!(
            c.spec.eye(SessionState::WaitAgent, Size::Bar).dx,
            1.5,
            "横目が消えた"
        );
        assert_eq!(
            c.spec.eye(SessionState::Done, Size::Bar).color,
            (1.0, 1.0, 1.0),
            "色が反映されていない"
        );
        // 見開き・アイドル・エラーは塗り替えない（状態が読めなくなるため）。
        assert_eq!(
            c.spec.eye(SessionState::Error, Size::Bar).color,
            crate::face::palette::EYE_ERROR
        );
    }

    /// 状態ごとに選んだ色が、その状態の顔に出る。
    #[test]
    fn the_colours_chosen_per_state_reach_the_face() {
        let mut colors = BTreeMap::new();
        colors.insert(
            "working".to_string(),
            StateColorCfg {
                accent: "#7f3ac2".into(),
                fill: "#241038".into(),
                eye: "#00ff88".into(),
            },
        );
        let cfg = CharacterConfig {
            colors,
            ..CharacterConfig::default()
        };
        let c = compose(&cfg);
        assert!(c.problems.is_empty(), "{:#?}", c.problems);
        // 16 進の丸めに縛られないよう、比較は `palette` の変換を往復させる。
        let hex = |c| crate::face::palette::to_hex(c);
        assert_eq!(hex(c.spec.accent(SessionState::Working)), "#7f3ac2");
        assert_eq!(hex(c.spec.fill(SessionState::Working)), "#241038");
        assert_eq!(
            hex(c.spec.eye(SessionState::Working, Size::Bar).color),
            "#00ff88"
        );
        // 形は触っていない（`[colors.*]` は `[eyes.states.*]` と別の口）。
        assert!(
            c.spec.eye(SessionState::Working, Size::Bar).blink,
            "色を選んだら瞬きが消えた"
        );
        // 選んでいない状態は既定パレットのまま。
        assert_eq!(
            c.spec.accent(SessionState::Error),
            crate::face::palette::accent(SessionState::Error)
        );
    }

    /// **読めない色は「指定なし」に落ちる**。UI から何が来ても顔は組み上がる。
    #[test]
    fn an_unreadable_colour_falls_back_to_the_default_palette() {
        let mut colors = BTreeMap::new();
        colors.insert(
            "done".to_string(),
            StateColorCfg {
                accent: "reddish".into(),
                fill: String::new(),
                eye: "#".into(),
            },
        );
        let cfg = CharacterConfig {
            colors,
            ..CharacterConfig::default()
        };
        let c = compose(&cfg);
        assert!(c.problems.is_empty(), "{:#?}", c.problems);
        assert_eq!(
            c.spec.accent(SessionState::Done),
            crate::face::palette::accent(SessionState::Done)
        );
    }

    /// 色を 1 つも選ばなければ `[colors.*]` は 1 行も出ない
    /// （＝ 0.2.0 の顔ファイルと同じテキストになる）。
    #[test]
    fn a_face_with_no_colours_writes_no_colour_table() {
        let c = compose(&CharacterConfig::default());
        assert!(!c.toml.contains("[colors."), "{}", c.toml);
    }

    /// 不正な id でもプレビューは生き残り、問題として報告される。
    #[test]
    fn a_bad_id_is_reported_without_killing_the_preview() {
        let cfg = CharacterConfig {
            id: "Bad Id".into(),
            ..CharacterConfig::default()
        };
        let c = compose(&cfg);
        assert!(
            c.problems.iter().any(|p| p.code == ProblemCode::Id),
            "{:#?}",
            c.problems
        );
        // 描けている（体の寸法が取れる）。
        assert!(c.spec.body_size(Size::Dock).0 > 0.0);
        assert!(!is_saveable_id(&cfg.id));
        assert!(is_saveable_id("my-face"));
    }

    /// 微調整は範囲外の値を渡しても安全側に丸められる。
    #[test]
    fn tweaks_are_clamped_into_a_sane_range() {
        let mut cfg = CharacterConfig::default();
        cfg.tweaks.insert(
            "mouth".into(),
            Tweak {
                dx: 99.0,
                dy: -99.0,
                scale: 1e9,
                gap: f64::NAN,
                bar: Some(true),
            },
        );
        let t = cfg.tweak("mouth");
        assert_eq!(t.dx, 0.25);
        assert_eq!(t.dy, -0.25);
        assert_eq!(t.scale, 2.0);
        assert_eq!(t.gap, 1.0);
        let c = compose(&cfg);
        assert!(c.problems.is_empty(), "{:#?}", c.problems);
    }

    /// bar のパネル線は必ず dock の部分集合（`bar-details-not-thinned` に落ちない根拠）。
    #[test]
    fn bar_lines_are_always_a_subset_of_dock_lines() {
        let base = CharacterConfig::default();
        for seed in 0..20u64 {
            let mut cfg = random(seed, &base);
            // 全カテゴリを bar にも出す指示をしても、dock には必ず出る。
            for cat in parts::LINES {
                cfg.tweaks.insert(
                    cat.id.to_string(),
                    Tweak {
                        bar: Some(true),
                        ..Tweak::default()
                    },
                );
            }
            let c = compose(&cfg);
            let bar = c.spec.face_details(1.0, 1.0, Size::Bar).len();
            let dock = c.spec.face_details(1.0, 1.0, Size::Dock).len();
            assert!(bar <= dock, "seed {seed}: bar {bar} > dock {dock}");
            assert!(c.problems.is_empty(), "seed {seed}: {:#?}", c.problems);
        }
    }

    /// 全パーツ・全状態・両サイズで SVG が壊れない（UI が描くもの）。
    #[test]
    fn every_part_renders_to_sane_svg() {
        for p in parts::FACES {
            let c = compose(&with(&[("face", p.id)]));
            for state in SessionState::ORDER {
                for size in [Size::Bar, Size::Dock] {
                    let svg = crate::face::svg::render(&c.spec, state, size);
                    assert!(svg.starts_with("<svg "), "{}", p.id);
                    assert!(!svg.contains("NaN"), "{} に NaN", p.id);
                }
            }
        }
    }
}
