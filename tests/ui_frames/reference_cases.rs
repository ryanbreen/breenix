//! Stored-reference and binding cases; the reference is an authored coordinate pattern.
use libuitest::{formula, frames::{self, Bindings}};
use libuitest::schema::{self, Value};
use std::path::Path;

pub const RELATIVE: &str = "ui-testing/coordinate-pattern.png";
pub const PIN: &str = "244b7ddb41d62cfb5969a036c3141662592798e5505cdce5dcddb829a64d87d9";

pub fn definition(case: &str) -> Value {
    serde_json::json!({"version":1, "case":case, "check":"pattern", "kind":"stored",
        "source":{"kind":"surface"}, "at":{"checkpoint":"after-pattern", "requires":[]},
        "reference":{"path":RELATIVE, "sha256":PIN},
        "regions":[{"name":"all", "x":0, "y":0, "w":4, "h":4}],
        "mask":[], "tolerance":{"channel":0, "maxDiffering":0}})
}

pub fn reference_pin_at(root: &Path) -> Result<(), String> {
    let path = root.join(RELATIVE);
    frames::load_reference(&path, PIN)?;
    // Corrupt both the digest and PNG header: pin refusal must precede decoding.
    let mut bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
    bytes[0] ^= 1;
    let error = frames::decode_reference(&path, &bytes, PIN).unwrap_err();
    if !error.contains("pin mismatch") || !error.contains(RELATIVE) { return Err(error); }
    let error = frames::decode_reference(&path, &bytes, &frames::digest(&bytes)).unwrap_err();
    if !error.contains("PNG") { return Err(format!("matched pin did not reach decoder: {error}")); }
    Ok(())
}

pub fn reference_missing_at(root: &Path) -> Result<(), String> {
    let path = root.join("ui-testing/missing-reference.png");
    let error = frames::load_reference(&path, PIN).unwrap_err();
    if !error.contains("missing reference") || !error.contains(path.to_str().unwrap()) { return Err(error); }
    Ok(())
}

pub fn png_reference_at(root: &Path) -> Result<(), String> {
    let image = frames::load_reference(&root.join(RELATIVE), PIN)?;
    let pixels = formula::reference(4, 4, crate::formula_pattern::pixel);
    if image.pixels != pixels || (image.width, image.height) != (4, 4) {
        return Err("generated PNG differs from authored pixels".into());
    }
    let encoded = libimg_encode::encode(&image).map_err(|e| format!("{e:?}"))?;
    let decoded = frames::decode_reference(Path::new("roundtrip.png"), &encoded, &frames::digest(&encoded))?;
    if decoded != image { return Err("PNG round trip changed pixels".into()); }
    Ok(())
}

pub fn schema_binding() -> Result<(), String> {
    let mut definition = definition("frames/schema-binding");
    definition["source"] = serde_json::json!({"kind":"window", "window":"never-opened"});
    let check = schema::validate(&definition, &schema::Context { frame:Some((4,4)), formulas:&[], previous:None })?;
    let mut bindings = Bindings::default();
    bindings.checkpoint("after-pattern");
    let error = bindings.finish(&[&check]).unwrap_err();
    if !error.contains("never-opened") { return Err(error); }
    bindings.window("never-opened");
    bindings.finish(&[&check])?;
    definition["at"]["checkpoint"] = "never-reached".into();
    let check = schema::validate(&definition, &schema::Context { frame:Some((4,4)), formulas:&[], previous:None })?;
    let error = bindings.finish(&[&check]).unwrap_err();
    if !error.contains("never-reached") { return Err(error); }
    bindings.checkpoint("never-reached");
    bindings.finish(&[&check])
}
