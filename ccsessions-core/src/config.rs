//! ccsessions の `config.toml` 設定。
//!
//! 方針: ファイルが無ければ組込みデフォルトを黙って
//! 返し（初回起動でエラーにしない）、パースエラーや未知の enum 値は `Err`
//! で人間が読めるメッセージにする（呼び出し側 = daemon は last-good を保持
//! できるようにするため、ここではプロセスを落とさない）。

use std::io;
use std::path::Path;

use serde::Deserialize;

use crate::write_atomic;

// ---------------------------------------------------------------------------
// Enums
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placement {
    Bar,
    Dock,
}

impl Placement {
    pub fn as_str(&self) -> &'static str {
        match self {
            Placement::Bar => "bar",
            Placement::Dock => "dock",
        }
    }

    /// `std::str::FromStr` は実装しない（失敗理由を持たない `Option` で十分
    /// なため。session.rs の `SessionState::from_str` と同じ判断）。
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "bar" => Some(Placement::Bar),
            "dock" => Some(Placement::Dock),
            _ => None,
        }
    }
}

/// `design` に指定できる**組込みの**値。`config.toml` のコメントに埋める。
///
/// ユーザ顔（`~/.config/ccsessions/faces/*.toml`）はここに現れない — 設定を書く
/// 時点でどんな顔が置かれているかは分からないため。実際に使える顔の一覧は
/// `ccsessions face list` が見せる。
pub fn design_choices() -> String {
    crate::face::builtin_ids()
        .iter()
        .map(|s| format!("\"{s}\""))
        .collect::<Vec<_>>()
        .join(" | ")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BarAlign {
    Auto,
    Center,
    LeftOfNotch,
    RightOfNotch,
}

impl BarAlign {
    pub fn as_str(&self) -> &'static str {
        match self {
            BarAlign::Auto => "auto",
            BarAlign::Center => "center",
            BarAlign::LeftOfNotch => "left-of-notch",
            BarAlign::RightOfNotch => "right-of-notch",
        }
    }

    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "auto" => Some(BarAlign::Auto),
            "center" => Some(BarAlign::Center),
            "left-of-notch" => Some(BarAlign::LeftOfNotch),
            "right-of-notch" => Some(BarAlign::RightOfNotch),
            _ => None,
        }
    }
}

/// 群れをコンパクト表示（一様な縮小）へ切り替える方針。`config.toml` の `compact_flock`。
///
/// **なぜ要るか**: 生き物は横一列に並ぶので、セッションが増えるほど帯が広くなる。
/// ノッチ機で `bar_align = "auto"` のとき、ノッチ右の空き帯（実測 225pt ＝ 6 匹）に
/// 入り切らない群れは左へ逃げるが、左は前面アプリのメニュー（File・Edit…）の領域で
/// **どのアプリが前面かによって重なる**。縮めて右に収められるならその方が安全なので、
/// 「収まらなくなったら縮める」を既定にする。
///
/// `Auto` は**収まっているうちは何もしない**ので、6 匹以下で使っている限り
/// 見た目は 0.1.0 と完全に同じ。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompactMode {
    /// 使える幅に収まらなくなったら縮める（既定）。
    Auto,
    /// 匹数によらず常に縮める。
    Always,
    /// 縮めない。
    Never,
}

impl CompactMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            CompactMode::Auto => "auto",
            CompactMode::Always => "always",
            CompactMode::Never => "never",
        }
    }

    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "auto" => Some(CompactMode::Auto),
            "always" => Some(CompactMode::Always),
            "never" => Some(CompactMode::Never),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

/// ccsessions の全設定。TTL 系フィールドは `std::time::Duration` ではなく素の
/// `u64`（ミリ秒）で持つ — 消費側（`Session::display_state` / `store::sweep`）
/// はどちらも epoch ms の整数演算で扱うので、Duration の変換コストと unwrap
/// を挟まずそのまま引き算できるようにするため。TOML 上は秒単位
/// （`done_ttl_secs` 等）で書く方が人間に分かりやすいので、そこだけ ms/秒の
/// ギャップが生じる。このギャップを外部から見えなくするため ms フィールド自体
/// は非公開にし、`done_ttl_ms()`/`session_ttl_ms()`（読み取り）と
/// `set_done_ttl_secs()`/`set_session_ttl_secs()`（書き込み、秒単位）だけを
/// 公開する。
#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub placement: Placement,
    /// 使う顔の id。**ここでは実在を検証しない**。
    ///
    /// ユーザ顔ファイルの存在は設定のパース時点では分からないので、検証は
    /// 顔レジストリの解決時（`face::Registry::resolve`）へ移してある。未知の id は
    /// `Err` ではなく `egg` へのフォールバック + ログになる。
    /// パース時に見るのは**形（`[a-z0-9-]+`）だけ**。
    pub design: String,
    pub reduce_motion: bool,
    pub show_glyphs: bool,
    pub bar_align: BarAlign,
    /// セッションが増えて帯が使える幅に収まらなくなったとき、群れを縮めるか。
    pub compact_flock: CompactMode,
    /// dock をドラッグして決めた位置（pt、AppKit のグローバル座標＝**画面左下が原点**）。
    ///
    /// `dock_x` は**パネルの中心**、`dock_y` は**パネルの下端**。中心で持つのは、
    /// パネルの幅がセッション数で変わるため — 左端で持つと匹数が増えるたびに右へ
    /// 伸びて、置いた位置の印象がずれる。
    ///
    /// **軸ごとに独立した `Option`** にしてある。`dock_x` だけ書かれた設定ファイルを
    /// エラーにも「両方無視」にもせず、書かれた軸だけ効かせて他方は既定（画面下部中央）
    /// へ落とせる。ドラッグは常に両方を書くので、この非対称が生じるのは人が手で
    /// 設定ファイルを編集したときだけ。
    ///
    /// `None` は既定配置。画面内へ収める補正は表示のたびに `geometry::dock_rect_at` が
    /// 行い、**ここへは書き戻さない** — 書き戻すと、外部モニタを外して画面が狭くなった
    /// ときに押し込まれた位置が確定してしまい、繋ぎ直しても元へ戻れなくなる。
    pub dock_x: Option<f64>,
    pub dock_y: Option<f64>,
    done_ttl_ms: u64,
    session_ttl_ms: u64,
    pub max_sessions: usize,
    pub detect_errors: bool,
}

impl Config {
    pub fn done_ttl_ms(&self) -> u64 {
        self.done_ttl_ms
    }

    pub fn session_ttl_ms(&self) -> u64 {
        self.session_ttl_ms
    }

    /// `ccsessions config set done_ttl_secs <n>` から呼ばれる想定。TOML の単位
    /// （秒）を受けて内部表現（ms）に変換する。
    pub fn set_done_ttl_secs(&mut self, secs: u64) {
        self.done_ttl_ms = secs.saturating_mul(1000);
    }

    pub fn set_session_ttl_secs(&mut self, secs: u64) {
        self.session_ttl_ms = secs.saturating_mul(1000);
    }

    fn done_ttl_secs(&self) -> u64 {
        self.done_ttl_ms / 1000
    }

    fn session_ttl_secs(&self) -> u64 {
        self.session_ttl_ms / 1000
    }
}

// ---------------------------------------------------------------------------
// Built-in default
// ---------------------------------------------------------------------------

const DEFAULT_PLACEMENT: &str = "bar";
const DEFAULT_DESIGN: &str = "egg";
const DEFAULT_REDUCE_MOTION: bool = false;
const DEFAULT_SHOW_GLYPHS: bool = true;
const DEFAULT_BAR_ALIGN: &str = "auto";
/// 既定は `auto`。**収まっているうちは縮めない**ので、既存ユーザの見た目は変わらない
/// （変わるのは「今までノッチ左へ逃げていた 7 匹以上」のときだけ）。
const DEFAULT_COMPACT_FLOCK: &str = "auto";
const DEFAULT_DONE_TTL_SECS: u64 = 180;
const DEFAULT_SESSION_TTL_SECS: u64 = 28_800;
const DEFAULT_MAX_SESSIONS: usize = 12;
/// 既定 `false`。エラー検出は `StopFailure` hook に一本化した。
///
/// API エラーで終わったターンは `Stop` ではなく `StopFailure` を出すので、
/// `Stop` のときだけ transcript を読むこの経路は**本来狙っていた場面では
/// 発火していなかった**。`StopFailure` が種別付きで直接エラーを運ぶようになった今、
/// 残っている用途は「`Stop` は来たが直近の assistant 行が
/// `isApiErrorMessage`」という未実証のケースだけなので、既定では毎回 64KB を
/// 読むコストを払わない。必要なら `detect_errors = true` で戻せる。
const DEFAULT_DETECT_ERRORS: bool = false;

/// `config.toml` が無いときに使う組込みデフォルト。
pub fn builtin_default() -> Config {
    Config {
        placement: Placement::Bar,
        design: DEFAULT_DESIGN.to_string(),
        reduce_motion: DEFAULT_REDUCE_MOTION,
        show_glyphs: DEFAULT_SHOW_GLYPHS,
        bar_align: BarAlign::Auto,
        compact_flock: CompactMode::Auto,
        dock_x: None,
        dock_y: None,
        done_ttl_ms: DEFAULT_DONE_TTL_SECS * 1000,
        session_ttl_ms: DEFAULT_SESSION_TTL_SECS * 1000,
        max_sessions: DEFAULT_MAX_SESSIONS,
        detect_errors: DEFAULT_DETECT_ERRORS,
    }
}

// ---------------------------------------------------------------------------
// load / save
// ---------------------------------------------------------------------------

/// `path` から設定を読み込む。
///
/// - ファイルが無ければ `Ok(builtin_default())`。
/// - パース済みかつ妥当なら `Ok(config)`。
/// - I/O（NotFound 以外）・TOML 構文エラー・未知の enum 値は `Err(msg)`。
///   呼び出し側（daemon）は last-good を保持してこのメッセージをログするだけ
///   にすべきで、ここでは決してパニックしない。
pub fn load(path: &Path) -> Result<Config, String> {
    let content = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(builtin_default()),
        Err(e) => return Err(format!("cannot read {}: {}", path.display(), e)),
    };
    load_from_str(&content)
}

fn load_from_str(s: &str) -> Result<Config, String> {
    let raw: RawConfig = toml::from_str(s).map_err(|e| format!("config parse error: {}", e))?;
    validate_and_build(&raw)
}

/// `path` へ設定を書き出す。
///
/// `toml::to_string` ではなくテンプレート文字列に値を埋める形にしている。
/// 前者はコメントを保持できず、`# "bar" | "dock"` のような選択肢の案内が
/// 消えてしまい、設定ファイルとして不親切になるため。
pub fn save(path: &Path, c: &Config) -> io::Result<()> {
    let content = render_toml(c);
    write_atomic(path, &content)
}

/// 設定を TOML テキストとして描画する。`save` の内部実装だが、
/// `ccsessions config`（get サブコマンド）が「ファイルに書かず現在の設定を
/// 表示するだけ」を実装するのにもそのまま使えるので公開している。
pub fn render_toml(c: &Config) -> String {
    let mut out = format!(
        r#"placement = "{placement}"          # "bar" | "dock"
design = "{design}"                # {choices}
reduce_motion = {reduce_motion}
show_glyphs = {show_glyphs}
bar_align = "{bar_align}"          # "auto" | "center" | "left-of-notch" | "right-of-notch"
compact_flock = "{compact_flock}"  # "auto"（入り切らなければ縮める）| "always" | "never"
done_ttl_secs = {done_ttl_secs}
session_ttl_secs = {session_ttl_secs}
max_sessions = {max_sessions}
detect_errors = {detect_errors}
"#,
        placement = c.placement.as_str(),
        design = c.design,
        choices = design_choices(),
        reduce_motion = c.reduce_motion,
        show_glyphs = c.show_glyphs,
        bar_align = c.bar_align.as_str(),
        compact_flock = c.compact_flock.as_str(),
        done_ttl_secs = c.done_ttl_secs(),
        session_ttl_secs = c.session_ttl_secs(),
        max_sessions = c.max_sessions,
        detect_errors = c.detect_errors,
    );
    // dock の位置は**ドラッグして動かしたときだけ**現れる行にする。既定のままなら
    // 行ごと出さないので、設定ファイルを開けば「まだ動かしていない」ことがそのまま
    // 読み取れる。
    //
    // 値を `{:?}` で書くのは、f64 を往復で完全に復元できる最短表記になるため。
    // `{}`（Display）は 687.0 を `687` と書いてしまい、TOML 上は整数になる。
    if c.dock_x.is_some() || c.dock_y.is_some() {
        out.push_str(
            "\n# dock をドラッグして決めた位置（パネル中心の x / 下端の y、画面左下が原点）。\n\
             # 既定の「画面下部中央」へ戻すには、この行を消すか `ccsessions config set dock_x auto`。\n",
        );
        if let Some(x) = c.dock_x {
            out.push_str(&format!("dock_x = {x:?}\n"));
        }
        if let Some(y) = c.dock_y {
            out.push_str(&format!("dock_y = {y:?}\n"));
        }
    }
    out
}

// ---------------------------------------------------------------------------
// 設定のスキーマ（キー・種類・選択肢）
// ---------------------------------------------------------------------------
//
// **設定の入口は Web UI（`ccsessions ui`）に一本化してある。** 画面がキーを
// ベタ書きしていると、設定を 1 つ足すたびに「core・CLI・HTML・JS」の 4 か所を
// 直すことになり、必ずどこかが落ちる（実際 `show_glyphs` は config・`config set`・
// status item・README に存在しながら描画側が読んでおらず、設定が嘘をついていた
// ことがある）。
//
// そこで**キーの一覧・型・選択肢・説明をここ 1 か所に置く**。CLI の
// `config set` も Web UI のフォームもこの表を読むので、設定を足すときに触るのは
// `Config` の定義・`RawConfig`・`render_toml`・この表だけになる。

/// 設定 1 項目の型。UI はこれを見て入力欄の形を決める。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldKind {
    /// 固定の選択肢（値, 表示ラベル）。
    Choice(&'static [(&'static str, &'static str)]),
    Bool,
    /// 非負整数。範囲は**書くときだけ**見る（読み込みは既存の値を尊重する）。
    Int {
        min: u64,
        max: u64,
        unit: &'static str,
    },
    /// 顔の id。選択肢はレジストリ（組込み + ユーザ顔）なので、ここには持たない。
    Face,
    /// dock の座標（pt）。`"auto"` で既定配置へ戻す。
    Coord,
}

/// 設定 1 項目の説明。
#[derive(Debug, Clone, Copy)]
pub struct FieldSpec {
    pub key: &'static str,
    pub label: &'static str,
    pub kind: FieldKind,
    pub help: &'static str,
}

const PLACEMENT_CHOICES: &[(&str, &str)] = &[("bar", "メニューバー"), ("dock", "ドック（画面下）")];

const BAR_ALIGN_CHOICES: &[(&str, &str)] = &[
    ("auto", "自動（ノッチを避ける）"),
    ("center", "中央"),
    ("left-of-notch", "ノッチの左"),
    ("right-of-notch", "ノッチの右"),
];

const COMPACT_CHOICES: &[(&str, &str)] = &[
    ("auto", "入り切らなければ縮める"),
    ("always", "常に縮める"),
    ("never", "縮めない"),
];

/// 設定の全項目。**UI が並べる順**でもある。
pub fn fields() -> &'static [FieldSpec] {
    &[
        FieldSpec {
            key: "placement",
            label: "配置",
            kind: FieldKind::Choice(PLACEMENT_CHOICES),
            help: "生き物をメニューバーに出すか、画面下のパネルに出すか。",
        },
        FieldSpec {
            key: "design",
            label: "生き物",
            kind: FieldKind::Face,
            help: "顔は faces/*.toml から来る。自分で作った顔もここに出る。",
        },
        FieldSpec {
            key: "reduce_motion",
            label: "動きを減らす",
            kind: FieldKind::Bool,
            help: "アニメーションを止める。",
        },
        FieldSpec {
            key: "show_glyphs",
            label: "記号を表示",
            kind: FieldKind::Bool,
            help: "状態記号（› ! ⋯ z ✓ ×）を出す。",
        },
        FieldSpec {
            key: "bar_align",
            label: "帯の寄せ",
            kind: FieldKind::Choice(BAR_ALIGN_CHOICES),
            help: "メニューバー配置のときの横位置。center はノッチ機だと隠れる。",
        },
        FieldSpec {
            key: "compact_flock",
            label: "群れを縮める",
            kind: FieldKind::Choice(COMPACT_CHOICES),
            help: "セッションが増えて帯に入り切らなくなったときの振る舞い。",
        },
        FieldSpec {
            key: "done_ttl_secs",
            label: "完了 → アイドル",
            kind: FieldKind::Int {
                min: 0,
                max: 86_400,
                unit: "秒",
            },
            help: "完了状態がアイドルへ変わるまでの時間。",
        },
        FieldSpec {
            key: "session_ttl_secs",
            label: "セッションの保険 TTL",
            kind: FieldKind::Int {
                min: 60,
                max: 604_800,
                unit: "秒",
            },
            help: "これだけ無更新なら消す。死んだセッションは pid で先に消えるので保険。",
        },
        FieldSpec {
            key: "max_sessions",
            label: "同時に出す最大数",
            kind: FieldKind::Int {
                min: 1,
                max: 64,
                unit: "匹",
            },
            help: "これを超えた分は直近に動いたセッションが優先される。",
        },
        FieldSpec {
            key: "detect_errors",
            label: "transcript でもエラー検出",
            kind: FieldKind::Bool,
            help: "Stop のたびに transcript の末尾を読む補助手段。既定は off。",
        },
        FieldSpec {
            key: "dock_x",
            label: "dock の x（中心）",
            kind: FieldKind::Coord,
            help: "ドラッグで決まる。auto で画面下部中央へ戻る。",
        },
        FieldSpec {
            key: "dock_y",
            label: "dock の y（下端）",
            kind: FieldKind::Coord,
            help: "ドラッグで決まる。auto で画面下部中央へ戻る。",
        },
    ]
}

pub fn field(key: &str) -> Option<&'static FieldSpec> {
    fields().iter().find(|f| f.key == key)
}

/// いまの値を**`set_field` がそのまま受け取れる文字列**にする。
///
/// 「読んだものを書き戻せば元に戻る」が保証されるので、UI はこの文字列を
/// 入力欄の初期値にすればよい（`the_current_value_of_every_field_round_trips`）。
pub fn field_value(cfg: &Config, key: &str) -> Option<String> {
    let v = match key {
        "placement" => cfg.placement.as_str().to_string(),
        "design" => cfg.design.clone(),
        "reduce_motion" => cfg.reduce_motion.to_string(),
        "show_glyphs" => cfg.show_glyphs.to_string(),
        "bar_align" => cfg.bar_align.as_str().to_string(),
        "compact_flock" => cfg.compact_flock.as_str().to_string(),
        "done_ttl_secs" => cfg.done_ttl_secs().to_string(),
        "session_ttl_secs" => cfg.session_ttl_secs().to_string(),
        "max_sessions" => cfg.max_sessions.to_string(),
        "detect_errors" => cfg.detect_errors.to_string(),
        "dock_x" => coord_to_string(cfg.dock_x),
        "dock_y" => coord_to_string(cfg.dock_y),
        _ => return None,
    };
    Some(v)
}

fn coord_to_string(v: Option<f64>) -> String {
    match v {
        Some(x) => format!("{x}"),
        None => "auto".to_string(),
    }
}

/// 設定を 1 項目だけ書き換える。**CLI (`ccsessions config set`) も Web UI も
/// ここを通る**ので、検証もエラーメッセージも 1 つで済む。
///
/// `faces` を引数で受けるのは、この関数をファイル I/O から切り離すため
/// （core の他の関数と同じ流儀。テストも組込みレジストリで書ける）。
pub fn set_field(
    cfg: &mut Config,
    key: &str,
    value: &str,
    faces: &crate::face::Registry,
) -> Result<(), String> {
    match key {
        "placement" => {
            cfg.placement = Placement::from_str(value)
                .ok_or_else(|| format!("invalid placement: {value:?} (want \"bar\" | \"dock\")"))?
        }
        // **明示的な操作なので、ここでは実在チェックをする**。
        // 設定ファイルの読み込み時に未知の顔を許すのは「ユーザ顔がまだ置かれて
        // いないかもしれない」からだが、ここは今まさに人が選んだ指示なので、
        // 打ち間違いを黙って受けるより即座に教える方が親切。
        "design" => {
            if faces.get(value).is_none() {
                return Err(format!(
                    "invalid design: {value:?} (使える顔: {})",
                    faces.ids().join(", ")
                ));
            }
            cfg.design = value.to_string();
        }
        "reduce_motion" => cfg.reduce_motion = parse_bool(value)?,
        "show_glyphs" => cfg.show_glyphs = parse_bool(value)?,
        "bar_align" => {
            cfg.bar_align = BarAlign::from_str(value).ok_or_else(|| {
                format!(
                    "invalid bar_align: {value:?} (want \"auto\" | \"center\" | \"left-of-notch\" | \"right-of-notch\")"
                )
            })?
        }
        "compact_flock" => {
            cfg.compact_flock = CompactMode::from_str(value).ok_or_else(|| {
                format!("invalid compact_flock: {value:?} (want \"auto\" | \"always\" | \"never\")")
            })?
        }
        "done_ttl_secs" => cfg.set_done_ttl_secs(parse_int(key, value)?),
        "session_ttl_secs" => cfg.set_session_ttl_secs(parse_int(key, value)?),
        "max_sessions" => cfg.max_sessions = parse_int(key, value)? as usize,
        "detect_errors" => cfg.detect_errors = parse_bool(value)?,
        // dock をドラッグして決めた位置。**`auto` で既定へ戻せる口を必ず残す** —
        // さもないと一度動かしたら、設定ファイルを手で編集する以外に画面下部中央へ
        // 戻す手段が無くなる。
        "dock_x" => cfg.dock_x = parse_coord(value)?,
        "dock_y" => cfg.dock_y = parse_coord(value)?,
        other => return Err(format!("unknown config key: {other:?}")),
    }
    Ok(())
}

fn parse_bool(v: &str) -> Result<bool, String> {
    match v {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(format!("expected \"true\" or \"false\", got {v:?}")),
    }
}

/// 整数の項目。範囲は `fields()` の宣言から取るので、上限・下限を 2 か所に
/// 書かなくて済む（UI の `min`/`max` 属性も同じ宣言から出る）。
fn parse_int(key: &str, v: &str) -> Result<u64, String> {
    let n: u64 = v
        .trim()
        .parse()
        .map_err(|_| format!("expected an integer, got {v:?}"))?;
    if let Some(FieldSpec {
        kind: FieldKind::Int { min, max, unit },
        ..
    }) = field(key)
    {
        if n < *min || n > *max {
            return Err(format!(
                "{key} は {min}〜{max}{unit} の範囲にしてください（got {n}）"
            ));
        }
    }
    Ok(n)
}

/// dock の座標（pt）。`"auto"` は「保存された位置を捨てて既定へ戻す」。
///
/// 有限でない値（`nan` / `inf`）は弾く — 通すと窓の矩形が壊れる。設定の読み込み側
/// （`validate_dock_coord`）でも弾いているが、打ち間違いは書く時点で教える方が親切。
fn parse_coord(v: &str) -> Result<Option<f64>, String> {
    let v = v.trim();
    if v == "auto" || v.is_empty() {
        return Ok(None);
    }
    match v.parse::<f64>() {
        Ok(x) if x.is_finite() => Ok(Some(x)),
        Ok(_) => Err(format!(
            "expected a finite number or \"auto\", got {v:?}（nan / inf は使えない）"
        )),
        Err(_) => Err(format!("expected a number or \"auto\", got {v:?}")),
    }
}

// ---------------------------------------------------------------------------
// Raw TOML deserialization (internal)
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct RawConfig {
    #[serde(default = "default_placement")]
    placement: String,
    #[serde(default = "default_design")]
    design: String,
    #[serde(default)]
    reduce_motion: bool,
    #[serde(default = "default_show_glyphs")]
    show_glyphs: bool,
    #[serde(default = "default_bar_align")]
    bar_align: String,
    #[serde(default = "default_compact_flock")]
    compact_flock: String,
    /// 省略可能（既定配置なら**行ごと存在しない**）。`Option` + `#[serde(default)]` は
    /// フィールド単位で完結するので、他の 10 個が使っている `default_*()` 関数群の
    /// 流儀を崩さない。
    #[serde(default)]
    dock_x: Option<f64>,
    #[serde(default)]
    dock_y: Option<f64>,
    #[serde(default = "default_done_ttl_secs")]
    done_ttl_secs: u64,
    #[serde(default = "default_session_ttl_secs")]
    session_ttl_secs: u64,
    #[serde(default = "default_max_sessions")]
    max_sessions: usize,
    #[serde(default = "default_detect_errors")]
    detect_errors: bool,
}

fn default_placement() -> String {
    DEFAULT_PLACEMENT.into()
}
fn default_design() -> String {
    DEFAULT_DESIGN.into()
}
fn default_show_glyphs() -> bool {
    DEFAULT_SHOW_GLYPHS
}
fn default_bar_align() -> String {
    DEFAULT_BAR_ALIGN.into()
}
fn default_compact_flock() -> String {
    DEFAULT_COMPACT_FLOCK.into()
}
fn default_done_ttl_secs() -> u64 {
    DEFAULT_DONE_TTL_SECS
}
fn default_session_ttl_secs() -> u64 {
    DEFAULT_SESSION_TTL_SECS
}
fn default_max_sessions() -> usize {
    DEFAULT_MAX_SESSIONS
}
fn default_detect_errors() -> bool {
    DEFAULT_DETECT_ERRORS
}

/// dock の座標として使える値か。
///
/// **有限でない値は `Err` にする**。`NaN` / `inf` を通すと窓の矩形が壊れ、AppKit 側で
/// 何が起きるか読めない（顔の寸法を `usable_dim` で弾いているのと同じ判断）。ここで
/// 弾けば daemon は last-good の設定を保ったまま、理由をログに出せる。
fn validate_dock_coord(key: &str, v: Option<f64>) -> Result<Option<f64>, String> {
    match v {
        Some(x) if !x.is_finite() => Err(format!("invalid {key}: {x}（有限の数値であること）")),
        other => Ok(other),
    }
}

fn validate_and_build(raw: &RawConfig) -> Result<Config, String> {
    let placement = Placement::from_str(&raw.placement).ok_or_else(|| {
        format!(
            "invalid placement: {:?} (want \"bar\" | \"dock\")",
            raw.placement
        )
    })?;
    // **実在チェックはしない**。ここで見るのは形だけで、
    // 「そんな顔は無い」の判定はレジストリ解決時に行い、egg へフォールバックする。
    if !crate::face::spec::is_valid_id(&raw.design) {
        return Err(format!(
            "invalid design: {:?} (顔の id は英小文字・数字・ハイフンだけで、\
             先頭は英数字、32 文字以内。組込みは {})",
            raw.design,
            design_choices()
        ));
    }
    let design = raw.design.clone();
    let bar_align = BarAlign::from_str(&raw.bar_align).ok_or_else(|| {
        format!(
            "invalid bar_align: {:?} (want \"auto\" | \"center\" | \"left-of-notch\" | \"right-of-notch\")",
            raw.bar_align
        )
    })?;
    let compact_flock = CompactMode::from_str(&raw.compact_flock).ok_or_else(|| {
        format!(
            "invalid compact_flock: {:?} (want \"auto\" | \"always\" | \"never\")",
            raw.compact_flock
        )
    })?;

    Ok(Config {
        placement,
        design,
        reduce_motion: raw.reduce_motion,
        show_glyphs: raw.show_glyphs,
        bar_align,
        compact_flock,
        dock_x: validate_dock_coord("dock_x", raw.dock_x)?,
        dock_y: validate_dock_coord("dock_y", raw.dock_y)?,
        done_ttl_ms: raw.done_ttl_secs.saturating_mul(1000),
        session_ttl_ms: raw.session_ttl_secs.saturating_mul(1000),
        max_sessions: raw.max_sessions,
        detect_errors: raw.detect_errors,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn write_config(dir: &TempDir, content: &str) -> std::path::PathBuf {
        let p = dir.path().join("config.toml");
        fs::write(&p, content).unwrap();
        p
    }

    #[test]
    fn absent_file_returns_builtin_default() {
        let dir = TempDir::new().unwrap();
        let p = dir.path().join("config.toml"); // does not exist
        let cfg = load(&p).unwrap();
        assert_eq!(cfg, builtin_default());
    }

    #[test]
    fn builtin_default_values() {
        let cfg = builtin_default();
        assert_eq!(cfg.placement, Placement::Bar);
        assert_eq!(cfg.design, "egg");
        assert!(!cfg.reduce_motion);
        assert!(cfg.show_glyphs);
        assert_eq!(cfg.bar_align, BarAlign::Auto);
        assert_eq!(cfg.compact_flock, CompactMode::Auto);
        assert_eq!(cfg.done_ttl_ms(), 180_000);
        assert_eq!(cfg.session_ttl_ms(), 28_800_000);
        assert_eq!(cfg.max_sessions, 12);
        assert!(
            !cfg.detect_errors,
            "エラー検出は StopFailure に一本化したので既定は false"
        );
    }

    #[test]
    fn full_config_parses() {
        let dir = TempDir::new().unwrap();
        let p = write_config(
            &dir,
            r#"
placement = "dock"
design = "bean"
reduce_motion = true
show_glyphs = false
bar_align = "left-of-notch"
compact_flock = "always"
done_ttl_secs = 60
session_ttl_secs = 3600
max_sessions = 5
detect_errors = true
"#,
        );
        let cfg = load(&p).unwrap();
        assert_eq!(cfg.placement, Placement::Dock);
        assert_eq!(cfg.design, "bean");
        assert!(cfg.reduce_motion);
        assert!(!cfg.show_glyphs);
        assert_eq!(cfg.bar_align, BarAlign::LeftOfNotch);
        assert_eq!(cfg.compact_flock, CompactMode::Always);
        assert_eq!(cfg.done_ttl_ms(), 60_000);
        assert_eq!(cfg.session_ttl_ms(), 3_600_000);
        assert_eq!(cfg.max_sessions, 5);
        // 既定が false になったので、明示指定が既定を上書きできることを見る。
        assert!(cfg.detect_errors);
    }

    #[test]
    fn partial_config_fills_missing_with_defaults() {
        let dir = TempDir::new().unwrap();
        let p = write_config(&dir, "placement = \"dock\"\n");
        let cfg = load(&p).unwrap();
        assert_eq!(cfg.placement, Placement::Dock);
        assert_eq!(cfg.design, "egg", "unspecified design should default");
        assert_eq!(
            cfg.max_sessions, 12,
            "unspecified max_sessions should default"
        );
    }

    // ---- unknown enum values → Err --------------------------------------------------

    #[test]
    fn unknown_placement_returns_err() {
        let dir = TempDir::new().unwrap();
        let p = write_config(&dir, "placement = \"floating\"\n");
        let err = load(&p).unwrap_err();
        assert!(
            err.contains("placement"),
            "error should mention placement: {err}"
        );
    }

    /// **存在しない顔の id は `Err` にしない**。
    ///
    /// ユーザ顔（`~/.config/ccsessions/faces/*.toml`）の存在は設定のパース時点では
    /// 分からないので、実在チェックはレジストリ解決時に移した。ここを `Err` に
    /// 戻すと、ユーザが自分の顔を設定した瞬間に daemon が既定へ落ちてしまう。
    #[test]
    fn an_unknown_design_id_is_accepted_and_resolved_later() {
        let dir = TempDir::new().unwrap();
        let p = write_config(&dir, "design = \"my-own-face\"\n");
        let cfg = load(&p).expect("未知の顔 id は設定のパースでは弾かない");
        assert_eq!(cfg.design, "my-own-face");
        // 実在しないので、解決時に egg へ落ちる。
        let reg = crate::face::Registry::builtin();
        assert_eq!(reg.resolve(&cfg.design).id, "egg");
    }

    /// ただし**形が不正**なものはパース時に弾く（打ち間違いの早期発見）。
    #[test]
    fn a_malformed_design_id_returns_err() {
        let dir = TempDir::new().unwrap();
        let p = write_config(&dir, "design = \"My Face!\"\n");
        assert!(load(&p).unwrap_err().contains("design"));
    }

    /// 組込みの顔がすべて選択肢の案内に載っていること（案内と実装のズレ防止）。
    #[test]
    fn every_builtin_design_is_listed_in_the_choices() {
        let choices = design_choices();
        for id in crate::face::builtin_ids() {
            assert!(choices.contains(id), "{id} が選択肢の案内に載っていない");
            assert!(crate::face::Registry::builtin().get(id).is_some());
        }
    }

    /// **後方互換の番人**: `compact_flock` を書いていない既存の設定ファイルは
    /// 既定（`auto`）で読める。`auto` は「収まっているうちは縮めない」なので、
    /// 6 匹以下で使っている限り見た目は 0.1.0 と同じ。
    #[test]
    fn a_config_without_compact_flock_defaults_to_auto() {
        let dir = TempDir::new().unwrap();
        let p = write_config(
            &dir,
            "placement = \"bar\"\ndesign = \"egg\"\nmax_sessions = 12\n",
        );
        assert_eq!(load(&p).unwrap().compact_flock, CompactMode::Auto);
    }

    #[test]
    fn compact_flock_round_trips_through_from_str() {
        for m in [CompactMode::Auto, CompactMode::Always, CompactMode::Never] {
            assert_eq!(CompactMode::from_str(m.as_str()), Some(m));
        }
        assert_eq!(CompactMode::from_str("shrink"), None);
    }

    #[test]
    fn unknown_compact_flock_returns_err() {
        let dir = TempDir::new().unwrap();
        let p = write_config(&dir, "compact_flock = \"shrink\"\n");
        assert!(load(&p).unwrap_err().contains("compact_flock"));
    }

    #[test]
    fn unknown_bar_align_returns_err() {
        let dir = TempDir::new().unwrap();
        let p = write_config(&dir, "bar_align = \"top-left\"\n");
        assert!(load(&p).unwrap_err().contains("bar_align"));
    }

    #[test]
    fn malformed_toml_returns_err() {
        let dir = TempDir::new().unwrap();
        let p = write_config(&dir, "this is not toml [[[");
        assert!(load(&p).is_err());
    }

    #[test]
    fn save_then_load_round_trips() {
        let dir = TempDir::new().unwrap();
        let p = dir.path().join("config.toml");
        let mut cfg = builtin_default();
        cfg.placement = Placement::Dock;
        cfg.design = "squircle".to_string();
        cfg.reduce_motion = true;
        cfg.bar_align = BarAlign::Center;
        cfg.compact_flock = CompactMode::Never;
        cfg.set_done_ttl_secs(42);
        cfg.set_session_ttl_secs(4242);
        cfg.max_sessions = 3;
        // 既定と違う値にして、往復で実際に保持されることを見る。
        cfg.detect_errors = true;
        // 小数を含む値にするのが要点。`{}`（Display）で書くと 687.0 が `687` に
        // なって TOML 上は整数になるので、書式の取り違えをここで捕まえる。
        cfg.dock_x = Some(687.25);
        cfg.dock_y = Some(20.0);

        save(&p, &cfg).unwrap();
        let loaded = load(&p).unwrap();
        assert_eq!(loaded, cfg);
    }

    // ---- dock の位置 ------------------------------------------------------------------

    /// **後方互換の番人**: 位置を書いていない既存の設定ファイルは `None`
    /// ＝ 既定配置（画面下部中央）のまま。見た目は今までと完全に同じ。
    #[test]
    fn a_config_without_a_dock_position_has_none() {
        let dir = TempDir::new().unwrap();
        let p = write_config(&dir, "placement = \"dock\"\n");
        let cfg = load(&p).unwrap();
        assert_eq!((cfg.dock_x, cfg.dock_y), (None, None));
    }

    /// 既定のままなら**位置の行を書かない**。設定ファイルを開いたときに
    /// 「まだ動かしていない」ことがそのまま読み取れるようにするため。
    #[test]
    fn the_default_config_does_not_mention_a_dock_position() {
        let rendered = render_toml(&builtin_default());
        assert!(!rendered.contains("dock_x"), "既定なのに位置が書かれている");
        assert!(!rendered.contains("dock_y"));
    }

    /// **軸ごとに独立**。片方だけ書かれた設定ファイルはエラーにも「両方無視」にもせず、
    /// 書かれた軸だけ効かせる（`geometry::dock_rect_at` が他方を既定へ落とす）。
    #[test]
    fn one_dock_axis_may_be_specified_on_its_own() {
        let dir = TempDir::new().unwrap();
        let p = write_config(&dir, "dock_x = 500.0\n");
        let cfg = load(&p).unwrap();
        assert_eq!(cfg.dock_x, Some(500.0));
        assert_eq!(cfg.dock_y, None);
    }

    /// 整数で書かれていても f64 として読める（人が手で `dock_x = 500` と書く）。
    #[test]
    fn an_integer_dock_coord_is_accepted() {
        let dir = TempDir::new().unwrap();
        let p = write_config(&dir, "dock_x = 500\ndock_y = 20\n");
        let cfg = load(&p).unwrap();
        assert_eq!((cfg.dock_x, cfg.dock_y), (Some(500.0), Some(20.0)));
    }

    /// 有限でない値は `Err`。通すと窓の矩形が壊れる。
    #[test]
    fn a_non_finite_dock_coord_returns_err() {
        let dir = TempDir::new().unwrap();
        for src in ["dock_x = nan\n", "dock_y = inf\n", "dock_x = -inf\n"] {
            let p = write_config(&dir, src);
            let err = load(&p).unwrap_err();
            assert!(err.contains("dock_"), "理由が分からない: {err}（{src:?}）");
        }
    }

    #[test]
    fn saved_toml_keeps_option_comments() {
        // 設定ファイルとして選択肢が分かるよう、コメントが残っていること
        // （toml::to_string ではなくテンプレートを使う理由の直接確認）。
        let dir = TempDir::new().unwrap();
        let p = dir.path().join("config.toml");
        save(&p, &builtin_default()).unwrap();
        let content = fs::read_to_string(&p).unwrap();
        assert!(content.contains("\"bar\" | \"dock\""));
    }

    // ---- スキーマ（fields / field_value / set_field） ---------------------------------

    fn faces() -> crate::face::Registry {
        crate::face::Registry::builtin()
    }

    /// **設定画面はこの表だけを見て描く**ので、`Config` の全フィールドが表に
    /// 載っていなければ「設定ファイルにはあるのに画面に出ない」項目ができる。
    /// フィールド名を直接数えられないので、`render_toml` が書くキーと突き合わせる。
    #[test]
    fn every_key_written_to_the_toml_is_in_the_schema() {
        let mut cfg = builtin_default();
        // dock の位置は既定だと行ごと出ないので、値を入れて全キーを出させる。
        cfg.dock_x = Some(1.0);
        cfg.dock_y = Some(2.0);
        for line in render_toml(&cfg).lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let key = line.split('=').next().unwrap().trim();
            assert!(
                field(key).is_some(),
                "{key} が設定のスキーマに無い（設定画面に出ない）"
            );
        }
    }

    /// 逆向き: 表にあるキーは全部読めて、全部書ける。
    #[test]
    fn every_field_can_be_read_and_written() {
        let mut cfg = builtin_default();
        for f in fields() {
            let v = field_value(&cfg, f.key).unwrap_or_else(|| panic!("{} が読めない", f.key));
            set_field(&mut cfg, f.key, &v, &faces())
                .unwrap_or_else(|e| panic!("{} を書き戻せない: {e}", f.key));
        }
    }

    /// **読んだ値を書き戻すと元に戻る。** UI は現在値を入力欄に入れるので、
    /// これが崩れると「触っていないのに設定が変わる」。
    #[test]
    fn the_current_value_of_every_field_round_trips() {
        let mut cfg = builtin_default();
        cfg.placement = Placement::Dock;
        cfg.design = "bean".into();
        cfg.reduce_motion = true;
        cfg.show_glyphs = false;
        cfg.bar_align = BarAlign::LeftOfNotch;
        cfg.compact_flock = CompactMode::Always;
        cfg.set_done_ttl_secs(42);
        cfg.set_session_ttl_secs(4242);
        cfg.max_sessions = 3;
        cfg.detect_errors = true;
        cfg.dock_x = Some(687.25);
        cfg.dock_y = Some(20.0);

        let mut back = builtin_default();
        for f in fields() {
            let v = field_value(&cfg, f.key).unwrap();
            set_field(&mut back, f.key, &v, &faces()).unwrap();
        }
        assert_eq!(back, cfg);
    }

    /// 選択肢の宣言は実際に受け付ける値と一致していること
    /// （画面に出したのに保存できない、が起きない）。
    #[test]
    fn every_declared_choice_is_accepted() {
        let mut cfg = builtin_default();
        for f in fields() {
            let FieldKind::Choice(choices) = f.kind else {
                continue;
            };
            for (value, label) in choices {
                assert!(!label.is_empty(), "{} の選択肢にラベルが無い", f.key);
                set_field(&mut cfg, f.key, value, &faces())
                    .unwrap_or_else(|e| panic!("{}={value} が拒否された: {e}", f.key));
            }
        }
    }

    /// 組込みの顔はすべて `design` に設定できる。
    #[test]
    fn every_builtin_face_can_be_selected() {
        let mut cfg = builtin_default();
        for id in crate::face::builtin_ids() {
            set_field(&mut cfg, "design", id, &faces()).unwrap();
            assert_eq!(cfg.design, id);
        }
        assert!(set_field(&mut cfg, "design", "no-such-face", &faces()).is_err());
    }

    /// 範囲外の整数は書く時点で止める（`fields()` の宣言が唯一の情報源）。
    #[test]
    fn an_out_of_range_integer_is_refused() {
        let mut cfg = builtin_default();
        assert!(set_field(&mut cfg, "max_sessions", "0", &faces()).is_err());
        assert!(set_field(&mut cfg, "max_sessions", "9999", &faces()).is_err());
        assert!(set_field(&mut cfg, "session_ttl_secs", "1", &faces()).is_err());
        assert!(set_field(&mut cfg, "max_sessions", "8", &faces()).is_ok());
        assert_eq!(cfg.max_sessions, 8);
    }

    /// `auto` で dock の位置を既定へ戻せる（戻す口が消えない番人）。
    #[test]
    fn auto_clears_a_dock_coordinate() {
        let mut cfg = builtin_default();
        set_field(&mut cfg, "dock_x", "500.5", &faces()).unwrap();
        assert_eq!(cfg.dock_x, Some(500.5));
        set_field(&mut cfg, "dock_x", "auto", &faces()).unwrap();
        assert_eq!(cfg.dock_x, None);
        assert_eq!(field_value(&cfg, "dock_x").as_deref(), Some("auto"));
        assert!(set_field(&mut cfg, "dock_y", "nan", &faces()).is_err());
    }

    #[test]
    fn an_unknown_key_is_refused() {
        let mut cfg = builtin_default();
        assert!(set_field(&mut cfg, "nope", "1", &faces()).is_err());
        assert!(field_value(&cfg, "nope").is_none());
    }

    // ---- setters ----------------------------------------------------------------------

    #[test]
    fn set_done_ttl_secs_converts_to_ms() {
        let mut cfg = builtin_default();
        cfg.set_done_ttl_secs(5);
        assert_eq!(cfg.done_ttl_ms(), 5000);
    }

    #[test]
    fn set_session_ttl_secs_converts_to_ms() {
        let mut cfg = builtin_default();
        cfg.set_session_ttl_secs(7);
        assert_eq!(cfg.session_ttl_ms(), 7000);
    }
}
