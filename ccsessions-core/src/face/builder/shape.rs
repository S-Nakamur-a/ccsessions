//! パーツを形に起こす幾何カーネル（純関数のみ）。
//!
//! ここには**パーツの中身（どんな髪型があるか）は一切書かない**。それは
//! `parts.rs` の表の仕事で、こちらは「表に書いた数値をどうやって
//! `Seg` や `[f64; 2]` の列にするか」だけを持つ。パーツを 1 つ足すときに
//! 触るのは表だけ、というのがこの分割の目的。
//!
//! # 座標系
//! `face/mod.rs` と同じ **0..1 比率・左下原点・y は上向き**。

use crate::face::{flatten, FaceSpec, Seg, Size};

/// 輪郭を折れ線に潰すときの分割数。`validate.rs` の `FLATTEN_STEPS` と揃える。
const FLATTEN_STEPS: usize = 24;

/// 0..1 に丸める。**生成した座標は例外なくここを通す** — 輪郭が体の矩形に
/// 収まる）は制御点も見るので、ベジェのオーバーシュートを箱の中へ押し戻す必要がある。
/// 制御点を箱にクランプすれば凸包性より曲線も箱に収まる。
pub fn clamp01(v: f64) -> f64 {
    v.clamp(0.0, 1.0)
}

fn clamp_pt((x, y): (f64, f64)) -> (f64, f64) {
    (clamp01(x), clamp01(y))
}

// ---------------------------------------------------------------------------
// 経由点 → 3 次ベジェ
// ---------------------------------------------------------------------------

/// 経由点の列を滑らかな 3 次ベジェの列にする（Catmull-Rom → Bezier）。
///
/// `pts[0]` が始点で、返す `Seg` を順に辿ると `pts` の最後の点に着く
/// （`OutlineSpec::Path` の `start` + `segs` にそのまま渡せる形）。
///
/// # 両端で「鏡像の相手」を接線に使う
/// この関数は `half = true` の輪郭（右半分だけ書いて左は鏡像）のために使う。
/// 端点の接線を素朴に「隣の点との差」で取ると、鏡像と接ぐ**あご先と頭頂に角が立つ**。
/// そこで端の外側の仮想点を **x = 0.5 に対する鏡像**（`(1 - u, v)`）に取る。
/// 実際に貼り合わされる相手そのものなので、継ぎ目の接線が水平になり滑らかにつながる。
pub fn smooth_path(pts: &[(f64, f64)]) -> Vec<Seg> {
    if pts.len() < 2 {
        return Vec::new();
    }
    let mirror = |(u, v): (f64, f64)| (1.0 - u, v);
    let at = |i: isize| -> (f64, f64) {
        if i < 0 {
            mirror(pts[1])
        } else if i as usize >= pts.len() {
            mirror(pts[pts.len() - 2])
        } else {
            pts[i as usize]
        }
    };

    let mut segs = Vec::with_capacity(pts.len() - 1);
    for i in 0..pts.len() - 1 {
        let p0 = at(i as isize - 1);
        let p1 = pts[i];
        let p2 = pts[i + 1];
        let p3 = at(i as isize + 2);
        segs.push(Seg::Cubic {
            c1: clamp_pt((p1.0 + (p2.0 - p0.0) / 6.0, p1.1 + (p2.1 - p0.1) / 6.0)),
            c2: clamp_pt((p2.0 - (p3.0 - p1.0) / 6.0, p2.1 - (p3.1 - p1.1) / 6.0)),
            to: clamp_pt(p2),
        });
    }
    segs
}

/// 2 次ベジェを 1 点評価する。目のまぶた曲線に使う。
pub fn quad(a: (f64, f64), b: (f64, f64), c: (f64, f64), t: f64) -> (f64, f64) {
    let u = 1.0 - t;
    (
        u * u * a.0 + 2.0 * u * t * b.0 + t * t * c.0,
        u * u * a.1 + 2.0 * u * t * b.1 + t * t * c.1,
    )
}

// ---------------------------------------------------------------------------
// 輪郭の半幅
// ---------------------------------------------------------------------------

/// パネル線を輪郭からどれだけ内側に置くか。
///
/// 枠線は 1.5pt 幅でパスの上に中心を置くので、縁ちょうどに線を引くと枠と
/// 重なって**輪郭が二重線に見える**（`faces/README.md` §5 と同じ話）。
/// 0.80 は手描きの顔のパネル線が実際に取っている内寄せ量に合わせた値。
pub const DETAIL_INSET: f64 = 0.80;

/// 断面表の標本数（v = 0/N .. N/N）。
const PROFILE_N: usize = 100;

/// パネル線を置いてよいとみなす最小の半幅（体幅に対する比率）。
/// これを下回る高さは「顔がそこに無い」とみなして避ける。
const MIN_ROOM: f64 = 0.04;

/// 顔の**断面表** — 高さごとに中心から縁までどれだけ使えるか。
///
/// # なぜ要るか
/// パネル線（髪・口・鼻）の幅を「体幅の何割」で決めると、細あごの顔で口が
/// 顔からはみ出し、幅広の顔では逆にすかすかになる。**その高さで実際に
/// 顔がどれだけ広いか**を基準にすれば、どのシルエットに載せ替えても口は口の
/// 大きさに見える。組み合わせが 30×30 に増えるビルダーでは、これが
/// 「どの組み合わせでも破綻しない」を支える主要な仕掛けになっている。
///
/// # 表にする理由
/// 素朴に毎回輪郭を潰して交点を数えると、1 つのプレビューで数百回 `flatten`
/// することになる。輪郭 1 つにつき一度だけ表を作り、あとは引くだけにする。
///
/// # bar と dock の狭いほうを持つ
/// `[[details]]` の座標は**比率 1 つで両サイズに使われる**のに対し、
/// `capsule` と `corners_pt` の顔は bar と dock で輪郭の比率が変わる
/// （角丸が pt 固定 / 高さ依存なので、体の縦横比で丸みの割合が動く）。
/// 狭いほうに合わせないと、片方のサイズだけ `detail-outside-body` に落ちる。
#[derive(Debug, Clone)]
pub struct Profile {
    half: Vec<f64>,
}

impl Profile {
    pub fn of(spec: &FaceSpec) -> Profile {
        let bar = section(spec, Size::Bar);
        let dock = section(spec, Size::Dock);
        Profile {
            half: bar
                .iter()
                .zip(dock.iter())
                .map(|(a, b)| a.min(*b))
                .collect(),
        }
    }

    /// 高さ `v` で使える半幅（体幅に対する比率）。
    ///
    /// 標本のあいだは**両隣の小さいほう**を返す。補間して大きい値を返すと、
    /// 凹んだ輪郭（もみあげの切れ込み）で実際より広く見積もってしまう。
    pub fn half_at(&self, v: f64) -> f64 {
        let x = clamp01(v) * PROFILE_N as f64;
        let i = (x.floor() as usize).min(PROFILE_N);
        let j = (x.ceil() as usize).min(PROFILE_N);
        self.half[i].min(self.half[j])
    }

    /// パネル線を置ける高さの帯（下端, 上端）。
    ///
    /// あごが持ち上がっている顔（`Sil::burn`）では下端が 0 ではない。
    /// ここを見ずに口を置くと、あごの下・もみあげのあいだの**何も無い所**に
    /// 線を引いてしまう（`detail-outside-body` で落ちる）。
    pub fn band(&self) -> (f64, f64) {
        let lo = self.half.iter().position(|&h| h > MIN_ROOM).unwrap_or(0);
        let hi = self
            .half
            .iter()
            .rposition(|&h| h > MIN_ROOM)
            .unwrap_or(PROFILE_N);
        let step = 1.0 / PROFILE_N as f64;
        // 標本ちょうどに置くと丸め次第で縁に触れるので、1 刻みぶん内側に寄せる。
        let (a, b) = ((lo as f64 + 1.0) * step, (hi as f64 - 1.0) * step);
        if a >= b {
            let mid = (a + b) / 2.0;
            (mid, mid)
        } else {
            (a, b)
        }
    }
}

/// 1 サイズぶんの断面表。
fn section(spec: &FaceSpec, size: Size) -> Vec<f64> {
    let (w, h) = spec.body_size(size);
    if w <= 0.0 || h <= 0.0 {
        return vec![0.0; PROFILE_N + 1];
    }
    let poly = flatten(&spec.body_outline(size), FLATTEN_STEPS);
    let c = w / 2.0;

    (0..=PROFILE_N)
        .map(|i| {
            let y = (i as f64 / PROFILE_N as f64) * h;

            // 水平線 y と輪郭の交点（偶奇規則と同じ走査）。
            let mut xs: Vec<f64> = Vec::new();
            for k in 0..poly.len() {
                let (x1, y1) = poly[k];
                let (x2, y2) = poly[(k + 1) % poly.len()];
                if (y1 > y) != (y2 > y) {
                    xs.push(x1 + (x2 - x1) * (y - y1) / (y2 - y1));
                }
            }
            xs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

            // **左端と右端ではなく、中心を含む区間を採る。**
            //
            // もみあげのある顔はこの高さで顔が 3 つに割れる:
            //   [左のもみあげ] … [あご] … [右のもみあげ]
            // 端どうしの距離を取ると「もみあげの外側まで顔がある」ことになり、
            // あごの外の隙間に口を置いてしまう。偶奇規則では内部区間は交点を
            // 2 つずつ組にしたものなので、中心が入る組を探す。
            for pair in xs.chunks(2) {
                let [lo, hi] = pair else { continue };
                if *lo <= c && c <= *hi {
                    return ((c - lo) / w).min((hi - c) / w).max(0.0);
                }
            }
            0.0
        })
        .collect()
}

/// 正規化座標 `(t, dv)`（`t` は -1..1 の左右位置、`dv` は基準 v からの差）を、
/// 体の矩形に対する 0..1 比率へ落とす。
///
/// - 幅は**その点の高さでの顔の半幅**に比例させる（`Profile` の doc 参照）
/// - 部品が顔の帯からはみ出すときは、**形を保ったまま帯の中へずらす**。
///   潰すのではなくずらすのは、あごの高い顔（もみあげ）で口が線に潰れるより、
///   少し上に載っているほうが顔として読めるため
///
/// `off` は**中心からの横位置**で、単位は `w` と同じ（その高さの半幅に対する比率）。
/// 0 なら中央、正なら右へ寄る。耳や頬のパネルのように顔の端に置く部品はこれを使う。
/// **`off` も半幅に比例する**ので、細い顔に載せた側面パーツは自動で内側に寄り、
/// 輪郭からはみ出さない（`detail-outside-body` に落ちない）。
pub fn place(
    profile: &Profile,
    base_v: f64,
    off: f64,
    w: f64,
    pts: &[(f64, f64)],
) -> Vec<[f64; 2]> {
    let (lo, hi) = profile.band();
    let dmin = pts.iter().fold(f64::INFINITY, |m, p| m.min(p.1));
    let dmax = pts.iter().fold(f64::NEG_INFINITY, |m, p| m.max(p.1));

    let mut base = base_v;
    if base + dmin < lo {
        base = lo - dmin;
    }
    if base + dmax > hi {
        base = hi - dmax;
    }

    pts.iter()
        .map(|&(t, dv)| {
            // 部品が帯より背が高いときだけ、ここで潰れる。
            let v = (base + dv).clamp(lo, hi);
            let half = profile.half_at(v) * DETAIL_INSET;
            [clamp01(0.5 + (off + t * w) * half), clamp01(v)]
        })
        .collect()
}

/// 横位置の微調整。**その高さの顔の半幅の範囲でだけ動かす**ので、
/// 寄せきっても輪郭から出ない。
pub fn shift_x(profile: &Profile, points: &mut [[f64; 2]], dx: f64) {
    for p in points.iter_mut() {
        let room = profile.half_at(p[1]) * DETAIL_INSET;
        p[0] = (p[0] + dx * room * 2.0).clamp(0.5 - room, 0.5 + room);
    }
}

// ---------------------------------------------------------------------------
// 折れ線の素形
// ---------------------------------------------------------------------------

/// `parts.rs` の表が選ぶ折れ線の型。**形の語彙はここが全部**で、
/// パーツを足すときはこの型の組み合わせと数値を 1 行書くだけで済む。
///
/// 後半 4 つは**人間でないもの**（装甲板・通気口・継ぎ目）のための素形。
/// 生き物の顔は曲線で、機械の顔は**直角と閉じた板**でできているので、
/// `Arc` や `Wave` をいくら足しても兜のような顔にはならない。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Curve {
    /// 描かない（「なし」のパーツ）。
    None,
    /// ゆるい弧。`amp` が正で上へ反る（笑顔・眉）、負で下へ反る（への字）。
    Arc,
    /// 波。`0` の山数で前髪のギザギザや波打つ口になる。
    Wave(u8),
    /// 斜めに流す（分け目）。`1` で右上がり、`-1` で左上がり。
    Sweep(i8),
    /// への字／レの字。3 点だけの折れ。
    Vee,
    /// かぎ形（鼻筋 → 小鼻）。`1` で左向き、`-1` で右向き。
    Hook(i8),
    /// 閉じた輪（口を開けた形・額の意匠）。
    Ring,
    /// ごく短い線（点に見える）。
    Dot,

    /// 閉じた台形の板。`taper` は下辺 / 上辺の幅比（1 で長方形、0 で三角）。
    /// `Ring` と違って**角が立つ**ので、同じ「閉じた形」でも機械の板に見える。
    Plate { taper: f64 },
    /// 門型。3 辺だけの枠で、`down` が真なら開口が下（∏）、偽なら上（∐）。
    Bracket { down: bool },
    /// 縦棒。**横に寝ていない唯一の素形**で、鼻筋や装甲の継ぎ目になる。
    /// `w` は効かない（常に中心に立つ）。
    Stroke,
    /// 角波。`0` の谷数で通気口・スピーカーグリルに見える。
    /// `Wave` が正弦なのに対しこちらは直角に折るので、口に置くと歯車じみる。
    Teeth(u8),
}

/// 素形を正規化座標 `(t, dv)` の列にする。`amp` は体高に対する比率。
///
/// 返す点は必ず 2 点以上（`Curve::None` を除く）。`parse.rs` が
/// 「`points` は 2 点以上」を要求するため。
pub fn curve_points(curve: Curve, amp: f64) -> Vec<(f64, f64)> {
    match curve {
        Curve::None => Vec::new(),

        // 放物線 1 本。7 点あれば bar の実寸でも角が見えない。
        Curve::Arc => (0..7)
            .map(|i| {
                let t = -1.0 + 2.0 * i as f64 / 6.0;
                (t, amp * (1.0 - t * t))
            })
            .collect(),

        // 山と谷を交互に。両端は谷（顔の縁に向かって落とす）。
        Curve::Wave(lobes) => {
            let lobes = lobes.max(1) as usize;
            let n = lobes * 2 + 1;
            (0..n)
                .map(|i| {
                    let t = -1.0 + 2.0 * i as f64 / (n - 1) as f64;
                    // 端ほど振れを小さくして、縁に刺さらないようにする。
                    let envelope = 1.0 - 0.45 * t * t;
                    let up = if i % 2 == 1 { 1.0 } else { 0.0 };
                    (t, amp * up * envelope)
                })
                .collect()
        }

        // 直線気味に流すが、わずかに反らせて「梳かした」感じを出す。
        Curve::Sweep(dir) => {
            let d = if dir < 0 { -1.0 } else { 1.0 };
            (0..5)
                .map(|i| {
                    let t = -1.0 + 2.0 * i as f64 / 4.0;
                    (t, d * amp * (t * 0.9 + 0.25 * (1.0 - t * t)))
                })
                .collect()
        }

        Curve::Vee => vec![(-1.0, 0.0), (0.0, -amp), (1.0, 0.0)],

        // 鼻筋を下り、小鼻へ折れる。`dir` で流す向きを変える。
        Curve::Hook(dir) => {
            let d = if dir < 0 { -1.0 } else { 1.0 };
            vec![(d * 0.35, amp), (d * 0.35, 0.0), (-d * 1.0, amp * 0.18)]
        }

        // 楕円を 9 点で。最後に始点へ戻して閉じた形に見せる。
        Curve::Ring => {
            let n = 9;
            let mut v: Vec<(f64, f64)> = (0..n)
                .map(|i| {
                    let a = std::f64::consts::TAU * i as f64 / n as f64;
                    (a.cos(), amp * a.sin())
                })
                .collect();
            v.push(v[0]);
            v
        }

        Curve::Dot => vec![(-1.0, 0.0), (1.0, 0.0)],

        // 上辺 → 下辺 → 始点に戻す。`Ring` と同じ「閉じて見せる」手だが、
        // 頂点を丸めないので板に見える。
        Curve::Plate { taper } => {
            let k = taper.clamp(0.0, 1.0);
            vec![(-1.0, amp), (1.0, amp), (k, -amp), (-k, -amp), (-1.0, amp)]
        }

        // 縦 → 横 → 縦。閉じないので枠に見える。
        Curve::Bracket { down } => {
            let d = if down { 1.0 } else { -1.0 };
            vec![
                (-1.0, -d * amp),
                (-1.0, d * amp),
                (1.0, d * amp),
                (1.0, -d * amp),
            ]
        }

        Curve::Stroke => vec![(0.0, amp), (0.0, -amp)],

        // 谷 → 山 → 山 → 谷 を直角に繰り返す。
        Curve::Teeth(n) => {
            let n = n.max(1) as usize;
            let mut v = Vec::with_capacity(n * 4);
            for i in 0..n {
                let a = -1.0 + 2.0 * i as f64 / n as f64;
                let b = -1.0 + 2.0 * (i + 1) as f64 / n as f64;
                v.extend([(a, 0.0), (a, amp), (b, amp), (b, 0.0)]);
            }
            v
        }
    }
}

// ---------------------------------------------------------------------------
// 目の多角形
// ---------------------------------------------------------------------------

/// 目の閉多角形を「上まぶた + 下まぶた」の 2 本の弧から作る。
///
/// 座標は**目の矩形 w×h に対する 0..1**（`EyesSpec::polygon` の約束）で、
/// 右目を書けば左目は `u → 1-u` の鏡像になる。つまり `u = 0` が鼻側、
/// `u = 1` がこめかみ側。
///
/// # 引数
/// - `inner` … 鼻側の目頭の高さ（0..1）
/// - `outer` … こめかみ側の目尻の高さ。`outer > inner` で**吊り目**、逆で**たれ目**
/// - `upper` … 上まぶたのふくらみ。大きいほど丸い目、小さいほど細い目
/// - `lower` … 下まぶたのふくらみ。`upper` と変えるとジト目・半月目になる
///
/// 最後に v を 0..1 いっぱいに正規化するので、`eyes.size` の pt がどの
/// バリエーションでも同じ意味（外接矩形）になる。
pub fn eye_polygon(inner: f64, outer: f64, upper: f64, lower: f64) -> Vec<[f64; 2]> {
    const STEPS: usize = 7;
    let a = (0.0, inner);
    let b = (1.0, outer);
    let top = inner.max(outer) + upper;
    let bot = inner.min(outer) - lower;

    let mut pts: Vec<(f64, f64)> = Vec::with_capacity(STEPS * 2);
    // 上まぶた: 目頭 → 目尻
    for i in 0..STEPS {
        pts.push(quad(a, (0.5, top), b, i as f64 / (STEPS - 1) as f64));
    }
    // 下まぶた: 目尻 → 目頭（最初と最後は上まぶたと重なるので落とす）
    for i in 1..STEPS - 1 {
        pts.push(quad(b, (0.5, bot), a, i as f64 / (STEPS - 1) as f64));
    }

    normalize_v(pts)
}

/// **くさび形**の目。鼻側とこめかみ側の**縦辺 2 本**だけで決まる四角形。
///
/// `eye_polygon` が「まぶた 2 本の弧」＝生き物の目なのに対し、こちらは
/// 角が 4 つとも立っている。兜のスリット
/// （`[[0.00,0.00],[0.00,0.62],[1.00,1.00],[0.94,0.38]]` のような形）がこれで、
/// **弧をいくら細くしても出せない**（`slit` 系は両端が尖る紡錘形になる）。
///
/// 引数はそれぞれの縦辺の下端・上端（0..1）。`outer_hi > inner_hi` で吊り、
/// 逆でたれ。`lo` と `hi` を近づけるとその側が尖る。
pub fn wedge_polygon(inner_lo: f64, inner_hi: f64, outer_lo: f64, outer_hi: f64) -> Vec<[f64; 2]> {
    normalize_v(vec![
        (0.0, inner_lo),
        (0.0, inner_hi),
        (1.0, outer_hi),
        (1.0, outer_lo),
    ])
}

/// 丸い目の、**こめかみ側の上（`up`）または下だけを角に引き出した**形。
///
/// **両端を尖らせてはいけない** — 紡錘形にすると目が「木の葉」になり、
/// 2 つ並べた瞬間に虫の翅か宇宙人の顔になる。周の 8 割を楕円のまま残し、
/// 尖らせるのは 1 点だけにする。`EyeForm::Lids` は目頭・目尻の両方が
/// 頂点になるので、この形は構造的に作れない。
pub fn horn_polygon(up: bool) -> Vec<[f64; 2]> {
    // 楕円から切り欠く角度の範囲。両脇の点から角へ 2 本の直線が伸びる。
    const NOTCH_LO: f64 = std::f64::consts::TAU * 20.0 / 360.0;
    const NOTCH_HI: f64 = std::f64::consts::TAU * 70.0 / 360.0;
    const STEPS: usize = 14;

    // **角が本体からどれだけ突き出るか**（本体の半径に対する倍率）。
    //
    // ここが小さいと「丸に小さな出っ張り」にしかならず、実寸 4pt の目では
    // ただの `bead` と見分けがつかない＝この形を足した意味が消える。
    // 手描きの角つき丸目の実測はおよそ 1.4 倍（角が半径 0.707 / 本体が 0.5）
    // なので、正規化後にそれくらいになる 1.5 を採る。
    const REACH: f64 = 1.5;

    let diag = std::f64::consts::FRAC_1_SQRT_2 * 0.5 * REACH;
    let mut pts = vec![(0.5 + diag, 0.5 + diag)];
    let sweep = std::f64::consts::TAU - (NOTCH_HI - NOTCH_LO);
    for i in 0..STEPS {
        let a = NOTCH_HI + sweep * i as f64 / (STEPS - 1) as f64;
        pts.push((0.5 + 0.5 * a.cos(), 0.5 + 0.5 * a.sin()));
    }
    if !up {
        for p in &mut pts {
            p.1 = 1.0 - p.1;
        }
    }
    // 角が箱の外へ出ているので、**縦横とも**引き伸ばして箱いっぱいに収める
    // （`eye_polygon` は u が最初から 0..1 なので v だけでよい）。
    normalize_uv(pts)
}

/// v だけ 0..1 へ引き伸ばす（u は既に 0..1 に収まっている前提）。
fn normalize_v(pts: Vec<(f64, f64)>) -> Vec<[f64; 2]> {
    let (lo, hi) = span(pts.iter().map(|p| p.1));
    pts.into_iter()
        .map(|(u, v)| [clamp01(u), clamp01((v - lo) / hi)])
        .collect()
}

/// u も v も 0..1 へ引き伸ばす。
fn normalize_uv(pts: Vec<(f64, f64)>) -> Vec<[f64; 2]> {
    let (ulo, uspan) = span(pts.iter().map(|p| p.0));
    let (vlo, vspan) = span(pts.iter().map(|p| p.1));
    pts.into_iter()
        .map(|(u, v)| [clamp01((u - ulo) / uspan), clamp01((v - vlo) / vspan)])
        .collect()
}

/// 最小値と、0 割りを避けた幅。
fn span(vals: impl Iterator<Item = f64> + Clone) -> (f64, f64) {
    let lo = vals.clone().fold(f64::INFINITY, f64::min);
    let hi = vals.fold(f64::NEG_INFINITY, f64::max);
    (lo, (hi - lo).max(1e-6))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::face::parse::parse;
    use crate::face::spec::Source;
    use crate::face::Registry;

    fn probe() -> FaceSpec {
        parse(
            r#"
id = "probe"
label = "検査用"
[size]
bar  = { w = 22, h = 20 }
dock = { w = 36, h = 34 }
[outline]
kind = "corners"
corners = [[0.50,0.58],[0.50,0.58],[0.48,0.42],[0.48,0.42]]
[eyes]
shape = "rounded"
gap  = { bar = 3.0, dock = 5.0 }
size = { bar = [3.0, 4.0], dock = [4.0, 6.0] }
"#,
            Source::Builtin,
        )
        .unwrap()
    }

    /// 生成した座標は必ず 0..1 の箱に入る（輪郭の検査が制御点まで見るため）。
    #[test]
    fn smooth_path_keeps_every_control_point_inside_the_box() {
        // わざと箱の縁を舐める経由点を与える。
        let pts = vec![
            (0.5, 0.0),
            (1.0, 0.05),
            (1.0, 0.5),
            (0.95, 0.98),
            (0.5, 1.0),
        ];
        for seg in smooth_path(&pts) {
            let all = match seg {
                Seg::Line { to } => vec![to],
                Seg::Cubic { c1, c2, to } => vec![c1, c2, to],
            };
            for (x, y) in all {
                assert!((0.0..=1.0).contains(&x), "x が箱の外: {x}");
                assert!((0.0..=1.0).contains(&y), "y が箱の外: {y}");
            }
        }
    }

    /// 端点の接線が鏡像を向く（＝あご先と頭頂で角が立たない）。
    ///
    /// 頭頂 `(0.5, 1.0)` に入る最後の制御点 `c2` が中央より**内側**にあると、
    /// 鏡像とつないだときに尖る。水平に入ってくることを見る。
    #[test]
    fn the_seam_tangents_are_horizontal() {
        let pts = vec![(0.5, 0.0), (0.9, 0.3), (0.85, 0.8), (0.5, 1.0)];
        let segs = smooth_path(&pts);
        match *segs.last().unwrap() {
            Seg::Cubic { c2, to, .. } => {
                assert!((to.0 - 0.5).abs() < 1e-9, "頭頂が中央にない");
                assert!(
                    (c2.1 - to.1).abs() < 1e-9,
                    "頭頂へ水平に入っていない（c2 = {c2:?}）"
                );
            }
            other => panic!("最後は Cubic のはず: {other:?}"),
        }
    }

    /// 半幅は顔の腹（v = 0.5）で最大に近く、上下端で 0 に近づく。
    #[test]
    fn half_width_follows_the_silhouette() {
        let p = Profile::of(&probe());
        let mid = p.half_at(0.5);
        assert!(mid > 0.45, "腹が細すぎる: {mid}");
        assert!(p.half_at(0.02) < mid, "下端が絞られていない");
        assert!(p.half_at(0.98) < mid, "上端が絞られていない");
        let (lo, hi) = p.band();
        assert!(lo > 0.0 && hi < 1.0 && lo < hi, "帯が変: {lo}..{hi}");
    }

    /// **組込み顔のどれに載せても**、生成した折れ線は輪郭の内側に入る。
    #[test]
    fn placed_points_stay_inside_every_builtin_outline() {
        for face in Registry::builtin().all() {
            let profile = Profile::of(face);
            for v in [0.28, 0.42, 0.56, 0.74] {
                let pts = place(&profile, v, 0.0, 1.0, &curve_points(Curve::Arc, 0.05));
                for size in [Size::Bar, Size::Dock] {
                    let (w, h) = face.body_size(size);
                    let poly = flatten(&face.body_outline(size), FLATTEN_STEPS);
                    for [u, vv] in &pts {
                        assert!(
                            crate::face::contains(&poly, (u * w, vv * h)),
                            "{} の v={v} の線が {:?} ではみ出す（{u}, {vv}）",
                            face.id,
                            size
                        );
                    }
                }
            }
        }
    }

    /// 目の多角形は外接矩形いっぱいに正規化され、3 点以上ある。
    #[test]
    fn eye_polygons_fill_their_bounding_box() {
        for (inner, outer) in [(0.5, 0.5), (0.35, 0.72), (0.72, 0.35)] {
            let p = eye_polygon(inner, outer, 0.45, 0.45);
            assert!(p.len() >= 3, "点が足りない");
            let lo = p.iter().fold(f64::INFINITY, |m, q| m.min(q[1]));
            let hi = p.iter().fold(f64::NEG_INFINITY, |m, q| m.max(q[1]));
            assert!(
                (lo - 0.0).abs() < 1e-9 && (hi - 1.0).abs() < 1e-9,
                "正規化されていない"
            );
            for q in &p {
                assert!((0.0..=1.0).contains(&q[0]) && (0.0..=1.0).contains(&q[1]));
            }
        }
    }

    /// 吊り目とたれ目は実際に別の形になる。
    #[test]
    fn tilt_actually_changes_the_shape() {
        let up = eye_polygon(0.35, 0.72, 0.4, 0.4);
        let down = eye_polygon(0.72, 0.35, 0.4, 0.4);
        assert_ne!(up, down);
    }

    /// くさび目は **4 点の四角形**（弧で近似されない）。
    #[test]
    fn a_wedge_eye_is_a_quadrilateral() {
        let p = wedge_polygon(0.00, 0.62, 0.38, 1.00);
        assert_eq!(p.len(), 4, "くさびが 4 点でない: {p:?}");
        // 鼻側が低く、こめかみ側が高い＝兜のスリット。
        assert!(p[1][1] < p[2][1], "こめかみ側が持ち上がっていない");
    }

    /// 角つき丸目は **尖りが 1 点だけ**（「両端を尖らせない」を守る）。
    ///
    /// こめかみ側の上に (1, 1) の角があり、鼻側（u が小さい側）には
    /// 箱の角に届く点が無いことを見る。
    #[test]
    fn a_horned_eye_has_exactly_one_corner() {
        let p = horn_polygon(true);
        let corner = |q: &[f64; 2]| (q[0] - 1.0).abs() < 1e-9 && (q[1] - 1.0).abs() < 1e-9;
        assert_eq!(p.iter().filter(|q| corner(q)).count(), 1, "角が 1 つでない");
        // 鼻側の半分には、上端にも下端にも張り付く点が無い＝丸いまま。
        for q in p.iter().filter(|q| q[0] < 0.4) {
            assert!(
                q[1] > 0.02 && q[1] < 0.98,
                "鼻側が尖っている（{q:?}）— 木の葉になる"
            );
        }
        // 上下反転すると別の形（たれ側に角が来る）。
        assert_ne!(p, horn_polygon(false));
    }

    /// **角が本体からはっきり突き出ている。**
    ///
    /// これが効いていないと、実寸 4pt の目では `bead`（ただの丸）と見分けが
    /// つかず、この形を足した意味が無くなる（実際に一度そうなった）。
    /// 本体の外接円に対して角がどれだけ外にあるかを見る。
    #[test]
    fn the_horn_actually_sticks_out() {
        let p = horn_polygon(true);
        let c = (0.5, 0.5);
        let d = |q: &[f64; 2]| ((q[0] - c.0).powi(2) + (q[1] - c.1).powi(2)).sqrt();
        let corner = d(&[1.0, 1.0]);
        // 角以外（＝楕円の本体）でいちばん遠い点。
        let body = p
            .iter()
            .filter(|q| !(q[0] > 0.99 && q[1] > 0.99))
            .fold(0.0_f64, |m, q| m.max(d(q)));
        assert!(
            corner / body > 1.3,
            "角の張り出しが足りない（角 {corner:.3} / 本体 {body:.3}）— 丸目と区別がつかない"
        );
    }

    /// 素形はどれも 2 点以上返す（`parse` の要求）。
    #[test]
    fn every_curve_yields_at_least_two_points() {
        for c in [
            Curve::Arc,
            Curve::Wave(2),
            Curve::Wave(5),
            Curve::Sweep(1),
            Curve::Sweep(-1),
            Curve::Vee,
            Curve::Hook(1),
            Curve::Hook(-1),
            Curve::Ring,
            Curve::Dot,
            Curve::Plate { taper: 0.6 },
            Curve::Plate { taper: 1.0 },
            Curve::Bracket { down: true },
            Curve::Bracket { down: false },
            Curve::Stroke,
            Curve::Teeth(1),
            Curve::Teeth(5),
        ] {
            assert!(curve_points(c, 0.05).len() >= 2, "{c:?} が短すぎる");
        }
        assert!(curve_points(Curve::None, 0.05).is_empty());
    }
}
