//! オーバーレイ窓をどこに・どれだけの大きさで置くかの算術。
//!
//! **ここは純関数だけ**にしてある。AppKit から読んだ画面の実測値を `ScreenMetrics`
//! として受け取り、窓の矩形を返す。理由は 2 つ:
//! 1. ノッチ回避の場合分けが一番バグりやすい所なので、FFI 抜きでテストしたい。
//! 2. 「ノッチが無い Mac」「外部モニタ」「Dock が横にある」などの構成を、
//!    実機を持ち替えずに再現できるようにしたい。
//!
//! 座標系は AppKit のグローバル座標（**左下原点・y 上向き**、primary の左下が原点）。

use ccsessions_core::config::BarAlign;

/// AppKit から読んだ画面の実測値。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScreenMetrics {
    /// `NSScreen.frame` — 画面全体（メニューバー・Dock を含む）。
    pub frame: Rect,
    /// `NSScreen.visibleFrame` — メニューバーと Dock を除いた領域。
    pub visible: Rect,
    /// `NSScreen.safeAreaInsets.top` — ノッチ機なら > 0。無い機種は 0。
    pub safe_area_top: f64,
    /// `NSScreen.auxiliaryTopLeftArea` — ノッチ左の使用可能領域。ノッチ無し機では `None`。
    pub aux_top_left: Option<Rect>,
    /// `NSScreen.auxiliaryTopRightArea` — ノッチ右の使用可能領域。ノッチ無し機では `None`。
    pub aux_top_right: Option<Rect>,
    /// メニューエクストラ（時計・コントロールセンター等）の左端 x（**AppKit 座標**）。
    /// `screen.rs` が実測して詰める。測れない環境では `None`。
    pub menu_extra_left: Option<f64>,
}

/// 左下原点の矩形。`NSRect` の素朴な写し（FFI 型を geometry へ持ち込まないため）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    pub fn new(x: f64, y: f64, w: f64, h: f64) -> Self {
        Self { x, y, w, h }
    }
    pub fn max_x(&self) -> f64 {
        self.x + self.w
    }
    pub fn max_y(&self) -> f64 {
        self.y + self.h
    }
    /// 横方向にこの矩形の範囲内か。**NaN は必ず false**（比較が両方 false になる）。
    pub fn contains_x(&self, v: f64) -> bool {
        v >= self.x && v <= self.max_x()
    }
    pub fn contains_y(&self, v: f64) -> bool {
        v >= self.y && v <= self.max_y()
    }
}

impl ScreenMetrics {
    /// メニューバーの高さ（pt）。`frame` の上端と `visibleFrame` の上端の差。
    ///
    /// ノッチ機ではメニューバーが高い（実測 **33pt**）。`safeAreaInsets.top`
    /// とは 1pt ずれる（実測 32）ので、**帯高の基準にはこちらの差分を使う**こと。
    /// `NSStatusBar.thickness()` は 22 を返すが全く別の値なので使わない。
    /// メニューバー自動非表示のときは `visibleFrame` が画面全体になり差分が 0 になるため、
    /// `safe_area_top` を下限として採用する（帯が画面外へ飛ばないようにする保険）。
    pub fn menu_bar_height(&self) -> f64 {
        let by_visible = self.frame.max_y() - self.visible.max_y();
        by_visible.max(self.safe_area_top)
    }

    /// ノッチの水平範囲 `(左端 x, 右端 x)`。ノッチが無ければ `None`。
    ///
    /// AppKit は「ノッチの矩形」を直接は教えてくれない。代わりに、ノッチの左右で
    /// 使える領域（`auxiliaryTopLeftArea` / `auxiliaryTopRightArea`）を返す。
    /// その隙間がノッチである。
    pub fn notch_x_range(&self) -> Option<(f64, f64)> {
        match (self.aux_top_left, self.aux_top_right) {
            (Some(l), Some(r)) if r.x > l.max_x() => Some((l.max_x(), r.x)),
            _ => None,
        }
    }
}

/// bar 配置の帯（オーバーレイ窓）の矩形を決める。
///
/// `content_w` は生き物の群れが実際に必要とする幅。帯はその幅ぴったりに作る
/// （窓が大きいとその矩形ぶんメニューバーのクリックを奪ってしまうため。
/// 窓を content ぴったりにすれば、帯の外のメニューバー操作は無傷で残る）。
///
/// 水平位置は `align` で決める:
/// - `Center` … 画面中央（ノッチ機では隠れる。明示指定のときだけ使う）
/// - `LeftOfNotch` … ノッチの左隣に右揃えで寄せる
/// - `RightOfNotch` … ノッチの右隣に左揃えで寄せる
/// - `Auto` … ノッチが無ければ中央。ノッチに重なるなら、まず `RightOfNotch`（安定した
///   空き帯）へ、そこに入り切らない幅なら `LeftOfNotch`（広いが前面アプリのメニューと
///   競合しうる）へ逃がす。判断根拠は `resolve_align` 参照
///
/// 垂直位置はメニューバーの中に収める。帯高 `band_h` がメニューバー高を超える場合は
/// メニューバー高に切り詰める（バーからはみ出して下の画面を覆わないため）。
pub fn bar_rect(m: &ScreenMetrics, content_w: f64, band_h: f64, align: BarAlign) -> Rect {
    let bar_h = m.menu_bar_height();
    // メニューバーより高い帯は作らない。バー内に収まる範囲で最大を使う。
    //
    // **測れなかったとき（0）も切り詰める**。`menu_bar_height()` は
    // `(frame.max_y - visible.max_y).max(safe_area_top)` なので、**非ノッチ画面で
    // メニューバーが自動非表示だと 0 になる**（ノッチ機は safe_area_top=32 が保険に
    // なるが、非ノッチにはその下限が無い）。そのまま素通しすると帯が 32pt のまま
    // 画面最上端に置かれ、そこはメニューバーではなくアプリのコンテンツ領域なので
    // **幅 × 32pt ぶんのクリックを奪う**。
    let h = band_h.min(if bar_h > 0.0 {
        bar_h
    } else {
        crate::layout::FALLBACK_MENU_BAR_H
    });
    let y = m.frame.max_y() - h;

    let w = content_w.max(1.0);
    let centered = m.frame.x + (m.frame.w - w) / 2.0;

    let x = match resolve_align(m, w, align) {
        BarAlign::Center | BarAlign::Auto => centered,
        BarAlign::LeftOfNotch => match m.notch_x_range() {
            Some((notch_l, _)) => notch_l - GUTTER - w,
            None => centered,
        },
        BarAlign::RightOfNotch => match m.notch_x_range() {
            Some((_, notch_r)) => notch_r + GUTTER,
            None => centered,
        },
    };

    // 画面外へはみ出さないようクランプ（狭い外部モニタや幅広の群れの保険）。
    let x = x.clamp(m.frame.x, (m.frame.max_x() - w).max(m.frame.x));
    Rect::new(x, y, w, h)
}

/// ノッチと帯の間に空ける隙間（pt）。ノッチのすぐ脇に生き物が貼り付くと窮屈に見える。
const GUTTER: f64 = 8.0;

/// 幅 `w` の帯を画面中央に置いたとき、ノッチに重なるか。
///
/// 用途は 2 つ:
/// 1. `resolve_align` が `Auto` をノッチ避けの配置へ退避させるかどうかの判断。
/// 2. `bar_align = "center"` を明示指定したユーザに「この画面では群れが隠れる」と
///    起動時に警告する（`main::warn_if_centred_under_the_notch`）。**明示指定は
///    `resolve_align` が素通しするので、警告しなければ黙って見えなくなる。**
///
/// ノッチが無い画面では常に `false`。
pub fn center_hits_notch(m: &ScreenMetrics, w: f64) -> bool {
    let Some((notch_l, notch_r)) = m.notch_x_range() else {
        return false;
    };
    let left = m.frame.x + (m.frame.w - w) / 2.0;
    let right = left + w;
    !(right <= notch_l || left >= notch_r)
}

/// `Auto` を具体的な整列へ解決する。
///
/// ノッチが無い、または中央に置いた帯がノッチと重ならないなら `Center`
/// （判定は `center_hits_notch` に委ねる）。重なるなら `RightOfNotch` へ退避する。
///
/// **なぜ右か**（当初は左を選んでいたが、接地の実測で逆にした）:
/// - ノッチの**左**（x < 663）は**前面アプリのアプリメニュー**（アップルメニュー・
///   File・Edit …）の領域。これは Window Server の 1 枚の窓の内部描画なので窓一覧から
///   幅を測れず、**どのアプリが前面かで右端が動く**。Xcode のようにメニューが多いアプリで
///   生き物と重なる。
/// - ノッチの**右**はメニューエクストラ（コントロールセンター・時計等）の左端までが
///   空いており、その左端は**実行時に測れる**（`right_of_notch_reserve` ←
///   `screen::menu_extra_left`）。前面アプリに左右されない。
///
/// つまり右の方が「他人の都合で動かない」。ただし右の空きは有限なので、**それを超える
/// 群れは左へ逃がす** — 左は 663pt あり、アプリメニューと重なる危険はあるが、時計や
/// コントロールセンターの下に潜り込むよりはましだと判断した。
///
/// **かつてこの空きは「ユーザが自分で増減しない限り動かない」として 233pt の実測を
/// ベタ書きしていたが、その前提で固定してよいという結論は撤回した** — 同じ開発機で
/// 3 日のうちに 8pt 動いており（1081 → 1073）、安全余裕が尽きていた。「動かない」こと
/// 自体は正しくても、**その「動かない位置」は機械ごと・時期ごとに違う**。経緯は
/// `docs/adr/0012-notch-avoidance.md` の改訂節。
fn resolve_align(m: &ScreenMetrics, w: f64, align: BarAlign) -> BarAlign {
    if align != BarAlign::Auto {
        return align;
    }
    if !center_hits_notch(m, w) {
        return BarAlign::Center;
    }
    if w + GUTTER <= right_of_notch_reserve(m) {
        BarAlign::RightOfNotch
    } else {
        BarAlign::LeftOfNotch
    }
}

/// ノッチ右に確保できる幅（pt）。実測があればそれ、無ければ見積もりの定数。
///
/// `resolve_align`（右に置くか左へ逃がすかの判断）と `bar_available_width`
/// （縮小の判断に渡す使える幅）が**必ず同じ根拠を見る**ようにするための集約点。
/// ここが分かれると「右に入ると判断したのに左へ逃がす」ような矛盾が起きる。
///
/// ガードは **`x.is_finite() && x >= notch_r`**。壊れた実測（NaN・無限大・
/// ノッチより左）は定数側へ落とし、それ以外はどれだけ狭くてもそのまま使う。
///
/// **`>` ではなく `>=` にしてある。** `x == notch_r`（＝エクストラがノッチの
/// 右端にぴったり接している ＝ 空きゼロ）は**壊れた値ではなく正しい実測**で、
/// 定数へ落とすと 225pt 空いていることにしてエクストラの下へ群れを置いてしまう
/// （外れる方向が危険側）。`>=` なら空き 0 → `bar_available_width` が 0 →
/// `resolve_align` がノッチ左へ逃がす、と安全側に倒れる。
///
/// `is_finite` が要るのは、NaN が比較で false になるのに対し `INFINITY` は
/// `INFINITY > notch_r` が true になり、空き幅が無限大として素通りするため
/// （群れが縮まなくなり、エクストラの下へ置かれる）。
fn right_of_notch_reserve(m: &ScreenMetrics) -> f64 {
    match (m.notch_x_range(), m.menu_extra_left) {
        (Some((_, notch_r)), Some(x)) if x.is_finite() && x >= notch_r => x - notch_r,
        _ => MENU_EXTRA_RESERVE,
    }
}

/// ノッチの右に確保できると見込む幅（pt）。実測（`menu_extra_left`）が取れない
/// ときの**フォールバック**（非ノッチ機・メニューバー自動非表示・フルスクリーン・
/// エクストラ 0 個）。実測の経路は `screen::menu_extra_left` を参照。
///
/// 値は開発機の実測に由来する: メニューエクストラ（コントロールセンター・時計等）の
/// 左端が x=1081、ノッチ右端が x=848 だったので空きは 233pt。安全側に少し削って
/// 225 とする。**エクストラを増やして生き物と重なったら `bar_align` を
/// `left-of-notch` か `center` にすれば逃げられる**（設定 1 行）。
///
/// 目安: 生き物 1 匹 31pt + 余白 24pt なので、225pt には **6 匹**まで入る。
const MENU_EXTRA_RESERVE: f64 = 225.0;

/// bar 配置で群れが使える水平幅（pt）。コンパクト表示の判断
/// （`layout::squeeze`）に渡す。
///
/// **`bar_rect` が実際に置く場所ではなく「その配置で確保できる帯の幅」**を返す。
/// `resolve_align` は幅を見て置き場所を決めるので、幅の見積もりが置き場所に
/// 依存すると循環する。そこで**その整列の第一候補の幅**を返し、縮めた結果を
/// `bar_rect` に渡して改めて解決させる。
///
/// `Auto` がノッチ右の空き帯（`right_of_notch_reserve` — 実測があればそれ、
/// 無ければ `MENU_EXTRA_RESERVE`）を基準にするのが要点:
/// 今までは入り切らない群れをノッチ左へ逃がしていたが、左は前面アプリのメニュー
/// 領域で重なりうる。**縮めて右に収まるならその方が安全**なので、右の空き幅を
/// 「使える幅」として縮小の判断材料にする。それでも下限まで縮めて入らなければ
/// `resolve_align` が今までどおり左へ逃がす。
///
/// ノッチが無い画面では画面幅そのもの（メニューバーの右端に何が並んでいるかは
/// `NSScreen` からは測れないので、ここでは絞らない）。
pub fn bar_available_width(m: &ScreenMetrics, align: BarAlign) -> f64 {
    let Some((notch_l, _)) = m.notch_x_range() else {
        return m.frame.w;
    };
    match align {
        // 中央配置はノッチを跨ぐので幅で救えない（`center_hits_notch` が警告する）。
        BarAlign::Center => m.frame.w,
        BarAlign::LeftOfNotch => (notch_l - GUTTER - m.frame.x).max(0.0),
        BarAlign::Auto | BarAlign::RightOfNotch => (right_of_notch_reserve(m) - GUTTER).max(0.0),
    }
}

/// dock 配置で群れが使える水平幅（pt）。パネルの左右余白を除いた可視領域。
///
/// `pad_x` は `theme::DOCK_PAD_X`（呼び出し側から渡す — `geometry` は
/// デザイン定数に依存させない）。
pub fn dock_available_width(m: &ScreenMetrics, pad_x: f64) -> f64 {
    (m.visible.w - pad_x * 2.0).max(0.0)
}

/// dock 配置のパネル矩形を決める。画面下部中央、Dock の**上**に浮かせる。
///
/// 縦: `visibleFrame` の下端が「Dock を除いた使用可能領域の下端」なので、そこから
/// `bottom_margin` だけ上げる。Dock が左右にある構成では `visible.y` は画面下端と
/// 一致するので、同じ式のまま自然に画面下へ寄る。
///
/// 横: **画面全体の中央**に置く（`visibleFrame` の中央ではない）。Dock が横にあると
/// `visibleFrame` は片側に寄っているので、そちらを基準にするとパネルが視覚的に
/// 中央からずれてしまう。ただしパネルが広くて Dock に潜り込む場合だけ
/// `visibleFrame` の内側へクランプする — 採寸した環境の Dock は**右**にあり、
/// 幅 59pt を占めていた。
///
/// # 保存された位置
/// `x` は**パネルの中心**、`y` は**パネルの下端**（どちらも画面左下が原点の pt）。
/// ユーザがドラッグして決めた位置がここに入る。`None` の軸は上記の既定 — x なら
/// 画面全体の中央、y なら可視領域の下端から `bottom_margin` — に落ちる。軸ごとに
/// 独立しているのは、人が設定ファイルで片方だけ書いた場合に、書いた軸だけ効かせる
/// ため（`config::Config::dock_x` の doc 参照）。
///
/// # 画面内へのクランプ
/// **どちらの軸も必ず可視領域の内側へ収める。** パネルが画面外へ出ると掴み直せなくなり、
/// 操作不能になるため。可視領域より大きいパネルは潰さず**左端・下端へ寄せる**
/// （既定配置が横にある Dock を避けるときと同じ方針）。
///
/// クランプは**表示のたびに掛けるだけで、設定には書き戻さない**。書き戻すと、外部モニタを
/// 外して画面が狭くなったときに押し込まれた位置が確定してしまい、繋ぎ直しても元の位置へ
/// 戻れなくなる（`config::Config::dock_x` の doc と対）。
///
/// # 別のディスプレイに保存された位置
/// **保存位置が今の画面の外にあるなら、その軸は既定へ落とす**（クランプしない）。
/// `screen::metrics` が見る `NSScreen::mainScreen` は**フォーカス追従**なので、外部モニタの
/// アプリを触った瞬間に「今の画面」が変わる。そこで別画面の座標をクランプすると、
/// パネルが画面の隅に張り付いたまま貼り付く。既定へ落とせば、その画面での見た目は
/// **この機能を入れる前とまったく同じ**（下部中央）になる。
///
/// 判定は軸ごと。`frame`（`visible` ではない）で見るのは、下端の余白ぶんだけ外にある値を
/// 「別画面」と誤判定しないため。
///
/// 有限でない値は既定へ落とす。設定の読み込み時にも弾いているが、ここが窓の矩形を
/// 決める最後の関門なので二重に守る。
pub fn dock_rect(
    m: &ScreenMetrics,
    content_w: f64,
    content_h: f64,
    bottom_margin: f64,
    x: Option<f64>,
    y: Option<f64>,
) -> Rect {
    let w = content_w.max(1.0);
    let h = content_h.max(1.0);

    // x は「中心」で受けるので左端へ直す。既定は画面全体の中央（`visibleFrame` の
    // 中央ではない — 横に Dock があるとそちらは片寄っていて、視覚的な中央からずれる）。
    let left = match x.filter(|v| v.is_finite() && m.frame.contains_x(*v)) {
        Some(cx) => cx - w / 2.0,
        None => m.frame.x + (m.frame.w - w) / 2.0,
    };
    let bottom = match y.filter(|v| v.is_finite() && m.frame.contains_y(*v)) {
        Some(by) => by,
        None => m.visible.y + bottom_margin,
    };

    Rect::new(
        clamp_span(left, w, m.visible.x, m.visible.max_x()),
        clamp_span(bottom, h, m.visible.y, m.visible.max_y()),
        w,
        h,
    )
}

/// ホバーカードを画面内へ収める。
///
/// `want` は「本来出したい位置」（`Flock::card_origin` が返す、群れ窓を基準にした矩形を
/// グローバル座標へ直したもの）、`anchor` は群れ窓の矩形、`gap` はカードと窓の間隔。
///
/// # なぜ要るか
/// dock が**画面下部中央に固定だったころは要らなかった**。カードは常にパネルの上へ出れば
/// 収まり、水平方向も画面中央付近だったのではみ出しようがなかった。**ドラッグで置き場所が
/// 自由になった瞬間にその前提が消える** — パネルを画面上端へ寄せればカードは画面外（や
/// メニューバーの裏）へ出て読めなくなり、左右の端へ寄せれば横にはみ出す。
///
/// 縦は**まず反転を試す**（上に入らなければ窓の下へ回す）。単にクランプすると、カードが
/// 群れに被さって生き物が読めなくなるため。下にも入らなければクランプで妥協する。
pub fn fit_card_on_screen(m: &ScreenMetrics, want: Rect, anchor: Rect, gap: f64) -> Rect {
    let mut r = want;
    if r.max_y() > m.visible.max_y() {
        let below = anchor.y - gap - r.h;
        if below >= m.visible.y {
            r = Rect::new(r.x, below, r.w, r.h);
        }
    }
    Rect::new(
        clamp_span(r.x, r.w, m.visible.x, m.visible.max_x()),
        clamp_span(r.y, r.h, m.visible.y, m.visible.max_y()),
        r.w,
        r.h,
    )
}

/// 長さ `len` の区間の開始位置 `v` を `[lo, hi]` の内側へ収める。
///
/// `len` が `hi - lo` より大きいときは `lo`（＝はみ出すぶんは終端側へ逃がす）。
///
/// **`f64::clamp` を使わない。** `clamp` は `min > max` **または どちらかが NaN** のときに
/// パニックする。`hi` 側は `.max(lo)` が NaN を吸うが、`lo`（＝ AppKit から読んだ
/// `visible.x`/`visible.y`）が NaN だと落ちる。ここは窓の矩形を決める最後の関門で、
/// 画面情報が壊れているときに daemon ごと落とす価値は無いので、`max`/`min`
/// （NaN を渡すともう一方を返す）で組んでパニック不能にする。
fn clamp_span(v: f64, len: f64, lo: f64, hi: f64) -> f64 {
    let upper = (hi - len).max(lo);
    v.max(lo).min(upper)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 14 インチのノッチ機（内蔵ディスプレイ）の**実測値**。
    ///
    /// `ccsessionsd` 起動時のログから採取したそのままの数字:
    /// ```text
    /// frame=1512x982 visible=(0,0,1455x949) safe_top=32 menu_bar=33
    /// aux_l=(0,950,663x32) aux_r=(848,950,664x32) notch=(663,848)
    /// ```
    /// `visible.w` が 1455（< 1512）なのは Dock が画面の横にあるため。
    fn notched() -> ScreenMetrics {
        ScreenMetrics {
            frame: Rect::new(0.0, 0.0, 1512.0, 982.0),
            visible: Rect::new(0.0, 0.0, 1455.0, 949.0),
            safe_area_top: 32.0,
            aux_top_left: Some(Rect::new(0.0, 950.0, 663.0, 32.0)),
            aux_top_right: Some(Rect::new(848.0, 950.0, 664.0, 32.0)),
            menu_extra_left: None,
        }
    }

    /// ノッチ無し（外部モニタ・古い MacBook）。
    fn plain() -> ScreenMetrics {
        ScreenMetrics {
            frame: Rect::new(0.0, 0.0, 1920.0, 1080.0),
            visible: Rect::new(0.0, 0.0, 1920.0, 1056.0),
            safe_area_top: 0.0,
            aux_top_left: None,
            aux_top_right: None,
            menu_extra_left: None,
        }
    }

    #[test]
    fn menu_bar_height_uses_the_larger_of_visible_gap_and_safe_area() {
        // 実測: frame の上端 982 − visible の上端 949 = 33pt。
        assert_eq!(notched().menu_bar_height(), 33.0);
        assert_eq!(plain().menu_bar_height(), 24.0);
        // メニューバー自動非表示（visible が画面全体）でも safe area(32) が下限になる。
        let mut m = notched();
        m.visible = m.frame;
        assert_eq!(m.menu_bar_height(), 32.0);
    }

    #[test]
    fn notch_range_is_the_gap_between_the_auxiliary_areas() {
        // 実測: 左の使用可能域は x<663、右は x>=848。その隙間 185pt がノッチ。
        assert_eq!(notched().notch_x_range(), Some((663.0, 848.0)));
        assert_eq!(plain().notch_x_range(), None);
    }

    /// ノッチ機で Auto にすると、ノッチの**右隣**へ退避する。
    /// （左はアプリメニュー領域で前面アプリ次第で動くため — `resolve_align` 参照）
    #[test]
    fn auto_dodges_the_notch_to_the_right() {
        let m = notched();
        let r = bar_rect(&m, 195.0, 26.0, BarAlign::Auto);
        assert_eq!(r.x, 848.0 + 8.0);
        assert!(r.max_x() < 1081.0, "メニューエクストラに掛かる: {r:?}");
    }

    /// 右の空き帯（メニューエクストラまで）に入り切らない群れは、広い左側へ逃がす。
    /// そのまま右に置き続けると時計やコントロールセンターの下に潜り込むため。
    #[test]
    fn auto_falls_back_to_the_left_when_too_wide_for_the_right_gap() {
        let m = notched();
        let r = bar_rect(&m, 400.0, 26.0, BarAlign::Auto);
        assert_eq!(r.max_x(), 663.0 - 8.0, "左へ逃げていない: {r:?}");
    }

    /// 6 匹（実測 195pt）は右に収まり、7 匹（226pt）で左へ切り替わる — 境界の固定。
    #[test]
    fn the_right_gap_holds_six_creatures() {
        let m = notched();
        assert!(bar_rect(&m, 195.0, 26.0, BarAlign::Auto).x > 848.0);
        assert!(bar_rect(&m, 226.0, 26.0, BarAlign::Auto).x < 663.0);
    }

    /// 帯が細くてノッチと重ならないなら Auto は中央のまま。
    /// ノッチ幅 200pt に対し幅 100pt の帯は中央 (706..806) に収まる。
    #[test]
    fn auto_stays_centred_when_it_fits_beside_the_notch() {
        let m = ScreenMetrics {
            aux_top_left: Some(Rect::new(0.0, 950.0, 700.0, 32.0)),
            aux_top_right: Some(Rect::new(812.0, 950.0, 700.0, 32.0)),
            ..notched()
        };
        // ノッチは 700..812。幅 80 の帯は中央 716..796 で内側 → 重なるので退避する。
        let r = bar_rect(&m, 80.0, 26.0, BarAlign::Auto);
        assert!(r.x >= 812.0);
    }

    /// ノッチが無い画面では Auto は素直に中央。
    #[test]
    fn auto_is_centred_without_a_notch() {
        let r = bar_rect(&plain(), 300.0, 26.0, BarAlign::Auto);
        assert_eq!(r.x, (1920.0 - 300.0) / 2.0);
    }

    /// **ノッチが画面の水平中央にある以上、中央配置は幅によらずノッチに掛かる。**
    ///
    /// `bar_align = "center"` の明示指定は `resolve_align` が素通しするので、
    /// これが `warn_if_centred_under_the_notch` の警告条件そのものになる。
    /// ノッチが無い画面では常に `false`（警告しない）。
    #[test]
    fn centring_hits_the_notch_whenever_the_notch_straddles_the_screen_centre() {
        let m = notched();
        // 実測のノッチ (663, 848) は画面中央 760 を跨ぐので、細い群れでも掛かる。
        for w in [1.0_f64, 40.0, 195.0, 400.0, 1000.0] {
            assert!(center_hits_notch(&m, w), "幅 {w} で掛からない判定になった");
        }
        // ノッチが無ければどれだけ広くても掛からない。
        for w in [1.0_f64, 400.0, 1900.0] {
            assert!(!center_hits_notch(&plain(), w));
        }
    }

    /// 帯はメニューバーの中に収まり、はみ出さない。
    #[test]
    fn band_is_clipped_to_the_menu_bar_and_sits_at_the_top() {
        let m = notched();
        let r = bar_rect(&m, 200.0, 26.0, BarAlign::Auto);
        assert_eq!(r.h, 26.0);
        assert_eq!(r.max_y(), 982.0);
        // 帯の要求高がメニューバーを超えたらメニューバー高に切り詰められる。
        assert_eq!(bar_rect(&m, 200.0, 50.0, BarAlign::Auto).h, 33.0);
        // メニューバーが 26pt より低い画面では帯もそこまで縮む。
        let r = bar_rect(&plain(), 200.0, 26.0, BarAlign::Auto);
        assert_eq!(r.h, 24.0);
        assert_eq!(r.max_y(), 1080.0);
    }

    /// 明示指定はノッチの有無に関わらず尊重される（ノッチ無しでは中央にフォールバック）。
    #[test]
    fn explicit_alignments_are_honoured() {
        let m = notched();
        let l = bar_rect(&m, 200.0, 26.0, BarAlign::LeftOfNotch);
        assert_eq!(l.max_x(), 663.0 - 8.0);
        let r = bar_rect(&m, 200.0, 26.0, BarAlign::RightOfNotch);
        assert_eq!(r.x, 848.0 + 8.0);
        let c = bar_rect(&m, 200.0, 26.0, BarAlign::Center);
        assert_eq!(c.x, (1512.0 - 200.0) / 2.0);
        // ノッチ無しでは left/right 指定も中央に落ちる。
        assert_eq!(
            bar_rect(&plain(), 200.0, 26.0, BarAlign::LeftOfNotch).x,
            (1920.0 - 200.0) / 2.0
        );
    }

    /// 画面より広い群れは画面内にクランプされる（左端に貼り付く）。
    #[test]
    fn oversized_flock_is_clamped_on_screen() {
        let r = bar_rect(&notched(), 4000.0, 26.0, BarAlign::Auto);
        assert_eq!(r.x, 0.0);
    }

    // ---- 使える幅（コンパクト表示の判断材料）------------------------------

    /// `Auto` はノッチ右の空き帯を基準にする。**`the_right_gap_holds_six_creatures`
    /// が固定している境界（195pt は入る / 226pt は入らない）と同じ幅**でなければ、
    /// 「縮小したのに結局左へ逃げる」「縮小が要らないのに縮む」のどちらかが起きる。
    #[test]
    fn the_available_width_for_auto_is_the_gap_right_of_the_notch() {
        let m = notched();
        let w = bar_available_width(&m, BarAlign::Auto);
        assert_eq!(w, MENU_EXTRA_RESERVE - GUTTER);
        // 6 匹（195pt）は入り、7 匹（226pt）は入らない。
        assert!(195.0 <= w);
        assert!(226.0 > w);
        // `resolve_align` の判定と一致する（幅 w までは右に置かれる）。
        assert!(bar_rect(&m, w, 26.0, BarAlign::Auto).x > 848.0);
    }

    /// 実測（`menu_extra_left`）があれば `bar_available_width(Auto)` はそれに従う。
    #[test]
    fn a_measured_menu_extra_left_drives_the_available_width() {
        let m = ScreenMetrics {
            // ノッチ右端 848 + 52pt だけ空いている、という実測。
            menu_extra_left: Some(900.0),
            ..notched()
        };
        assert_eq!(
            bar_available_width(&m, BarAlign::Auto),
            900.0 - 848.0 - GUTTER
        );
    }

    /// **回帰の番人**: 実測が無ければ現行と同じ値（217）を返す。
    ///
    /// 期待値は `MENU_EXTRA_RESERVE` から導かず固定値で書く。定数を参照すると、
    /// 定数を書き換えたときに期待値も一緒に動いて回帰を検出できなくなる。
    #[test]
    fn without_a_measurement_the_available_width_matches_the_current_fallback() {
        assert_eq!(bar_available_width(&notched(), BarAlign::Auto), 217.0);
    }

    /// 実測は `resolve_align` にも効く。定数のままなら右に収まる幅（200pt）でも、
    /// 実測がそれより狭ければノッチ左へ逃がす。
    #[test]
    fn a_narrower_measurement_pushes_a_flock_that_would_fit_under_the_constant_to_the_left() {
        let m = ScreenMetrics {
            // ノッチ右端 848 + 190pt しか空いていない、という実測（定数 225 より狭い）。
            menu_extra_left: Some(848.0 + 190.0),
            ..notched()
        };
        // 定数のままなら 200pt は右に収まる（200 + GUTTER = 208 <= 225）。
        let r = bar_rect(&m, 200.0, 26.0, BarAlign::Auto);
        assert_eq!(
            r.max_x(),
            663.0 - 8.0,
            "実測の狭い空きに追従せず右へ置かれている: {r:?}"
        );
    }

    /// **空きゼロは「壊れた値」ではない。** エクストラがノッチ右端にぴったり
    /// 接している実測は正しい値なので、定数へ落とさず 0 として扱う。落とすと
    /// 「225pt 空いている」ことにしてエクストラの下へ群れを置く（危険側に外れる）。
    #[test]
    fn a_measurement_flush_against_the_notch_means_zero_room_not_a_broken_value() {
        let m = ScreenMetrics {
            menu_extra_left: Some(848.0), // ノッチ右端ちょうど
            ..notched()
        };
        assert_eq!(bar_available_width(&m, BarAlign::Auto), 0.0);
        // 幅ゼロでは右に置けないので、ノッチ左へ逃がす。
        let r = bar_rect(&m, 120.0, 26.0, BarAlign::Auto);
        assert_eq!(r.max_x(), 663.0 - 8.0, "右に置かれている: {r:?}");
    }

    /// 実測が壊れた値（NaN・無限大・負・ノッチ右端より左）でもパニックせず、定数側へ落ちる。
    #[test]
    fn a_broken_measurement_falls_back_to_the_constant_without_panicking() {
        for bad in [
            f64::NAN,
            f64::INFINITY,
            -100.0,
            800.0, /* ノッチ右端 848 より左 */
        ] {
            let m = ScreenMetrics {
                menu_extra_left: Some(bad),
                ..notched()
            };
            assert_eq!(
                bar_available_width(&m, BarAlign::Auto),
                MENU_EXTRA_RESERVE - GUTTER,
                "{bad} で定数へ落ちていない"
            );
        }
    }

    /// 明示的な左寄せでは、ノッチ左のアプリメニュー領域まるごとが使える幅。
    #[test]
    fn the_available_width_left_of_the_notch_is_the_wider_area() {
        let m = notched();
        let left = bar_available_width(&m, BarAlign::LeftOfNotch);
        assert_eq!(left, 663.0 - 8.0);
        assert!(
            left > bar_available_width(&m, BarAlign::Auto),
            "左の方が広いという前提が崩れている"
        );
    }

    /// ノッチが無い画面では画面幅そのもの（絞る根拠が測れない）。
    #[test]
    fn a_screen_without_a_notch_offers_its_full_width() {
        for align in [
            BarAlign::Auto,
            BarAlign::Center,
            BarAlign::LeftOfNotch,
            BarAlign::RightOfNotch,
        ] {
            assert_eq!(bar_available_width(&plain(), align), 1920.0);
        }
    }

    /// dock はパネルの左右余白を除いた可視領域。負にはならない。
    #[test]
    fn the_dock_available_width_excludes_the_panel_padding() {
        // 実測: visible.w = 1455（右 59pt が Dock）。
        assert_eq!(dock_available_width(&notched(), 19.0), 1455.0 - 38.0);
        assert_eq!(dock_available_width(&notched(), 10_000.0), 0.0);
    }

    /// dock パネルは Dock の上に浮き、横位置は**画面全体の中央**になる。
    #[test]
    fn dock_panel_floats_above_the_dock() {
        let m = ScreenMetrics {
            visible: Rect::new(0.0, 70.0, 1512.0, 879.0),
            ..notched()
        };
        let r = dock_rect(&m, 400.0, 80.0, 20.0, None, None);
        assert_eq!(r.y, 90.0);
        assert_eq!(r.x, (1512.0 - 400.0) / 2.0);
    }

    /// 横にある Dock（採寸した環境では右・幅 59pt）の下にパネルが潜り込まない。
    /// 通常幅なら画面中央のまま、可視領域からはみ出す幅のときだけクランプされる。
    #[test]
    fn dock_panel_avoids_a_side_dock() {
        let m = notched(); // visible = (0,0,1455x949) — 右 59pt が Dock
                           // 通常幅: 画面中央のまま（Dock には届かない）
        let r = dock_rect(&m, 400.0, 80.0, 20.0, None, None);
        assert_eq!(r.x, (1512.0 - 400.0) / 2.0);
        assert!(r.max_x() <= 1455.0);
        // 可視領域いっぱいに近い幅: Dock に食い込まないよう左へ寄る
        let wide = dock_rect(&m, 1440.0, 80.0, 20.0, None, None);
        assert!(
            wide.max_x() <= 1455.0,
            "Dock の下に潜り込んでいる: {wide:?}"
        );
    }

    // ---- ドラッグして決めた位置 -------------------------------------------------

    /// 保存された位置は**中心 x・下端 y**として解釈される。
    #[test]
    fn a_saved_dock_position_is_honoured() {
        let m = notched();
        let r = dock_rect(&m, 400.0, 80.0, 20.0, Some(500.0), Some(300.0));
        assert_eq!(r.x, 300.0, "中心 500 ならば左端は 500 - 400/2");
        assert_eq!(r.y, 300.0, "y は下端そのもの");
        assert_eq!((r.w, r.h), (400.0, 80.0));
    }

    /// 位置を持たない軸は既定へ落ちる。**軸ごとに独立**であることの番人
    /// （人が設定ファイルで片方だけ書いた場合に、書いた軸だけ効く）。
    #[test]
    fn an_unset_axis_falls_back_to_the_default_placement() {
        let m = ScreenMetrics {
            visible: Rect::new(0.0, 70.0, 1512.0, 879.0),
            ..notched()
        };
        let default = dock_rect(&m, 400.0, 80.0, 20.0, None, None);

        let only_x = dock_rect(&m, 400.0, 80.0, 20.0, Some(500.0), None);
        assert_eq!(only_x.x, 300.0);
        assert_eq!(only_x.y, default.y, "y は既定のまま");

        let only_y = dock_rect(&m, 400.0, 80.0, 20.0, None, Some(400.0));
        assert_eq!(only_y.x, default.x, "x は既定のまま");
        assert_eq!(only_y.y, 400.0);
    }

    /// どこへ振っても可視領域の内側に留まる（掴み直せなくならない）。
    #[test]
    fn a_saved_dock_position_is_clamped_into_the_visible_area() {
        let m = notched(); // visible = (0,0,1455x949)
        let (w, h) = (400.0, 80.0);
        for (cx, cy) in [
            (-10_000.0, -10_000.0),
            (10_000.0, 10_000.0),
            (0.0, 0.0),
            (1455.0, 949.0),
        ] {
            let r = dock_rect(&m, w, h, 20.0, Some(cx), Some(cy));
            assert!(
                r.x >= m.visible.x && r.max_x() <= m.visible.max_x(),
                "横がはみ出している: {r:?}（cx={cx}）"
            );
            assert!(
                r.y >= m.visible.y && r.max_y() <= m.visible.max_y(),
                "縦がはみ出している: {r:?}（cy={cy}）"
            );
        }
    }

    /// 匹数が増えてパネルが広がっても**中心 x が動かない**。
    ///
    /// 左端で位置を持つとここが崩れる（幅が増えたぶん右へずれて、置いた位置の
    /// 印象がずれる）。中心をアンカーにした判断の番人。
    #[test]
    fn a_saved_dock_position_keeps_its_centre_when_the_flock_grows() {
        let m = notched();
        let centre = 600.0;
        let narrow = dock_rect(&m, 200.0, 80.0, 20.0, Some(centre), Some(300.0));
        let wide = dock_rect(&m, 500.0, 80.0, 20.0, Some(centre), Some(300.0));
        assert_eq!(narrow.x + narrow.w / 2.0, centre);
        assert_eq!(wide.x + wide.w / 2.0, centre);
    }

    /// 可視領域より大きいパネルは**潰さず左下へ寄せる**（既定配置と同じ方針）。
    #[test]
    fn a_panel_larger_than_the_screen_is_pinned_to_the_corner() {
        let m = notched();
        let r = dock_rect(&m, 3000.0, 2000.0, 20.0, Some(600.0), Some(300.0));
        assert_eq!((r.x, r.y), (m.visible.x, m.visible.y));
        assert_eq!((r.w, r.h), (3000.0, 2000.0), "潰さない");
    }

    /// 別のディスプレイに保存された位置は、隅へクランプせず**既定へ落ちる**。
    ///
    /// `NSScreen::mainScreen` はフォーカス追従なので、外部モニタのアプリを触った瞬間に
    /// 「今の画面」が変わる。そこで別画面の座標をクランプすると、この機能を入れる前には
    /// 無かった「パネルが画面の隅に張り付く」退化が起きる。
    #[test]
    fn a_position_saved_on_another_display_falls_back_to_the_default() {
        let m = notched(); // frame = (0,0,1512x982)
        let default = dock_rect(&m, 400.0, 80.0, 20.0, None, None);

        // 右隣の外部モニタ上の座標。
        let elsewhere = dock_rect(&m, 400.0, 80.0, 20.0, Some(2500.0), Some(400.0));
        assert_eq!(elsewhere.x, default.x, "隅に張り付かず既定の中央へ");

        // 縦だけ別画面（上下に並べた構成）。
        let above = dock_rect(&m, 400.0, 80.0, 20.0, Some(600.0), Some(1500.0));
        assert_eq!(above.y, default.y, "y だけ既定へ落ちる");
        assert_eq!(above.x, 400.0, "x は同じ画面内なので活きる");
    }

    /// 画面内の位置は（`visible` の外＝下端の余白ぶんでも）既定へ落とさずクランプする。
    /// `frame` で判定している理由の番人。
    #[test]
    fn a_position_inside_the_screen_is_clamped_not_discarded() {
        let m = ScreenMetrics {
            visible: Rect::new(0.0, 70.0, 1512.0, 879.0), // 下 70pt が Dock
            ..notched()
        };
        // y=10 は visible の外だが frame の中 → クランプされて visible の下端に載る。
        let r = dock_rect(&m, 400.0, 80.0, 20.0, Some(600.0), Some(10.0));
        assert_eq!(r.y, m.visible.y);
        assert_eq!(r.x, 400.0, "x は保存値のまま");
    }

    // ---- ホバーカードの収まり ------------------------------------------------

    /// dock を画面上端へ寄せると、カードは**パネルの下へ反転**する。
    ///
    /// dock が下部中央固定だったころは「常にパネルの上」で収まっていた。ドラッグで
    /// 置き場所が自由になった瞬間にその前提が消える、という退化の番人。
    #[test]
    fn a_card_that_would_leave_the_top_of_the_screen_flips_below_the_panel() {
        let m = notched();
        // パネルを可視領域の上端いっぱいに置く。
        let anchor = Rect::new(600.0, 820.0, 358.0, 120.0);
        // 本来出したい位置＝パネルの上（8pt 空けて高さ 60）。上端を突き抜ける。
        let want = Rect::new(600.0, anchor.max_y() + 8.0, 240.0, 60.0);
        assert!(want.max_y() > m.visible.max_y(), "前提: 上へはみ出している");

        let r = fit_card_on_screen(&m, want, anchor, 8.0);
        assert_eq!(r.y, anchor.y - 8.0 - 60.0, "パネルの下へ反転する");
        assert!(r.max_y() <= m.visible.max_y());
        assert!(r.y >= m.visible.y);
    }

    /// 左右にはみ出すカードは画面内へ寄せる（1 匹だけ・長い名前で起きる）。
    #[test]
    fn a_card_is_clamped_into_the_screen_horizontally() {
        let m = notched();
        let anchor = Rect::new(0.0, 20.0, 90.0, 120.0); // 左端に置いた細いパネル
        let want = Rect::new(-75.0, 148.0, 240.0, 60.0); // 生き物中心に揃えると負の x
        let r = fit_card_on_screen(&m, want, anchor, 8.0);
        assert_eq!(r.x, m.visible.x);
        assert!(r.max_x() <= m.visible.max_x());

        // 右端でも同様。
        let anchor_r = Rect::new(1363.0, 20.0, 90.0, 120.0);
        let want_r = Rect::new(1400.0, 148.0, 240.0, 60.0);
        let rr = fit_card_on_screen(&m, want_r, anchor_r, 8.0);
        assert!(rr.max_x() <= m.visible.max_x());
    }

    /// 収まっているカードは**一切動かさない**（既存の見た目を変えないことの番人）。
    #[test]
    fn a_card_that_already_fits_is_left_alone() {
        let m = notched();
        let anchor = Rect::new(577.0, 20.0, 358.0, 120.0); // 既定配置
        let want = Rect::new(600.0, 148.0, 240.0, 60.0);
        assert_eq!(fit_card_on_screen(&m, want, anchor, 8.0), want);
    }

    /// `clamp_span` は画面情報が壊れていてもパニックしない（m4）。
    ///
    /// `f64::clamp` は境界のどちらかが NaN でも落ちる。ここは窓の矩形を決める最後の
    /// 関門なので、daemon ごと落とすより既定側へ倒す。
    #[test]
    fn a_broken_screen_rect_does_not_panic() {
        let broken = ScreenMetrics {
            visible: Rect::new(f64::NAN, f64::NAN, f64::NAN, f64::NAN),
            ..notched()
        };
        let _ = dock_rect(&broken, 400.0, 80.0, 20.0, Some(600.0), Some(300.0));
        let _ = fit_card_on_screen(
            &broken,
            Rect::new(0.0, 0.0, 10.0, 10.0),
            Rect::new(0.0, 0.0, 10.0, 10.0),
            8.0,
        );
    }

    /// 有限でない座標は既定配置へ落ちる（窓の矩形を決める最後の関門としての二重防御）。
    #[test]
    fn a_non_finite_saved_position_falls_back_to_the_default() {
        let m = notched();
        let default = dock_rect(&m, 400.0, 80.0, 20.0, None, None);
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let r = dock_rect(&m, 400.0, 80.0, 20.0, Some(bad), Some(bad));
            assert_eq!(r, default, "{bad} で既定へ落ちていない");
        }
    }
}
