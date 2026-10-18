//! Self-host workspace data (data/studio.json): saved Elements (named reference images called with
//! @tag in a prompt), the project style prefix and the still-first rule. Also upload sniffing and the
//! prompt recipes behind grids, sheets and one-change edits.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use crate::fsutil;

pub const MAX_ELEMENTS: usize = 200;
pub const MAX_ELEMENT_IMAGES: usize = 4;
pub const MAX_STYLE_PREFIX: usize = 800;
pub const MAX_NAME: usize = 60;
/// Largest uploaded file (images, plus audio and video for assembly).
pub const MAX_UPLOAD_BYTES: usize = 40 << 20;
pub const KINDS: [&str; 5] = ["character", "product", "location", "prop", "style"];

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Element {
    /// Lowercase handle used in prompts as @tag.
    pub tag: String,
    /// Words that replace @tag in the prompt sent to the model.
    pub name: String,
    pub kind: String,
    /// /outputs/... images, sent to the model as references in this order.
    pub images: Vec<String>,
    pub created_at: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct Project {
    /// Glued to the front of every image and video prompt.
    #[serde(default)]
    pub style_prefix: String,
    /// When on, text-to-video is refused and video starts from an approved still. Off by default.
    #[serde(default)]
    pub still_first: bool,
}

fn yes() -> bool {
    true
}


/// One clip on the saved Agave Edit timeline.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct EditClip {
    /// /outputs/... video or picture.
    pub url: String,
    /// "video" or "image".
    #[serde(default)]
    pub kind: String,
    /// Trim inside the source, in seconds (for a picture, `to` is how long it shows).
    #[serde(default)]
    pub from: f64,
    #[serde(default)]
    pub to: Option<f64>,
    /// Length of the whole source, as the browser measured it.
    #[serde(default)]
    pub len: Option<f64>,
    #[serde(default)]
    pub caption: String,
}

/// A caption correction: a word whisper heard one way, shown another way ("" hides it).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct WordFix {
    pub from: String,
    #[serde(default)]
    pub to: String,
}

/// The saved Agave Edit timeline, so an edit survives a reload or a restart.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct EditDoc {
    #[serde(default)]
    pub clips: Vec<EditClip>,
    /// "original", "16:9", "9:16", "1:1" or "4:5".
    #[serde(default)]
    pub frame: String,
    #[serde(default)]
    pub fill: bool,
    #[serde(default = "yes")]
    pub keep_sound: bool,
    #[serde(default)]
    pub song_url: Option<String>,
    #[serde(default)]
    pub song_name: Option<String>,
    /// Auto captions from the clips' speech: "off", "simple" or "pop" (word by word).
    #[serde(default = "off")]
    pub auto_captions: String,
    #[serde(default)]
    pub word_fixes: Vec<WordFix>,
    /// With fill: the crop follows the subject.
    #[serde(default)]
    pub follow: bool,
    #[serde(default)]
    pub clean_voice: bool,
    /// Bring the export to -14 LUFS.
    #[serde(default)]
    pub loudness: bool,
}

fn off() -> String {
    "off".into()
}

impl Default for EditDoc {
    fn default() -> Self {
        EditDoc {
            clips: Vec::new(),
            frame: "original".into(),
            fill: false,
            keep_sound: true,
            song_url: None,
            song_name: None,
            auto_captions: off(),
            word_fixes: Vec::new(),
            follow: false,
            clean_voice: false,
            loudness: false,
        }
    }
}

#[derive(Serialize, Deserialize, Default)]
struct StudioFile {
    #[serde(default)]
    elements: Vec<Element>,
    #[serde(default)]
    project: Project,
    #[serde(default)]
    edit: EditDoc,
    /// Goes up by one on every timeline save, so a page holding an older copy cannot write over newer clips.
    #[serde(default)]
    edit_rev: u64,
}

#[derive(Clone)]
pub struct Studio {
    path: Arc<PathBuf>,
    inner: Arc<Mutex<StudioFile>>,
}

#[derive(Debug, PartialEq)]
pub enum StudioError {
    Full,
    Io,
    /// The timeline changed since the caller read it (its rev is old).
    Stale,
}

/// A tag: 1 to 32 of a-z, 0-9, _ and -, starting with a letter or digit.
pub fn valid_tag(t: &str) -> bool {
    !t.is_empty()
        && t.len() <= 32
        && t.chars().next().is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        && t.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
}

impl Studio {
    pub fn load(path: PathBuf) -> Result<Self, String> {
        let file: StudioFile = crate::jobs::read_state(&path)?;
        Ok(Studio { path: Arc::new(path), inner: Arc::new(Mutex::new(file)) })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, StudioFile> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn save(&self, f: &StudioFile) -> Result<(), StudioError> {
        let bytes = serde_json::to_vec_pretty(f).map_err(|_| StudioError::Io)?;
        fsutil::write_private(&self.path, &bytes).map_err(|_| StudioError::Io)
    }

    pub fn elements(&self) -> Vec<Element> {
        self.lock().elements.clone()
    }

    /// Adds an element, or replaces the one with the same tag.
    pub fn put_element(&self, e: Element) -> Result<(), StudioError> {
        let mut f = self.lock();
        if let Some(slot) = f.elements.iter_mut().find(|x| x.tag == e.tag) {
            *slot = e;
        } else {
            if f.elements.len() >= MAX_ELEMENTS {
                return Err(StudioError::Full);
            }
            f.elements.push(e);
        }
        self.save(&f)
    }

    /// True when an element was removed.
    pub fn delete_element(&self, tag: &str) -> Result<bool, StudioError> {
        let mut f = self.lock();
        let before = f.elements.len();
        f.elements.retain(|x| x.tag != tag);
        if f.elements.len() == before {
            return Ok(false);
        }
        self.save(&f).map(|_| true)
    }

    pub fn project(&self) -> Project {
        self.lock().project.clone()
    }

    pub fn set_project(&self, p: Project) -> Result<(), StudioError> {
        let mut f = self.lock();
        f.project = p;
        self.save(&f)
    }

    pub fn edit(&self) -> EditDoc {
        self.lock().edit.clone()
    }

    /// The saved timeline and its revision.
    pub fn edit_rev(&self) -> (EditDoc, u64) {
        let f = self.lock();
        (f.edit.clone(), f.edit_rev)
    }

    /// Saves the timeline and returns its new revision. With `expect`, only when the saved one is still at that
    /// revision (else Stale); without it, always, as before revisions existed.
    pub fn set_edit_checked(&self, e: EditDoc, expect: Option<u64>) -> Result<u64, StudioError> {
        let mut f = self.lock();
        if expect.is_some_and(|r| r != f.edit_rev) {
            return Err(StudioError::Stale);
        }
        let before = (f.edit.clone(), f.edit_rev);
        f.edit = e;
        f.edit_rev += 1;
        match self.save(&f) {
            Ok(()) => Ok(f.edit_rev),
            Err(err) => {
                (f.edit, f.edit_rev) = before;
                Err(err)
            }
        }
    }

    pub fn set_edit(&self, e: EditDoc) -> Result<(), StudioError> {
        self.set_edit_checked(e, None).map(|_| ())
    }

    /// Replaces each @tag of a saved element with its name and returns the element images in the
    /// order the tags appear (each element once). An @ right after a letter or digit (an email) and
    /// unknown tags are left as they are.
    pub fn expand(&self, prompt: &str) -> (String, Vec<String>) {
        let f = self.lock();
        let chars: Vec<char> = prompt.chars().collect();
        let mut out = String::with_capacity(prompt.len());
        let mut images: Vec<String> = Vec::new();
        let mut used: Vec<&str> = Vec::new();
        let mut i = 0;
        while i < chars.len() {
            let c = chars[i];
            let after_word = i > 0 && chars[i - 1].is_ascii_alphanumeric();
            if c == '@' && !after_word {
                let mut j = i + 1;
                while j < chars.len() && (chars[j].is_ascii_alphanumeric() || chars[j] == '_' || chars[j] == '-') {
                    j += 1;
                }
                let tag: String = chars[i + 1..j].iter().collect::<String>().to_ascii_lowercase();
                // A trailing - or _ is punctuation, not part of the tag.
                let tag = tag.trim_end_matches(['-', '_']).to_string();
                let end = i + 1 + tag.chars().count();
                if let Some(e) = f.elements.iter().find(|e| e.tag == tag) {
                    out.push_str(&e.name);
                    if !used.contains(&e.tag.as_str()) {
                        used.push(e.tag.as_str());
                        for img in &e.images {
                            if !images.contains(img) {
                                images.push(img.clone());
                            }
                        }
                    }
                    i = end;
                    continue;
                }
            }
            out.push(c);
            i += 1;
        }
        (out, images)
    }
}

/// What an uploaded file really is, from its first bytes (the declared type is ignored).
/// Returns (extension, kind).
pub fn sniff(b: &[u8]) -> Option<(&'static str, &'static str)> {
    let at = |off: usize, sig: &[u8]| b.len() >= off + sig.len() && &b[off..off + sig.len()] == sig;
    if at(0, b"\x89PNG\r\n\x1a\n") {
        Some(("png", "image"))
    } else if at(0, b"\xff\xd8\xff") {
        Some(("jpg", "image"))
    } else if at(0, b"RIFF") && at(8, b"WEBP") {
        Some(("webp", "image"))
    } else if at(0, b"RIFF") && at(8, b"WAVE") {
        Some(("wav", "audio"))
    } else if at(4, b"ftypM4A") {
        Some(("m4a", "audio"))
    } else if at(4, b"ftypqt") {
        Some(("mov", "video"))
    } else if at(4, b"ftyp") {
        Some(("mp4", "video"))
    } else if at(0, b"ID3") || (b.len() > 1 && b[0] == 0xff && (b[1] & 0xe0) == 0xe0) {
        Some(("mp3", "audio"))
    } else if at(0, b"\x1a\x45\xdf\xa3") {
        Some(("webm", "video"))
    } else {
        None
    }
}

/// Prompt recipes. `detail` is the user's text; some recipes have a default when it is empty.
pub fn recipe(id: &str, detail: &str) -> Option<String> {
    let d = detail.trim();
    let or = |default: &str| if d.is_empty() { default.to_string() } else { d.to_string() };
    let lock = "Keep the exact same person, face, skin texture, outfit, lighting and camera framing as the reference image";
    Some(match id {
        "grid4" => format!(
            "A 2x2 grid of four frames of the exact same subject as the reference image. {lock} in all four frames. \
             Change only this between frames: {}. Thin even gutters, no text, no labels.",
            or("a natural, distinct pose in each frame")
        ),
        "grid9" => format!(
            "A 3x3 grid of nine frames of the exact same subject as the reference image. {lock} in all nine frames. \
             Change only this between frames: {}. Thin even gutters, no text, no labels.",
            or("the background, a different simple setting in each frame")
        ),
        "edit" => {
            if d.is_empty() {
                return None;
            }
            format!(
                "Keep the exact same person, face, skin texture, expression, pose, background, proportions and camera \
                 framing as the reference image. Change only this: {d}."
            )
        }
        "sheet" => format!(
            "Character reference sheet, three panels side by side on a flat neutral gray background with no shadows. \
             Left: the full body from the front, cropped at the neck so no face shows. Middle: the same full body from \
             the back. Right: a tight close-up of the face looking straight into the camera. The same original \
             character in every panel, only one face in the image. {}",
            or("Natural skin texture, no beauty filter.")
        ),
        "product" => format!(
            "Product reference sheet: a 3x3 grid of the product from the reference image on a plain light gray \
             background, a clearly different camera angle in every panel. Top row: straight front, three-quarter \
             left, left side. Middle row: straight back, three-quarter right, right side. Bottom row: directly \
             overhead looking down, low angle looking up from below, close-up of one detail. Keep its shape, label \
             text, colours and materials exactly the same. No other objects, no added text, logos or marks. {}",
            d
        ),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn studio(name: &str) -> Studio {
        Studio::load(fsutil::test_dir(name).join("studio.json")).unwrap()
    }

    fn el(tag: &str, name: &str, images: &[&str]) -> Element {
        Element {
            tag: tag.into(),
            name: name.into(),
            kind: "character".into(),
            images: images.iter().map(|s| s.to_string()).collect(),
            created_at: 1,
        }
    }

    #[test]
    fn tags_expand_to_names_and_collect_images_once() {
        let s = studio("studio_expand");
        s.put_element(el("mia", "Mia, a woman in her late 20s", &["/outputs/a.png", "/outputs/b.png"])).unwrap();
        s.put_element(el("mug", "the cream mug", &["/outputs/m.png", "/outputs/a.png"])).unwrap();
        let (p, imgs) = s.expand("@Mia holds @mug. Close-up of @mia, mail me at x@mia.com about @nobody.");
        assert_eq!(
            p,
            "Mia, a woman in her late 20s holds the cream mug. Close-up of Mia, a woman in her late 20s, mail me at x@mia.com about @nobody."
        );
        assert_eq!(imgs, vec!["/outputs/a.png", "/outputs/b.png", "/outputs/m.png"]);
    }

    #[test]
    fn elements_and_project_survive_a_reload() {
        let dir = fsutil::test_dir("studio_reload");
        let s = Studio::load(dir.join("studio.json")).unwrap();
        assert!(!s.project().still_first, "still-first is off by default");
        s.put_element(el("mia", "Mia", &["/outputs/a.png"])).unwrap();
        s.put_element(el("mia", "Mia R", &["/outputs/c.png"])).unwrap();
        s.set_project(Project { style_prefix: "35mm film".into(), still_first: false }).unwrap();
        let again = Studio::load(dir.join("studio.json")).unwrap();
        assert_eq!(again.elements(), vec![el("mia", "Mia R", &["/outputs/c.png"])]);
        assert_eq!(again.project(), Project { style_prefix: "35mm film".into(), still_first: false });
        assert_eq!(again.delete_element("mia"), Ok(true));
        assert_eq!(again.delete_element("mia"), Ok(false));
    }

    #[test]
    fn the_edit_timeline_survives_a_reload() {
        let s = studio("studio_edit");
        assert_eq!(s.edit(), EditDoc::default());
        let doc = EditDoc {
            clips: vec![EditClip { url: "/outputs/a.mp4".into(), kind: "video".into(), from: 1.0, to: Some(2.5), len: Some(5.0), caption: "Hi".into() }],
            frame: "9:16".into(),
            fill: true,
            keep_sound: false,
            song_url: None,
            song_name: None,
            auto_captions: "pop".into(),
            word_fixes: vec![WordFix { from: "bamboo".into(), to: "Bambu".into() }],
            follow: true,
            clean_voice: true,
            loudness: true,
        };
        s.set_edit(doc.clone()).unwrap();
        let again = Studio::load((*s.path).clone()).unwrap();
        assert_eq!(again.edit(), doc);
    }

    #[test]
    fn tags_are_strict() {
        for ok in ["mia", "m", "hero-2", "s_hero", "9lives"] {
            assert!(valid_tag(ok), "{ok}");
        }
        for bad in ["", "Mia", "-mia", "_x", "mi a", "mía", &"a".repeat(33)] {
            assert!(!valid_tag(bad), "{bad}");
        }
    }

    #[test]
    fn sniff_uses_content_not_names() {
        assert_eq!(sniff(b"\x89PNG\r\n\x1a\nrest"), Some(("png", "image")));
        assert_eq!(sniff(b"\xff\xd8\xff\xe0...."), Some(("jpg", "image")));
        assert_eq!(sniff(b"RIFF\0\0\0\0WEBPVP8 "), Some(("webp", "image")));
        assert_eq!(sniff(b"\0\0\0\x18ftypisom"), Some(("mp4", "video")));
        assert_eq!(sniff(b"ID3\x04\0"), Some(("mp3", "audio")));
        assert_eq!(sniff(b"<svg onload=alert(1)>"), None);
        assert_eq!(sniff(b"<html>"), None);
    }

    #[test]
    fn recipes_lock_identity_and_need_a_change_for_edits() {
        assert!(recipe("grid4", "").unwrap().contains("Keep the exact same person"));
        assert!(recipe("grid9", "a new outfit").unwrap().contains("Change only this between frames: a new outfit."));
        assert!(recipe("edit", "  ").is_none());
        assert!(recipe("edit", "a red scarf").unwrap().ends_with("Change only this: a red scarf."));
        assert!(recipe("sheet", "").unwrap().contains("flat neutral gray"));
        assert!(recipe("nope", "x").is_none());
    }
}
