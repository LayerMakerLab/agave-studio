//! Provider key slots. Keys live in memory and in config/keys.env (0600). Values are never returned or logged.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::PathBuf;
use std::sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard};

use crate::fsutil;

pub struct Slot {
    pub env: &'static str,
    pub provider: &'static str,
    pub docs: &'static str,
}

/// Every key slot the gateway knows. POST /api/keys rejects any other name.
/// Must match config/keys.example.env (checked by a unit test).
pub const SLOTS: &[Slot] = &[
    Slot { env: "FAL_KEY", provider: "fal.ai", docs: "https://fal.ai/docs/model-apis/model-endpoints/queue" },
    Slot { env: "REPLICATE_API_TOKEN", provider: "Replicate", docs: "https://replicate.com/docs/reference/http" },
    Slot { env: "GEMINI_API_KEY", provider: "Google Gemini", docs: "https://ai.google.dev/gemini-api/docs/image-generation" },
    Slot { env: "OPENAI_API_KEY", provider: "OpenAI", docs: "https://developers.openai.com/api/docs/guides/image-generation" },
    Slot { env: "BFL_API_KEY", provider: "Black Forest Labs", docs: "https://docs.bfl.ai/quick_start/generating_images" },
    Slot { env: "ELEVENLABS_API_KEY", provider: "ElevenLabs", docs: "https://elevenlabs.io/docs/api-reference/text-to-speech/convert" },
    Slot { env: "MESHY_API_KEY", provider: "Meshy (3D)", docs: "https://docs.meshy.ai/en/api/image-to-3d" },
];

pub fn slot(env: &str) -> Option<&'static Slot> {
    SLOTS.iter().find(|s| s.env == env)
}

/// Keys are 8 to 512 printable ASCII characters: no whitespace, control characters or quotes
/// (quotes would be stripped when keys.env is read back, and non-ASCII cannot go in a header).
/// Error messages never echo the value.
pub fn validate_value(value: &str) -> Result<(), &'static str> {
    let n = value.chars().count();
    if !(8..=512).contains(&n) {
        return Err("value_length");
    }
    if value.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err("value_has_whitespace");
    }
    if !value.chars().all(|c| c.is_ascii_graphic() && c != '"' && c != '\'' && c != '\\') {
        return Err("value_bad_chars");
    }
    Ok(())
}

/// Known, non-empty slots from keys.env text.
fn file_values(text: &str) -> HashMap<String, String> {
    parse_env(text)
        .into_iter()
        .filter(|(k, v)| slot(k).is_some() && !v.is_empty())
        .collect()
}

/// Parses KEY=VALUE lines. Skips blanks and comments, strips optional quotes.
fn parse_env(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                return None;
            }
            let (k, v) = line.split_once('=')?;
            let v = v.trim().trim_matches(|c| c == '"' || c == '\'');
            Some((k.trim().to_string(), v.to_string()))
        })
        .collect()
}

fn render(values: &HashMap<String, String>) -> String {
    let mut out = String::from("# Agave Studio gateway keys. Local only. Never commit this file.\n");
    for s in SLOTS {
        if let Some(v) = values.get(s.env) {
            out.push_str(&format!("{}={}\n", s.env, v));
        }
    }
    out
}

#[derive(Clone)]
pub struct KeyStore {
    values: Arc<RwLock<HashMap<String, String>>>,
    file: Arc<PathBuf>,
}

impl KeyStore {
    /// Loads known slots from the file, then overlays environment variables (env wins).
    pub fn load(file: PathBuf) -> Self {
        // A keys.env copied from the example is world-readable; tighten it to owner-only before reading.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Ok(meta) = fs::metadata(&file) {
                if meta.permissions().mode() & 0o077 != 0 {
                    if let Err(e) = fs::set_permissions(&file, fs::Permissions::from_mode(0o600)) {
                        eprintln!(
                            "keys: could not make {} owner-only ({e}); other accounts on this computer may be able to read your keys. Run: chmod 600 \"{}\"",
                            file.display(),
                            file.display()
                        );
                    }
                }
            }
        }
        let mut map = fs::read_to_string(&file).map(|t| file_values(&t)).unwrap_or_default();
        for s in SLOTS {
            if let Ok(v) = std::env::var(s.env) {
                let v = v.trim();
                if !v.is_empty() {
                    map.insert(s.env.to_string(), v.to_string());
                }
            }
        }
        KeyStore { values: Arc::new(RwLock::new(map)), file: Arc::new(file) }
    }

    /// Empty store that never reads the file or the environment. Used by tests and by test mode.
    pub fn empty(file: PathBuf) -> Self {
        KeyStore { values: Arc::new(RwLock::new(HashMap::new())), file: Arc::new(file) }
    }

    fn read(&self) -> RwLockReadGuard<'_, HashMap<String, String>> {
        self.values.read().unwrap_or_else(|e| e.into_inner())
    }

    fn write(&self) -> RwLockWriteGuard<'_, HashMap<String, String>> {
        self.values.write().unwrap_or_else(|e| e.into_inner())
    }

    /// Returns the key value for an outgoing provider call. Never serialize or log the result.
    pub fn get(&self, env: &str) -> Option<String> {
        self.read().get(env).cloned()
    }

    pub fn is_set(&self, env: &str) -> bool {
        self.read().contains_key(env)
    }

    /// Stores a key and rewrites config/keys.env (0600). Memory is updated only after the write succeeds.
    /// The file gets only what was already in it plus this key, so keys that came from environment
    /// variables are never copied to disk. The write lock is held throughout so writers cannot race.
    /// Callers validate the slot name and value first.
    pub fn set(&self, env: &str, value: &str) -> io::Result<()> {
        let mut guard = self.write();
        let mut on_disk = match fs::read_to_string(&*self.file) {
            Ok(text) => file_values(&text),
            Err(e) if e.kind() == io::ErrorKind::NotFound => HashMap::new(),
            Err(e) => return Err(e),
        };
        on_disk.insert(env.to_string(), value.to_string());
        fsutil::write_private(&self.file, render(&on_disk).as_bytes())?;
        guard.insert(env.to_string(), value.to_string());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fsutil::test_dir;

    #[test]
    fn known_slot_names_only() {
        assert!(slot("FAL_KEY").is_some());
        assert!(slot("PATH").is_none());
        assert!(slot("fal_key").is_none());
        assert!(slot("").is_none());
    }

    #[test]
    fn value_length_bounds() {
        assert_eq!(validate_value("1234567"), Err("value_length"));
        assert!(validate_value("12345678").is_ok());
        assert!(validate_value(&"a".repeat(512)).is_ok());
        assert_eq!(validate_value(&"a".repeat(513)), Err("value_length"));
    }

    #[test]
    fn value_rejects_newlines_and_whitespace() {
        assert_eq!(validate_value("abcdefgh\nijk"), Err("value_has_whitespace"));
        assert_eq!(validate_value("abcdefgh\r"), Err("value_has_whitespace"));
        assert_eq!(validate_value("abc defgh"), Err("value_has_whitespace"));
        assert_eq!(validate_value("abcdefgh\u{2028}x"), Err("value_has_whitespace"));
    }

    #[test]
    fn value_rejects_quotes_and_non_ascii() {
        assert_eq!(validate_value("\"abcdefgh\""), Err("value_bad_chars"));
        assert_eq!(validate_value("abcdefgh'"), Err("value_bad_chars"));
        assert_eq!(validate_value("abcdéfghij"), Err("value_bad_chars"));
        assert!(validate_value("fal:abc=def/123+_-").is_ok());
    }

    #[test]
    fn set_does_not_persist_other_in_memory_keys() {
        let dir = test_dir("keys_no_env_leak");
        let file = dir.join("keys.env");
        let store = KeyStore::empty(file.clone());
        // Simulates a key that came from an environment variable (memory only).
        store.write().insert("OPENAI_API_KEY".into(), "env-only-secret".into());
        store.set("FAL_KEY", "abc=def:123").expect("set");
        let text = fs::read_to_string(&file).expect("read");
        assert!(!text.contains("env-only-secret"), "{text}");
        assert_eq!(file_values(&text).get("FAL_KEY").map(String::as_str), Some("abc=def:123"));
        assert_eq!(store.get("OPENAI_API_KEY").as_deref(), Some("env-only-secret"));
    }

    #[test]
    fn example_env_lists_every_slot() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../config/keys.example.env");
        let text = fs::read_to_string(path).expect("keys.example.env");
        let names: Vec<String> = parse_env(&text).into_iter().map(|(k, _)| k).collect();
        for s in SLOTS {
            assert!(names.iter().any(|n| n == s.env), "missing {} in keys.example.env", s.env);
        }
        assert_eq!(names.len(), SLOTS.len(), "example file has extra or duplicate slots");
    }

    #[test]
    fn set_writes_private_file_and_round_trips() {
        let dir = test_dir("keys_roundtrip");
        let file = dir.join("keys.env");
        let store = KeyStore::empty(file.clone());
        store.set("FAL_KEY", "test-value-123").expect("set");
        assert!(store.is_set("FAL_KEY"));
        assert_eq!(store.get("FAL_KEY").as_deref(), Some("test-value-123"));

        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(&file).expect("meta").permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);

        let reloaded = KeyStore { values: Arc::new(RwLock::new(HashMap::new())), file: Arc::new(file.clone()) };
        let text = fs::read_to_string(&file).expect("read");
        for (k, v) in parse_env(&text) {
            if slot(&k).is_some() {
                reloaded.write().insert(k, v);
            }
        }
        assert_eq!(reloaded.get("FAL_KEY").as_deref(), Some("test-value-123"));
    }
}
