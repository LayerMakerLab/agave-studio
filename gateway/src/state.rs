//! Shared application state and filesystem layout.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use crate::jobs::JobStore;
use crate::keys::KeyStore;
use crate::media::Ctx;
use crate::models::Registry;
use crate::promptcraft::{self, PresetCatalog};
use crate::studio::Studio;

pub struct Paths {
    pub web: PathBuf,
    pub keys_file: PathBuf,
    pub jobs_file: PathBuf,
    pub outputs_dir: PathBuf,
    pub models_file: PathBuf,
    pub studio_file: PathBuf,
}

impl Paths {
    /// Repo layout relative to this crate: <repo>/gateway (crate), <repo>/web, <repo>/config, <repo>/data.
    pub fn new(web: Option<PathBuf>) -> Self {
        let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let repo = crate_dir.parent().map(Path::to_path_buf).unwrap_or_else(|| crate_dir.clone());
        Paths {
            web: web.unwrap_or_else(|| repo.join("web")),
            keys_file: repo.join("config/keys.env"),
            jobs_file: repo.join("data/jobs.json"),
            outputs_dir: repo.join("data/outputs"),
            models_file: crate_dir.join("models.toml"),
            studio_file: repo.join("data/studio.json"),
        }
    }
}

#[derive(Clone)]
pub struct AppState {
    pub paths: Arc<Paths>,
    pub registry: Arc<Registry>,
    pub presets: Arc<PresetCatalog>,
    pub keys: KeyStore,
    pub jobs: JobStore,
    /// Self-host Elements and project settings.
    pub studio: Studio,
    /// Host headers and Origins the gateway answers to (its own local addresses only).
    pub allowed_hosts: Arc<Vec<String>>,
    pub allowed_origins: Arc<Vec<String>>,
    pub ctx: Arc<Ctx>,
}

impl AppState {
    pub fn load(web: Option<PathBuf>) -> Result<Self, String> {
        let paths = Paths::new(web);
        let keys = KeyStore::load(paths.keys_file.clone());
        Self::build(paths, keys)
    }

    fn build(paths: Paths, keys: KeyStore) -> Result<Self, String> {
        let registry = Registry::load(&paths.models_file)?;
        let presets = promptcraft::presets()?;
        let jobs = JobStore::load(paths.jobs_file.clone())?;
        let studio = Studio::load(paths.studio_file.clone())?;
        let http = reqwest::Client::builder()
            .user_agent("agave-studio/0.1")
            // Never auto-follow redirects: key headers (x-key, xi-api-key, x-goog-api-key) are not
            // stripped by reqwest on cross-host redirects. media::fetch follows them by hand.
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(300))
            .build()
            .map_err(|e| format!("http client: {e}"))?;
        let ctx = Ctx { http, outputs_dir: paths.outputs_dir.clone() };
        let hosts = vec!["127.0.0.1:4170".to_string(), "localhost:4170".to_string()];
        let origins = vec!["http://127.0.0.1:4170".to_string(), "http://localhost:4170".to_string()];
        Ok(AppState {
            registry: Arc::new(registry),
            presets: Arc::new(presets),
            keys,
            jobs,
            studio,
            allowed_hosts: Arc::new(hosts),
            allowed_origins: Arc::new(origins),
            ctx: Arc::new(ctx),
            paths: Arc::new(paths),
        })
    }

    /// State under target/test-tmp with an empty key store (never reads the environment).
    #[cfg(test)]
    pub fn for_tests(name: &str) -> Self {
        let dir = crate::fsutil::test_dir(name);
        let paths = Paths {
            web: dir.join("web"),
            keys_file: dir.join("keys.env"),
            jobs_file: dir.join("jobs.json"),
            outputs_dir: dir.join("outputs"),
            models_file: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("models.toml"),
            studio_file: dir.join("studio.json"),
        };
        let keys = KeyStore::empty(paths.keys_file.clone());
        let mut st = Self::build(paths, keys).expect("test state");
        // A model on the always-failing test provider, so route tests never call a real provider.
        let mut reg = Registry::load(&st.paths.models_file).expect("registry");
        reg.models.push(crate::models::ModelDef {
            id: "test/offline".into(),
            name: "Test".into(),
            vendor: "Test".into(),
            provider: "test".into(),
            endpoint: "test".into(),
            modality: "video".into(),
            modes: vec!["t2v".into()],
            key: "FAL_KEY".into(),
            verified: false,
            params: crate::models::Params { aspect: vec![], duration: vec![5], resolution: vec![] },
            price: None,
            max_refs: 0,
            provider_credits: None,
        });
        // A model whose provider has no adapter, for the 501 path.
        let mut none = reg.models.last().expect("test model").clone();
        none.id = "test/no-adapter".into();
        none.provider = "nobody".into();
        none.modes = vec!["i2v".into()];
        reg.models.push(none);
        st.registry = Arc::new(reg);
        st
    }
}
