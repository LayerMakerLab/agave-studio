//! Model registry loaded from models.toml.

use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::keys::SLOTS;

#[derive(Deserialize, Serialize, Clone, Debug, Default)]
pub struct Params {
    #[serde(default)]
    pub aspect: Vec<String>,
    #[serde(default)]
    pub duration: Vec<u32>,
    #[serde(default)]
    pub resolution: Vec<String>,
    /// Native audio on (true) or off (false); the first value is the model's own default.
    #[serde(default)]
    pub audio: Vec<bool>,
}

/// Provider list price. The app shows it as an estimate of what a job costs on your own key.
#[derive(Deserialize, Clone, Debug)]
pub struct Price {
    /// "second" (video, times the duration) or "image".
    pub per: String,
    /// Default rate in USD. Must be the highest rate so an unlisted resolution is never under-estimated.
    pub usd: f64,
    #[serde(default)]
    pub by_resolution: BTreeMap<String, f64>,
    /// Rate with native audio "on" or "off", for models that bill audio separately (Kling 3.0, Kling O3, Veo 3.1).
    #[serde(default)]
    pub by_audio: BTreeMap<String, f64>,
}

impl Price {
    /// The rate for one job: the audio rate when the model bills audio, else the resolution rate, else `usd`.
    pub fn rate(&self, resolution: Option<&str>, audio: Option<bool>) -> f64 {
        if let Some(r) = audio.and_then(|a| self.by_audio.get(if a { "on" } else { "off" })) {
            return *r;
        }
        resolution.and_then(|r| self.by_resolution.get(r)).copied().unwrap_or(self.usd)
    }
}

#[derive(Deserialize, Clone, Debug)]
pub struct ModelDef {
    pub id: String,
    pub name: String,
    pub vendor: String,
    pub provider: String,
    pub endpoint: String,
    pub modality: String,
    pub modes: Vec<String>,
    pub key: String,
    /// Informational: mirrors the `verified` flag in models.toml. Not sent to clients.
    #[allow(dead_code)]
    #[serde(default)]
    pub verified: bool,
    #[serde(default)]
    pub params: Params,
    #[serde(default)]
    pub price: Option<Price>,
    /// How many reference images the model takes as a list (0: one image_url at most).
    #[serde(default)]
    pub max_refs: u32,
    /// How many @tagged Elements the model takes beside its start frame (Kling 3.0 `elements`; 0: none).
    #[serde(default)]
    pub max_elements: u32,
    /// Takes an end picture (fal `end_image_url`). Keyframed camera moves (moves.rs) then also send where the move
    /// ends, because Kling 3.0 often ignores camera words alone.
    #[serde(default)]
    pub end_frame: bool,
    /// Cost in the provider's own credits, shown when there is no US-dollar list price (Meshy).
    #[serde(default)]
    pub provider_credits: Option<u32>,
}

impl ModelDef {
    /// Public JSON shape from the contract. `ready` means the key slot is set.
    pub fn view(&self, ready: bool) -> Value {
        let price = self.price.as_ref().map(|p| json!({"per": p.per, "usd": p.usd, "by_resolution": p.by_resolution, "by_audio": p.by_audio}));
        json!({
            "id": self.id,
            "name": self.name,
            "vendor": self.vendor,
            "provider": self.provider,
            "modality": self.modality,
            "modes": self.modes,
            "params": self.params,
            "key": self.key,
            "ready": ready,
            "price": price,
            "max_refs": self.max_refs,
            "max_elements": self.max_elements,
            "end_frame": self.end_frame,
            "provider_credits": self.provider_credits,
        })
    }
}

#[derive(Deserialize)]
struct File {
    #[serde(default)]
    model: Vec<ModelDef>,
}

pub struct Registry {
    pub models: Vec<ModelDef>,
}

impl Registry {
    pub fn load(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?;
        let file: File = toml::from_str(&text).map_err(|e| format!("parse {}: {e}", path.display()))?;
        Self::validate(&file.model)?;
        Ok(Registry { models: file.model })
    }

    fn validate(models: &[ModelDef]) -> Result<(), String> {
        let mut ids = HashSet::new();
        for m in models {
            if !ids.insert(m.id.as_str()) {
                return Err(format!("duplicate model id {}", m.id));
            }
            if !SLOTS.iter().any(|s| s.env == m.key) {
                return Err(format!("{}: key {} is not a known slot", m.id, m.key));
            }
            if !matches!(m.modality.as_str(), "video" | "image" | "audio" | "3d") {
                return Err(format!("{}: bad modality {}", m.id, m.modality));
            }
            if m.modes.is_empty() {
                return Err(format!("{}: no modes", m.id));
            }
            if m.max_refs > 10 || (m.max_refs > 0 && !m.modes.iter().any(|x| matches!(x.as_str(), "i2i" | "i2v" | "i23d"))) {
                return Err(format!("{}: max_refs needs an image mode and at most 10", m.id));
            }
            if m.max_elements > 4 || (m.max_elements > 0 && (m.modes != ["i2v"] || m.max_refs > 0)) {
                return Err(format!("{}: max_elements is for one-picture image to video models, at most 4", m.id));
            }
            if m.end_frame && m.modes != ["i2v"] {
                return Err(format!("{}: end_frame is for image to video models", m.id));
            }
            if let Some(p) = &m.price {
                let ok_rate = |r: f64| r.is_finite() && r > 0.0 && r < 100.0;
                if !ok_rate(p.usd) || !p.by_resolution.values().chain(p.by_audio.values()).all(|&r| ok_rate(r) && r <= p.usd) {
                    return Err(format!("{}: price rates must be positive and usd must be the highest", m.id));
                }
                if !p.by_audio.is_empty()
                    && (!p.by_audio.keys().all(|k| k == "on" || k == "off") || m.params.audio.is_empty() || !p.by_resolution.is_empty())
                {
                    return Err(format!("{}: by_audio takes \"on\"/\"off\", needs params.audio and no by_resolution", m.id));
                }
                match p.per.as_str() {
                    "second" if m.params.duration.is_empty() => {
                        return Err(format!("{}: per-second price needs durations", m.id))
                    }
                    "second" | "image" => {}
                    other => return Err(format!("{}: bad price unit {other}", m.id)),
                }
            }
        }
        Ok(())
    }

    pub fn get(&self, id: &str) -> Option<&ModelDef> {
        self.models.iter().find(|m| m.id == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audio_rates_follow_the_fal_pages_and_default_to_the_first_listed_value() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("models.toml");
        let reg = Registry::load(&path).expect("models.toml must load");
        let k3 = reg.get("fal/kling-3-pro-i2v").expect("kling 3 pro i2v");
        let p = k3.price.as_ref().expect("priced");
        assert_eq!(k3.params.audio.first(), Some(&true), "fal's default for Kling 3.0 is audio on");
        assert_eq!(p.rate(None, Some(true)), 0.168);
        assert_eq!(p.rate(None, Some(false)), 0.112);
        let o3 = reg.get("fal/kling-o3-i2v").expect("kling o3");
        assert_eq!(o3.params.audio.first(), Some(&false), "fal's default for Kling O3 is audio off");
        assert_eq!(o3.price.as_ref().expect("priced").rate(None, o3.params.audio.first().copied()), 0.084);
        let veo = reg.get("fal/veo-3.1-t2v").expect("veo");
        assert_eq!(veo.price.as_ref().expect("priced").rate(Some("1080p"), Some(false)), 0.20);
        for m in ["fal/kling-3-pro-t2v", "fal/kling-3-standard-i2v", "fal/kling-o3-i2v"] {
            let d = &reg.get(m).expect(m).params.duration;
            assert_eq!(d.first(), Some(&5), "{m}: 5 s stays the default");
            assert!((3..=15).all(|s| d.contains(&s)), "{m}: fal takes 3 to 15 s");
        }
    }

    #[test]
    fn by_audio_needs_on_off_keys_and_an_audio_param() {
        let toml = |extra: &str, audio: &str| {
            format!(
                "[[model]]\nid = \"x\"\nname = \"X\"\nvendor = \"V\"\nprovider = \"fal\"\nendpoint = \"e\"\nmodality = \"video\"\nmodes = [\"t2v\"]\nkey = \"FAL_KEY\"\nparams = {{ duration = [5]{audio} }}\nprice = {{ per = \"second\", usd = 0.2{extra} }}\n"
            )
        };
        let load = |s: String| -> Result<(), String> {
            let f: File = toml::from_str(&s).map_err(|e| e.to_string())?;
            Registry::validate(&f.model)
        };
        assert!(load(toml(", by_audio = { on = 0.2, off = 0.1 }", ", audio = [true, false]")).is_ok());
        assert!(load(toml(", by_audio = { loud = 0.1 }", ", audio = [true, false]")).is_err(), "unknown key");
        assert!(load(toml(", by_audio = { on = 0.2, off = 0.1 }", "")).is_err(), "needs params.audio");
        assert!(load(toml(", by_audio = { on = 0.3 }", ", audio = [true]")).is_err(), "usd must be the highest");
    }

    #[test]
    fn bundled_registry_parses_and_is_consistent() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("models.toml");
        let reg = Registry::load(&path).expect("models.toml must load");
        assert!(reg.models.len() >= 20, "expected about 20 models, got {}", reg.models.len());
        assert!(reg.get("fal/kling-v2.1-master-t2v").is_some());
        assert!(reg.get("fal/kling-o3-i2v").is_some());
        assert!(reg.models.iter().filter(|m| m.price.is_some()).count() >= 10);
    }
}
