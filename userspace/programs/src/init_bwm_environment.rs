/// Session settings inherited by the desktop compositor, when present.
pub fn bwm_environment(mut lookup: impl FnMut(&str) -> Option<String>) -> Vec<String> {
    ["BREENIX_UI_CLOCK", "BREENIX_INPUT_DEVICES", "BREENIX_CONFIG_ROOT", "BREENIX_UI_SEED", "BREENIX_UI_MODE"]
        .into_iter().filter_map(|key| lookup(key).map(|value| format!("{key}={value}"))).collect()
}
