//! Frame schema cases from docs/gui/plan.md section 7.5.
use crate::frame_case::{fail, CaseResult};
use crate::frame_schema::{self, Context, Formula, Param, ParamType, Value};

// Suite-source signatures, also compiled by the manifest validator.
pub const FORMULAS: &[Formula] = &[Formula {
    name: "schema-pattern",
    params: &[
        Param {
            name: "unsigned",
            kind: ParamType::U32(0, 10),
        },
        Param {
            name: "signed",
            kind: ParamType::I32(-2, 2),
        },
        Param {
            name: "colour",
            kind: ParamType::Colour,
        },
        Param {
            name: "rect",
            kind: ParamType::Rect,
        },
        Param {
            name: "window",
            kind: ParamType::WindowKey,
        },
        Param {
            name: "seed",
            kind: ParamType::Seed,
        },
    ],
}];

pub fn fixtures() -> Vec<Value> {
    frame_schema::from_str(include_str!(
        "../fixtures/frame-schema-v1.json"
    ))
    .expect("committed frame schema fixture table")
}

pub fn verdict(row: &Value) -> Result<frame_schema::Check, String> {
    let frame = Some((
        row["frame"][0].as_u64().unwrap() as u32,
        row["frame"][1].as_u64().unwrap() as u32,
    ));
    let previous = row
        .get("previous")
        .map(|value| {
            frame_schema::validate(
                value,
                &Context {
                    frame,
                    formulas: FORMULAS,
                    previous: None,
                },
            )
        })
        .transpose()?;
    frame_schema::validate(
        &row["check"],
        &Context {
            frame,
            formulas: FORMULAS,
            previous: previous.as_ref(),
        },
    )
}

fn group(name: &str) -> CaseResult {
    let mut seen = 0;
    for row in fixtures().iter().filter(|row| row["group"] == name) {
        seen += 1;
        match (verdict(row), row["error"].as_str()) {
            (Ok(_), None) => {}
            (Err(error), Some(expected)) if error.contains(expected) => {}
            (result, expected) => {
                return fail(format!(
                    "schema {}: expected {expected:?}, got {result:?}",
                    row["name"]
                ))
            }
        }
    }
    if seen == 0 {
        return fail(format!("empty schema group {name}"));
    }
    Ok(())
}

pub fn unknown_field() -> CaseResult {
    group("unknown-field")
}
pub fn formula_params() -> CaseResult {
    group("formula-params")
}
pub fn schema_version() -> CaseResult {
    group("schema-version")
}

pub fn mask() -> CaseResult {
    group("mask")?;
    let rows = fixtures();
    let row = rows.iter().find(|r| r["name"] == "valid-mask").unwrap();
    let check = verdict(row)?;
    if check.compares(0, 0) || !check.compares(10, 10) || check.compared_area != 9900 {
        return fail("mask did not exclude exactly its rectangle");
    }
    Ok(())
}

pub fn mask_cap() -> CaseResult {
    group("mask-cap")?;
    let rows = fixtures();
    let check = verdict(
        rows.iter()
            .find(|r| r["name"] == "four-border-strips")
            .unwrap(),
    )?;
    if check.compared_area != 9604 {
        return fail("border strip union must leave 9604 pixels");
    }
    Ok(())
}

pub fn structural_vacuous() -> CaseResult {
    group("structural-vacuous")?;
    let rows = fixtures();
    let row = rows
        .iter()
        .find(|r| r["name"] == "outside-unmasked")
        .unwrap();
    let check = verdict(row)?;
    if check.effective_area != 396
        || !check.in_effective_domain(0, 0)
        || check.in_effective_domain(1, 1)
    {
        return fail("outside effective domain differs from border");
    }
    let mut pixels = vec![0u8; 200 * 100 * 4];
    let outside_matches = |pixels: &[u8]| {
        use libpixcheck::{Format, Frame, Options, Predicate, Rect};
        let frame =
            Frame::new(pixels, 200, 100, 800, Format::Bgrx).map_err(|e| format!("{e:?}"))?;
        libpixcheck::structural(
            frame,
            Options {
                regions: &[Rect {
                    x: 0,
                    y: 0,
                    w: 100,
                    h: 100,
                }],
                ..Options::default()
            },
            Predicate::Outside {
                region: 0,
                rect: Rect {
                    x: 1,
                    y: 1,
                    w: 98,
                    h: 98,
                },
                colour: [255, 0, 0],
            },
            None,
        )
        .map_err(|e| format!("{e:?}"))
    };
    if !outside_matches(&pixels)? {
        return fail("clean outside region failed");
    }
    pixels[2] = 255;
    if outside_matches(&pixels)? {
        return fail("outside missed planted border pixel");
    }
    Ok(())
}
