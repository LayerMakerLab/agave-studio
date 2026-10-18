//! Shared application state and filesystem layout.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use crate::jobs::JobStore;
use crate::keys::KeyStore;
use crate::media::Ctx;
use crate::models::Registry;
use crate::pipelines::PipelineStore;
use crate::promptcraft::{self, PresetCatalog};
use crate::providers::Fake;
use crate::studio::Studio;

pub struct Paths {
    pub web: PathBuf,
    pub keys_file: PathBuf,
    pub jobs_file: PathBuf,
    pub outputs_dir: PathBuf,
    pub models_file: PathBuf,
    pub studio_file: PathBuf,
    pub pipelines_file: PathBuf,
}

impl Paths {
    /// Repo layout relative to this crate: <repo>/gateway (crate), <repo>/web, <repo>/config, <repo>/data.
    /// Test mode keeps everything in <repo>/data/fake-providers, so fake jobs never mix with real ones.
    pub fn new(web: Option<PathBuf>, fake: bool) -> Self {
        let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let repo = crate_dir.parent().map(Path::to_path_buf).unwrap_or_else(|| crate_dir.clone());
        let data = if fake { repo.join("data/fake-providers") } else { repo.join("data") };
        Paths {
            web: web.unwrap_or_else(|| repo.join("web")),
            // Test mode never touches the real key file.
            keys_file: if fake { data.join("keys.env") } else { repo.join("config/keys.env") },
            jobs_file: data.join("jobs.json"),
            outputs_dir: data.join("outputs"),
            models_file: crate_dir.join("models.toml"),
            studio_file: data.join("studio.json"),
            pipelines_file: data.join("pipelines.json"),
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
    /// Library pipelines: runs and their step state (data/pipelines.json).
    pub pipelines: PipelineStore,
    /// Test mode: every model is answered by a local fake provider (no network, no key, no cost). Only set
    /// when the gateway starts with AGAVE_FAKE_PROVIDERS=1, or by unit tests.
    pub fake: Option<Arc<Fake>>,
    /// Host headers and Origins the gateway answers to (its own local addresses only).
    pub allowed_hosts: Arc<Vec<String>>,
    pub allowed_origins: Arc<Vec<String>>,
    pub ctx: Arc<Ctx>,
}

impl AppState {
    pub fn load(web: Option<PathBuf>) -> Result<Self, String> {
        let fake = std::env::var("AGAVE_FAKE_PROVIDERS").is_ok_and(|v| v == "1");
        let paths = Paths::new(web, fake);
        if fake {
            // Test mode never reads the saved keys: nothing it does may reach a provider.
            let keys = KeyStore::empty(paths.keys_file.clone());
            let fail = std::env::var("AGAVE_FAKE_FAIL").unwrap_or_default();
            let mut st = Self::build(paths, keys)?;
            st.fake = Some(Arc::new(Fake::new(st.paths.web.clone(), Duration::from_millis(1500), &fail)));
            return Ok(st);
        }
        let keys = KeyStore::load(paths.keys_file.clone());
        Self::build(paths, keys)
    }

    fn build(paths: Paths, keys: KeyStore) -> Result<Self, String> {
        let registry = Registry::load(&paths.models_file)?;
        let presets = promptcraft::presets()?;
        let jobs = JobStore::load(paths.jobs_file.clone())?;
        let studio = Studio::load(paths.studio_file.clone())?;
        let pipelines = PipelineStore::load(paths.pipelines_file.clone())?;
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
            pipelines,
            fake: None,
            allowed_hosts: Arc::new(hosts),
            allowed_origins: Arc::new(origins),
            ctx: Arc::new(ctx),
            paths: Arc::new(paths),
        })
    }

    /// State under target/test-tmp with an empty key store (never reads the environment).
    #[cfg(test)]
    pub fn for_tests(name: &str) -> Self {
        Self::for_tests_in(crate::fsutil::test_dir(name))
    }

    /// Test state in an existing folder (kept as it is, so a test can "restart" on the same data).
    #[cfg(test)]
    pub fn for_tests_in(dir: PathBuf) -> Self {
        let paths = Paths {
            web: dir.join("web"),
            keys_file: dir.join("keys.env"),
            jobs_file: dir.join("jobs.json"),
            outputs_dir: dir.join("outputs"),
            models_file: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("models.toml"),
            studio_file: dir.join("studio.json"),
            pipelines_file: dir.join("pipelines.json"),
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
