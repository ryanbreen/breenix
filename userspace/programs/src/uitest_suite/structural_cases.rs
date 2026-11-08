use libuitest::{compare::{self, Frame, Format, Options}, formula};
use libuitest::schema::{self, Context};

type Result = core::result::Result<(), String>;
fn frame(bytes: &[u8]) -> Frame<'_> { Frame::new(bytes, 4, 4, 16, Format::Bgrx).unwrap() }
fn check(predicate: &str, params: &str) -> schema::Check {
    let text = format!(r##"{{"version":1,"case":"frames/test","check":"test","kind":"structural","source":{{"kind":"screen"}},"at":{{"checkpoint":"test","requires":[]}},"regions":[{{"name":"all","x":0,"y":0,"w":4,"h":4}}],"mask":[],"tolerance":{{"channel":0,"maxDiffering":0}},"predicate":"{predicate}","params":{params}}}"##);
    let value = schema::from_str(&text).unwrap();
    schema::validate(&value, &Context { frame: Some((4,4)), formulas: &[], previous: None }).unwrap()
}
fn judge(pixels: &[u8], check: &schema::Check) -> core::result::Result<bool, String> {
    compare::structural(frame(pixels), check).map_err(|e| format!("{e:?}"))
}
pub fn uniform() -> Result {
    let c = check("uniform", r##"{"region":"all","colour":"#ff0000"}"##);
    let mut pixels = [0u8;64]; for p in pixels.chunks_exact_mut(4) { p.copy_from_slice(&[0,0,255,255]); }
    if !judge(&pixels,&c)? { return Err("flat fill failed".into()); }
    pixels[0]=1;
    if judge(&pixels,&c)? { return Err("stray pixel passed".into()); } Ok(())
}
pub fn outside() -> Result {
    let c = check("outside", r##"{"region":"all","rect":{"x":1,"y":1,"w":2,"h":2},"colour":"#ff0000"}"##);
    let mut pixels=[0u8;64]; pixels[22]=255;
    if !judge(&pixels,&c)? { return Err("contained colour failed".into()); }
    pixels[2]=255;
    if judge(&pixels,&c)? { return Err("leaked colour passed".into()); } Ok(())
}
pub fn count() -> Result {
    let c=check("count", r#"{"region":"all","min":2}"#);
    let mut pixels=[0u8;64]; pixels[0]=1;
    if !judge(&pixels,&c)? { return Err("two colours failed".into()); }
    pixels[0]=0;
    if judge(&pixels,&c)? { return Err("blank frame passed".into()); } Ok(())
}
pub fn formula() -> Result {
    let expected = formula::reference(4,4, |x,y| crate::formula_pattern::pixel(x,y));
    let image = libimg::png::decode(include_bytes!("../../../../tests/fixtures/ui-formula.png")).map_err(|e| format!("{e:?}"))?;
    let mut actual=expected.clone();
    for changed in [false,true] {
        if changed { actual[4]=9; }
        let a=compare::compare(frame(&expected),frame(&actual),Options::default()).map_err(|e| format!("{e:?}"))?;
        let b=compare::compare(frame(&image.pixels),frame(&actual),Options::default()).map_err(|e| format!("{e:?}"))?;
        if a.comparison.passed()!=b.comparison.passed() || a.comparison.first!=b.comparison.first { return Err("formula and PNG disagree".into()); }
        if a.comparison.passed()==changed || (changed && a.comparison.first.is_none()) { return Err("wrong comparison verdict or missing first pixel".into()); }
    } Ok(())
}
