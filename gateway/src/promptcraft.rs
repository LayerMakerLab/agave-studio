//! Safe wrapper over the C++ prompt-craft engine (engine/promptcraft).

use std::collections::HashSet;
use std::ffi::{CStr, CString};
use std::os::raw::c_char;

use serde_json::Value;

extern "C" {
    fn pc_compose(prompt: *const c_char, camera_id: *const c_char, style_id: *const c_char) -> *const c_char;
    fn pc_presets_json() -> *const c_char;
    fn pc_free(p: *const c_char);
}

/// Copies a C string returned by the engine and frees the original.
///
/// # Safety
/// `p` must be null or a pointer returned by pc_compose / pc_presets_json that has not been freed.
unsafe fn take_c_string(p: *const c_char) -> Option<String> {
    if p.is_null() {
        return None;
    }
    let s = CStr::from_ptr(p).to_string_lossy().into_owned();
    pc_free(p);
    Some(s)
}

fn to_cstring(s: &str) -> CString {
    // Interior NULs cannot cross the FFI boundary; drop them.
    CString::new(s.replace('\0', "")).expect("no NUL bytes after replace")
}

/// Appends camera-move and style language to a prompt. Unknown ids are ignored by the engine.
pub fn compose(prompt: &str, camera: Option<&str>, style: Option<&str>) -> String {
    let p = to_cstring(prompt);
    let c = camera.map(to_cstring);
    let s = style.map(to_cstring);
    let c_ptr = c.as_ref().map_or(std::ptr::null(), |c| c.as_ptr());
    let s_ptr = s.as_ref().map_or(std::ptr::null(), |s| s.as_ptr());
    // SAFETY: all pointers are valid NUL-terminated strings that outlive the call.
    unsafe { take_c_string(pc_compose(p.as_ptr(), c_ptr, s_ptr)) }.unwrap_or_else(|| prompt.to_string())
}

/// The preset catalog as served at GET /api/presets, plus the id sets used for validation.
#[derive(Clone)]
pub struct PresetCatalog {
    pub json: Value,
    pub camera_ids: HashSet<String>,
    pub style_ids: HashSet<String>,
}

pub fn presets() -> Result<PresetCatalog, String> {
    // SAFETY: the engine returns a malloc'd string or null; take_c_string frees it.
    let raw = unsafe { take_c_string(pc_presets_json()) }.ok_or("engine returned no preset list")?;
    let json: Value = serde_json::from_str(&raw).map_err(|e| format!("preset JSON invalid: {e}"))?;
    let ids = |key: &str| -> HashSet<String> {
        json.get(key)
            .and_then(Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(|o| o.get("id").and_then(Value::as_str).map(String::from))
                    .collect()
            })
            .unwrap_or_default()
    };
    Ok(PresetCatalog { camera_ids: ids("camera"), style_ids: ids("styles"), json })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compose_appends_camera_and_style_phrases() {
        let out = compose("A red bicycle on a foggy pier.", Some("dolly_in"), Some("film_35"));
        assert!(out.starts_with("A red bicycle on a foggy pier"), "got: {out}");
        assert!(out.contains("slow dolly in"), "got: {out}");
        assert!(out.contains("35mm film"), "got: {out}");
        assert!(out.ends_with('.'), "got: {out}");
    }

    #[test]
    fn compose_without_presets_returns_prompt_unchanged() {
        assert_eq!(compose("A red bicycle.", None, None), "A red bicycle.");
    }

    #[test]
    fn compose_ignores_unknown_ids() {
        assert_eq!(compose("A cat", Some("no_such_move"), None), "A cat");
    }

    #[test]
    fn presets_list_has_twenty_cameras_and_ten_styles() {
        let p = presets().expect("presets");
        assert_eq!(p.camera_ids.len(), 20);
        assert_eq!(p.style_ids.len(), 10);
        assert!(p.camera_ids.contains("dolly_in"));
        assert!(p.camera_ids.contains("static_lockoff"));
        assert!(p.style_ids.contains("anime"));
        assert_eq!(p.json["camera"].as_array().map(Vec::len), Some(20));
    }
}
