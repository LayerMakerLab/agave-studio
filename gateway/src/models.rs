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
    /// Cost in the provider's own credits, shown when there is no US-dollar list price (Meshy).
    #[serde(default)]
    pub provider_credits: Option<u32>,
}

impl ModelDef {
    /// Public JSON shape from the contract. `ready` means the key slot is set.
    pub fn view(&self, ready: bool) -> Value {
        let price = self.price.as_ref().map(|p| json!({"per": p.per, "usd": p.usd, "by_resolution": p.by_resolution}));
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
            if let Some(p) = &m.price {
                let ok_rate = |r: f64| r.is_finite() && r > 0.0 && r < 100.0;
                if !ok_rate(p.usd) || !p.by_resolution.values().all(|&r| ok_rate(r) && r <= p.usd) {
                    return Err(format!("{}: price rates must be positive and usd must be the highest", m.id));
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
    fn bundled_registry_parses_and_is_consistent() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("models.toml");
        let reg = Registry::load(&path).expect("models.toml must load");
        assert!(reg.models.len() >= 20, "expected about 20 models, got {}", reg.models.len());
        assert!(reg.get("fal/kling-v2.1-master-t2v").is_some());
        assert!(reg.get("fal/kling-o3-i2v").is_some());
        assert!(reg.models.iter().filter(|m| m.price.is_some()).count() >= 10);
    }
}
