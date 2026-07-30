//! `faces/*.toml` → `FaceSpec` のパース。
//!
//! スキーマの説明は `faces/README.md`。ここは**形の検査だけ**を行い、
//! 「輪郭からはみ出す」「メニューバーに入らない」といった意味の検査は
//! `validate.rs` の担当。
//!
//! # 失敗の返し方
//! `Vec<Problem>` を返す。**最初の 1 件で止めない**のは、投稿者が 1 回の実行で
//! 全部直せるようにするため。TOML の構文エラーだけは serde が 1 件しか返さない
//! ので、そこは 1 件になる。

use serde::Deserialize;

use crate::face::spec::{
    BodySize, BySize, CornerSpec, DetailSpec, EyeColor, EyeOverride, EyeShape, EyesSpec, FaceSpec,
    OutlineSpec, Problem, ProblemCode, Source,
};
use crate::face::{Seg, Size};
use crate::session::SessionState;

/// TOML テキストを `FaceSpec` にする。
pub fn parse(text: &str, source: Source) -> Result<FaceSpec, Vec<Problem>> {
    let raw: RawFace = toml::from_str(text).map_err(|e| {
        vec![Problem::new(
            ProblemCode::Parse,
            format!("Failed to parse TOML: {e}"),
        )]
    })?;
    build(raw, source)
}

// ---------------------------------------------------------------------------
// Raw（TOML の素の形）
// ---------------------------------------------------------------------------

// `deny_unknown_fields` を付けているのは、投稿者の綴り間違い（`corner` /
// `detials` 等）を黙って無視せずエラーにするため。データ駆動では
// 「書いたのに効かない」がいちばん分かりにくい失敗になる。

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFace {
    id: String,
    label: String,
    label_en: Option<String>,
    author: Option<String>,
    size: RawSizes,
    outline: RawOutline,
    eyes: RawEyes,
    #[serde(default)]
    details: Vec<RawDetail>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSizes {
    bar: RawWh,
    dock: RawWh,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawWh {
    w: f64,
    h: f64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawBarDock {
    bar: f64,
    dock: f64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawOutline {
    kind: String,
    corners: Option<[[f64; 2]; 4]>,
    corners_pt: Option<RawBarDock>,
    #[serde(default)]
    half: bool,
    d: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawEyes {
    shape: String,
    v: Option<f64>,
    gap: RawBarDock,
    size: RawEyeSizes,
    radius: Option<f64>,
    polygon: Option<Vec<[f64; 2]>>,
    #[serde(default)]
    states: std::collections::BTreeMap<String, RawEyeState>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawEyeSizes {
    bar: [f64; 2],
    dock: [f64; 2],
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawEyeState {
    w_scale: Option<f64>,
    h_scale: Option<f64>,
    dx: Option<f64>,
    color: Option<String>,
    glow: Option<f64>,
    blink: Option<bool>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawDetail {
    name: String,
    sizes: Option<Vec<String>>,
    points: Vec<[f64; 2]>,
}

// ---------------------------------------------------------------------------
// Raw → FaceSpec
// ---------------------------------------------------------------------------

fn build(raw: RawFace, source: Source) -> Result<FaceSpec, Vec<Problem>> {
    let mut problems = Vec::new();

    if !is_valid_id(&raw.id) {
        problems.push(Problem::new(
            ProblemCode::Id,
            format!(
                "id {:?} is invalid. Use only lowercase letters, digits, and hyphens, \
                 start with a letter or digit, and keep it to 32 characters or fewer \
                 (e.g. \"my-face\")",
                raw.id
            ),
        ));
    }
    if raw.label.trim().is_empty() {
        problems.push(Problem::new(ProblemCode::Parse, "label is empty"));
    }

    let size = BySize {
        bar: BodySize {
            w: raw.size.bar.w,
            h: raw.size.bar.h,
        },
        dock: BodySize {
            w: raw.size.dock.w,
            h: raw.size.dock.h,
        },
    };

    let outline = build_outline(&raw.outline, &mut problems);
    let eyes = build_eyes(&raw.eyes, &mut problems);
    let details = build_details(&raw.details, &mut problems);

    match (outline, eyes) {
        (Some(outline), Some(eyes)) if problems.is_empty() => Ok(FaceSpec {
            id: raw.id,
            label: raw.label,
            label_en: raw.label_en,
            author: raw.author,
            size,
            outline,
            eyes,
            details,
            source,
        }),
        _ => {
            if problems.is_empty() {
                problems.push(Problem::new(ProblemCode::Parse, "Could not build the face"));
            }
            Err(problems)
        }
    }
}

/// `id` は `[a-z0-9][a-z0-9-]*` の 32 文字以内。
fn is_valid_id(id: &str) -> bool {
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

fn build_outline(raw: &RawOutline, problems: &mut Vec<Problem>) -> Option<OutlineSpec> {
    match raw.kind.as_str() {
        "corners" => {
            if raw.d.is_some() {
                problems.push(Problem::new(
                    ProblemCode::Parse,
                    "d cannot be used with kind = \"corners\" (use kind = \"path\" instead)",
                ));
            }
            match (raw.corners, &raw.corners_pt) {
                (Some(c), None) => Some(OutlineSpec::Corners(CornerSpec::Ratio(c))),
                (None, Some(pt)) => Some(OutlineSpec::Corners(CornerSpec::Pt(BySize {
                    bar: pt.bar,
                    dock: pt.dock,
                }))),
                (Some(_), Some(_)) => {
                    problems.push(Problem::new(
                        ProblemCode::Parse,
                        "corners and corners_pt cannot both be set. Use only one of them",
                    ));
                    None
                }
                (None, None) => {
                    problems.push(Problem::new(
                        ProblemCode::Parse,
                        "kind = \"corners\" requires either corners or corners_pt",
                    ));
                    None
                }
            }
        }
        "capsule" => {
            if raw.corners.is_some() || raw.corners_pt.is_some() || raw.d.is_some() {
                problems.push(Problem::new(
                    ProblemCode::Parse,
                    "corners / corners_pt / d cannot be used with kind = \"capsule\" \
                     (the corner radius is fixed at half the height)",
                ));
            }
            Some(OutlineSpec::Corners(CornerSpec::Capsule))
        }
        "path" => {
            if raw.corners.is_some() || raw.corners_pt.is_some() {
                problems.push(Problem::new(
                    ProblemCode::Parse,
                    "corners / corners_pt cannot be used with kind = \"path\"",
                ));
            }
            let Some(d) = raw.d.as_deref() else {
                problems.push(Problem::new(
                    ProblemCode::Parse,
                    "kind = \"path\" requires d",
                ));
                return None;
            };
            match parse_path(d) {
                Ok((start, segs)) => Some(OutlineSpec::Path {
                    half: raw.half,
                    start,
                    segs,
                }),
                Err(p) => {
                    problems.push(p);
                    None
                }
            }
        }
        other => {
            problems.push(Problem::new(
                ProblemCode::Parse,
                format!("outline.kind is {other:?} (must be \"corners\" | \"capsule\" | \"path\")"),
            ));
            None
        }
    }
}

fn build_eyes(raw: &RawEyes, problems: &mut Vec<Problem>) -> Option<EyesSpec> {
    let shape = match raw.shape.as_str() {
        "rounded" => EyeShape::Rounded,
        "polygon" => EyeShape::Polygon,
        other => {
            problems.push(Problem::new(
                ProblemCode::Parse,
                format!("eyes.shape is {other:?} (must be \"rounded\" or \"polygon\")"),
            ));
            return None;
        }
    };

    match (shape, &raw.polygon) {
        (EyeShape::Polygon, None) => {
            problems.push(Problem::new(
                ProblemCode::Parse,
                "eyes.shape = \"polygon\" requires polygon",
            ));
            return None;
        }
        (EyeShape::Polygon, Some(p)) if p.len() < 3 => {
            problems.push(Problem::new(
                ProblemCode::Parse,
                format!(
                    "eyes.polygon requires at least 3 points (only {} given)",
                    p.len()
                ),
            ));
        }
        (EyeShape::Rounded, Some(_)) => {
            problems.push(Problem::new(
                ProblemCode::Parse,
                "polygon cannot be used with eyes.shape = \"rounded\"",
            ));
        }
        _ => {}
    }

    let mut states = [None; 6];
    for (key, st) in &raw.states {
        let Some(state) = SessionState::from_str(key) else {
            problems.push(Problem::new(
                ProblemCode::Parse,
                format!(
                    "eyes.states.{key} is an unknown state (must be working / wait_user / \
                     wait_agent / idle / done / error)"
                ),
            ));
            continue;
        };
        let color = match st.color.as_deref() {
            None => EyeColor::Eye,
            Some(c) => match EyeColor::parse(c) {
                Some(c) => c,
                None => {
                    problems.push(Problem::new(
                        ProblemCode::Parse,
                        format!(
                            "eyes.states.{key}.color is {c:?} \
                             (must be \"eye\" | \"eye_closed\" | \"eye_error\" | \"white\")"
                        ),
                    ));
                    EyeColor::Eye
                }
            },
        };
        let d = EyeOverride::default();
        states[crate::face::spec::state_index(state)] = Some(EyeOverride {
            w_scale: st.w_scale.unwrap_or(d.w_scale),
            h_scale: st.h_scale.unwrap_or(d.h_scale),
            dx: st.dx.unwrap_or(d.dx),
            color,
            glow: st.glow.unwrap_or(d.glow),
            blink: st.blink.unwrap_or(d.blink),
        });
    }

    Some(EyesSpec {
        shape,
        v: raw.v.unwrap_or(0.5),
        gap: BySize {
            bar: raw.gap.bar,
            dock: raw.gap.dock,
        },
        size: BySize {
            bar: raw.size.bar,
            dock: raw.size.dock,
        },
        radius: raw.radius.unwrap_or(2.0),
        polygon: raw.polygon.clone(),
        states,
    })
}

fn build_details(raw: &[RawDetail], problems: &mut Vec<Problem>) -> Vec<DetailSpec> {
    let mut out = Vec::with_capacity(raw.len());
    let mut seen: Vec<&str> = Vec::new();
    for d in raw {
        if seen.contains(&d.name.as_str()) {
            problems.push(Problem::new(
                ProblemCode::Parse,
                format!("duplicate details name {:?}", d.name),
            ));
        }
        seen.push(&d.name);

        if d.points.len() < 2 {
            problems.push(Problem::new(
                ProblemCode::Parse,
                format!(
                    "details {:?} points requires at least 2 points (only {} given)",
                    d.name,
                    d.points.len()
                ),
            ));
        }

        let sizes = match &d.sizes {
            None => vec![Size::Bar, Size::Dock],
            Some(list) => {
                let mut v = Vec::with_capacity(list.len());
                for s in list {
                    match s.as_str() {
                        "bar" => v.push(Size::Bar),
                        "dock" => v.push(Size::Dock),
                        other => problems.push(Problem::new(
                            ProblemCode::Parse,
                            format!(
                                "details {:?} sizes contains {other:?} (must be \"bar\" or \"dock\")",
                                d.name
                            ),
                        )),
                    }
                }
                v
            }
        };

        out.push(DetailSpec {
            name: d.name.clone(),
            sizes,
            points: d.points.clone(),
        });
    }
    out
}

// ---------------------------------------------------------------------------
// `d` 属性のパーサ（SVG のサブセット）
// ---------------------------------------------------------------------------

/// SVG の `d` 属性のうち **`M` / `L` / `C` / `Z` の絶対座標だけ**を読む。
///
/// ベクタツール（Illustrator / Figma など）から書き出した
/// パスを 0..1 に正規化して貼るだけで済むように、この 4 つに絞ってある。
/// 組込みの顔が実際に使っているのも `Line` と `Cubic` の 2 種だけで、表現力は足りている。
///
/// 相対座標（小文字）と `A` / `Q` / `S` / `T` / `H` / `V` は**エラー**にする。
/// 黙って無視すると「書いたのに形が違う」になり、原因が分からなくなるため。
fn parse_path(d: &str) -> Result<((f64, f64), Vec<Seg>), Problem> {
    let mut tokens = d
        .split(|c: char| c.is_whitespace() || c == ',')
        .filter(|t| !t.is_empty())
        .peekable();

    let mut start: Option<(f64, f64)> = None;
    let mut segs = Vec::new();

    while let Some(tok) = tokens.next() {
        // 数値が命令の前に来た＝命令文字の省略（`L 1 2 3 4`）。未対応。
        let cmd = match tok {
            "M" | "L" | "C" | "Z" => tok,
            "m" | "l" | "c" | "z" | "a" | "q" | "s" | "t" | "h" | "v" => {
                return Err(Problem::new(
                    ProblemCode::Parse,
                    format!(
                        "d has a relative-coordinate command {tok:?}. Use absolute \
                         coordinates (uppercase M / L / C / Z)"
                    ),
                ))
            }
            "A" | "Q" | "S" | "T" | "H" | "V" => {
                return Err(Problem::new(
                    ProblemCode::Parse,
                    format!(
                        "d command {tok:?} is not supported. Only M / L / C / Z can be used \
                         (convert arcs or quadratic Béziers to cubic Béziers in your vector tool)"
                    ),
                ))
            }
            other => {
                return Err(Problem::new(
                    ProblemCode::Parse,
                    format!(
                        "d has an unreadable token {other:?}. Omitting the command letter \
                         (M / L / C / Z) is not supported"
                    ),
                ))
            }
        };

        let mut num = |cmd: &str| -> Result<f64, Problem> {
            let t = tokens.next().ok_or_else(|| {
                Problem::new(
                    ProblemCode::Parse,
                    format!("d command {cmd} is missing numbers"),
                )
            })?;
            let v: f64 = t.parse().map_err(|_| {
                Problem::new(
                    ProblemCode::Parse,
                    format!("d value {t:?} cannot be read as a number"),
                )
            })?;
            if !v.is_finite() {
                return Err(Problem::new(
                    ProblemCode::Parse,
                    format!("d number {t:?} is not finite"),
                ));
            }
            Ok(v)
        };

        match cmd {
            "M" => {
                if start.is_some() {
                    return Err(Problem::new(
                        ProblemCode::Parse,
                        "d has M more than once. Write the outline as a single closed path",
                    ));
                }
                start = Some((num("M")?, num("M")?));
            }
            "L" => {
                if start.is_none() {
                    return Err(Problem::new(ProblemCode::Parse, "d must start with M"));
                }
                segs.push(Seg::Line {
                    to: (num("L")?, num("L")?),
                });
            }
            "C" => {
                if start.is_none() {
                    return Err(Problem::new(ProblemCode::Parse, "d must start with M"));
                }
                segs.push(Seg::Cubic {
                    c1: (num("C")?, num("C")?),
                    c2: (num("C")?, num("C")?),
                    to: (num("C")?, num("C")?),
                });
            }
            // `Z` は「閉じる」の意思表示。手は増やさない（閉じているかは `validate` が見る）。
            "Z" => {}
            _ => unreachable!("cmd は上の match で絞ってある"),
        }
    }

    let start =
        start.ok_or_else(|| Problem::new(ProblemCode::Parse, "d is empty (must start with M)"))?;
    if segs.is_empty() {
        return Err(Problem::new(ProblemCode::Parse, "d has no segments"));
    }
    Ok((start, segs))
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- d 属性 -------------------------------------------------------------

    #[test]
    fn parses_move_line_cubic_and_close() {
        let (start, segs) = parse_path("M 0.5 0 L 0.6 0 C 0.7 0.1 0.8 0.2 0.9 0.3 Z").unwrap();
        assert_eq!(start, (0.5, 0.0));
        assert_eq!(segs.len(), 2, "Z は手を増やさない");
        assert_eq!(segs[0], Seg::Line { to: (0.6, 0.0) });
        assert_eq!(
            segs[1],
            Seg::Cubic {
                c1: (0.7, 0.1),
                c2: (0.8, 0.2),
                to: (0.9, 0.3),
            }
        );
    }

    #[test]
    fn accepts_commas_and_newlines_as_separators() {
        let (_, segs) = parse_path("M 0,0\nL 1,0\nL 1,1").unwrap();
        assert_eq!(segs.len(), 2);
    }

    #[test]
    fn rejects_relative_commands() {
        let e = parse_path("M 0 0 l 1 0").unwrap_err();
        assert!(e.message.contains("relative-coordinate"), "{}", e.message);
    }

    #[test]
    fn rejects_unsupported_commands() {
        let e = parse_path("M 0 0 A 1 1 0 0 1 1 1").unwrap_err();
        assert!(e.message.contains("not supported"), "{}", e.message);
    }

    #[test]
    fn rejects_a_second_move() {
        let e = parse_path("M 0 0 L 1 0 M 2 2").unwrap_err();
        assert!(e.message.contains("more than once"), "{}", e.message);
    }

    #[test]
    fn rejects_missing_numbers() {
        let e = parse_path("M 0 0 C 1 1 2 2 3").unwrap_err();
        assert!(e.message.contains("missing numbers"), "{}", e.message);
    }

    #[test]
    fn rejects_implicit_command_repetition() {
        let e = parse_path("M 0 0 L 1 0 2 0").unwrap_err();
        assert!(e.message.contains("Omitting"), "{}", e.message);
    }

    #[test]
    fn rejects_non_finite_numbers() {
        let e = parse_path("M 0 0 L inf 0").unwrap_err();
        assert!(e.message.contains("not finite"), "{}", e.message);
    }

    #[test]
    fn rejects_an_empty_path() {
        assert!(parse_path("   ").is_err());
        assert!(parse_path("M 0 0").is_err(), "手が無い");
    }

    // ---- 全体 ---------------------------------------------------------------

    const MINIMAL: &str = r#"
id = "test"
label = "テスト"
[size]
bar  = { w = 22, h = 20 }
dock = { w = 36, h = 34 }
[outline]
kind = "corners"
corners = [[0.5,0.5],[0.5,0.5],[0.5,0.5],[0.5,0.5]]
[eyes]
shape = "rounded"
gap  = { bar = 3.0, dock = 5.0 }
size = { bar = [3.0, 4.0], dock = [4.0, 6.0] }
"#;

    #[test]
    fn parses_a_minimal_face() {
        let f = parse(MINIMAL, Source::Builtin).unwrap();
        assert_eq!(f.id, "test");
        assert_eq!(f.label, "テスト");
        assert_eq!(f.body_size(Size::Bar), (22.0, 20.0));
        assert_eq!(f.eyes.v, 0.5, "v の既定は中央");
        assert_eq!(f.eyes.radius, 2.0, "radius の既定");
        assert!(f.details.is_empty());
        assert!(
            f.eyes.states.iter().all(|s| s.is_none()),
            "states を書かなければ全部既定ルール"
        );
    }

    /// TOML の整数（`w = 22`）が f64 として読める。投稿者が `22.0` と書かなくてよい。
    #[test]
    fn integers_are_accepted_where_floats_are_expected() {
        let f = parse(MINIMAL, Source::Builtin).unwrap();
        assert_eq!(f.body_size(Size::Dock), (36.0, 34.0));
    }

    #[test]
    fn parses_a_path_outline_with_states_and_details() {
        let text = r#"
id = "plated"
label = "かぶと"
author = "tester"
[size]
bar  = { w = 18, h = 20 }
dock = { w = 30, h = 34 }
[outline]
kind = "path"
half = true
d = """
M 0.500 0.000
L 0.615 0.000
C 0.700 0.020 0.770 0.075 0.815 0.170
"""
[eyes]
shape = "polygon"
v = 0.58
gap  = { bar = 2.5, dock = 4.0 }
size = { bar = [5.5, 3.0], dock = [9.0, 4.6] }
polygon = [[0.00,0.00],[0.00,0.62],[1.00,1.00],[0.94,0.38]]
[eyes.states.idle]
h_scale = 0.55
color = "eye_closed"
[[details]]
name = "brow"
sizes = ["bar", "dock"]
points = [[0.19,0.725],[0.50,0.775],[0.81,0.725]]
[[details]]
name = "forehead"
sizes = ["dock"]
points = [[0.425,0.78],[0.425,0.945]]
"#;
        let f = parse(text, Source::Builtin).unwrap();
        assert_eq!(f.author.as_deref(), Some("tester"));
        assert_eq!(f.eyes.v, 0.58);
        assert_eq!(f.eyes.shape, EyeShape::Polygon);
        match &f.outline {
            OutlineSpec::Path { half, segs, .. } => {
                assert!(half);
                assert_eq!(segs.len(), 2);
            }
            other => panic!("path のはず: {other:?}"),
        }
        // idle だけ上書きされ、他は既定ルール。
        let idle = crate::face::spec::state_index(SessionState::Idle);
        assert!(f.eyes.states[idle].is_some());
        assert_eq!(f.eyes.states[idle].unwrap().h_scale, 0.55);
        assert!(f.eyes.states[crate::face::spec::state_index(SessionState::Done)].is_none());
        // details は sizes で絞られる。
        assert_eq!(f.face_details(18.0, 20.0, Size::Bar).len(), 1);
        assert_eq!(f.face_details(30.0, 34.0, Size::Dock).len(), 2);
    }

    // ---- エラー -------------------------------------------------------------

    fn err_of(text: &str) -> Vec<Problem> {
        parse(text, Source::Builtin).unwrap_err()
    }

    #[test]
    fn rejects_a_bad_id() {
        let text = MINIMAL.replace("id = \"test\"", "id = \"My Face\"");
        let e = err_of(&text);
        assert!(e.iter().any(|p| p.code == ProblemCode::Id), "{e:?}");
    }

    #[test]
    fn rejects_unknown_fields() {
        // 綴り間違いを黙って無視しない。
        let text = format!("{MINIMAL}\nlabel_ja = \"だめ\"\n");
        let e = err_of(&text);
        assert!(e.iter().any(|p| p.code == ProblemCode::Parse), "{e:?}");
    }

    #[test]
    fn rejects_both_corner_forms() {
        let text = MINIMAL.replace(
            "corners = [[0.5,0.5],[0.5,0.5],[0.5,0.5],[0.5,0.5]]",
            "corners = [[0.5,0.5],[0.5,0.5],[0.5,0.5],[0.5,0.5]]\n\
             corners_pt = { bar = 7.0, dock = 10.0 }",
        );
        let e = err_of(&text);
        assert!(
            e.iter().any(|p| p.message.contains("cannot both be set")),
            "{e:?}"
        );
    }

    #[test]
    fn rejects_neither_corner_form() {
        let text = MINIMAL.replace("corners = [[0.5,0.5],[0.5,0.5],[0.5,0.5],[0.5,0.5]]", "");
        let e = err_of(&text);
        assert!(
            e.iter().any(|p| p.message.contains("requires either")),
            "{e:?}"
        );
    }

    #[test]
    fn rejects_polygon_shape_without_a_polygon() {
        let text = MINIMAL.replace("shape = \"rounded\"", "shape = \"polygon\"");
        let e = err_of(&text);
        assert!(
            e.iter().any(|p| p.message.contains("requires polygon")),
            "{e:?}"
        );
    }

    #[test]
    fn rejects_an_unknown_state_key() {
        let text = format!("{MINIMAL}\n[eyes.states.sleeping]\nh_scale = 0.5\n");
        let e = err_of(&text);
        assert!(
            e.iter().any(|p| p.message.contains("unknown state")),
            "{e:?}"
        );
    }

    #[test]
    fn rejects_an_unknown_eye_color() {
        let text = format!("{MINIMAL}\n[eyes.states.idle]\ncolor = \"purple\"\n");
        let e = err_of(&text);
        assert!(e.iter().any(|p| p.message.contains("color")), "{e:?}");
    }

    #[test]
    fn collects_several_problems_at_once() {
        // 投稿者が 1 回の実行で全部直せること。
        let text = r#"
id = "Bad Id"
label = ""
[size]
bar  = { w = 22, h = 20 }
dock = { w = 36, h = 34 }
[outline]
kind = "corners"
[eyes]
shape = "rounded"
gap  = { bar = 3.0, dock = 5.0 }
size = { bar = [3.0, 4.0], dock = [4.0, 6.0] }
"#;
        let e = err_of(text);
        assert!(e.len() >= 3, "問題をまとめて返していない: {e:?}");
    }
}
