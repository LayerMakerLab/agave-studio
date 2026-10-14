//! First setup: downloading the on-device speech models. Every file is pinned (exact repository
//! revision, size and SHA-256), saved under data/models/.<id>.part first, checked, and only then
//! moved into place, so a half or tampered download is never used. A lost connection resumes
//! from the bytes already on disk.

use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

/// One file of a model: path inside the repository, size in bytes, SHA-256.
type File = (&'static str, u64, &'static str);

pub struct Model {
    pub id: &'static str,
    pub name: &'static str,
    pub engine: &'static str,
    pub languages: &'static str,
    pub source: &'static str,
    /// Base URL of the pinned revision; each file is <base>/<path>.
    base: &'static str,
    /// Where it lives under data/: a folder for many files, a file for one.
    dest: &'static str,
    files: &'static [File],
}

/// NVIDIA Parakeet TDT 0.6B v3 as CoreML (CC-BY-4.0), the files FluidAudio 0.17.7 loads for v3/int8.
const PARAKEET_FILES: &[File] = &[
    ("Decoder.mlmodelc/analytics/coremldata.bin", 243, "4238c4e81ecd0dc94bd7dfbb60f7e2cc824107c1ffe0387b8607b72833dba350"),
    ("Decoder.mlmodelc/coremldata.bin", 554, "18647af085d87bd8f3121c8a9b4d4564c1ede038dab63d295b4e745cf2d7fb99"),
    ("Decoder.mlmodelc/metadata.json", 3427, "a39e93cd8371b8ded92635c7804fcd0590f0d1dd9415c6d19a0484be073077d9"),
    ("Decoder.mlmodelc/model.mil", 13110, "ef2a0a281695398a62fde86ac269c68f73d5b578d7ed3b31f2ba91a2d1ea1f35"),
    ("Decoder.mlmodelc/weights/weight.bin", 23604992, "48adf0f0d47c406c8253d4f7fef967436a39da14f5a65e66d5a4b407be355d41"),
    ("Encoder.mlmodelc/analytics/coremldata.bin", 243, "42e638870d73f26b332918a3496ce36793fbb413a81cbd3d16ba01328637a105"),
    ("Encoder.mlmodelc/coremldata.bin", 485, "d48034a167a82e88fc3df64f60af963ab3983538271175b8319e7d5720a0fb86"),
    ("Encoder.mlmodelc/metadata.json", 2921, "da24da9cca943fb29d7fa8e376d57fca7cb3aa08ca51b956b0b0e56813f087e9"),
    ("Encoder.mlmodelc/model.mil", 959769, "ed7b19156ca29fa7dfd6891deb9fda4b0e8893f68597c985d135736546a43808"),
    ("Encoder.mlmodelc/weights/weight.bin", 445187200, "e2020f323703477a5b21d7c2d282c403e371afb5962e79877e3033e73ba6f421"),
    ("JointDecisionv3.mlmodelc/analytics/coremldata.bin", 243, "26def4bf73dd56d29dee21c8ef97cb8969e62f6120ed1adc91e46828e2737b6c"),
    ("JointDecisionv3.mlmodelc/coremldata.bin", 521, "f5fc08b741400f0088492c9e839418b1e18522f19cba28d361dd030c5f398342"),
    ("JointDecisionv3.mlmodelc/metadata.json", 3453, "d9307211b9a37e0f0ac260c7660b1571a3de25841035cfdf9b58fd40425f890f"),
    ("JointDecisionv3.mlmodelc/model.mil", 11775, "be60732943389a047175111a83f8839f3eb39d4803adafa828a0871b2f39818d"),
    ("JointDecisionv3.mlmodelc/weights/weight.bin", 12642764, "4e0e63d840032f7f07ddb1d64446051166281e5491bf22da8a945c41f6eedb3e"),
    ("Preprocessor.mlmodelc/analytics/coremldata.bin", 243, "c9beeb989c8d66f8be11df59bc6df277ec76cee404f6865b46243835ef562f6d"),
    ("Preprocessor.mlmodelc/coremldata.bin", 486, "dbde3f2300842c1fd51ef3ff948a0bcffe65ffd2dca10707f2509f32c1d65b1d"),
    ("Preprocessor.mlmodelc/metadata.json", 2841, "2a98699e22d279dd37fa1d238aeb1c6db1df0d6fad687775324157689d8f3acf"),
    ("Preprocessor.mlmodelc/model.mil", 28181, "4b8518a956450fec57f06c2a21bdffc26973f7f1fa6842fb38fe917f896b6b93"),
    ("Preprocessor.mlmodelc/weights/weight.bin", 491072, "129b76e3aeafa8afa3ea76d995b964b145fe83700d579f6ff42c4c38fa0968ea"),
    ("config.json", 475, "97f19ecccd0fdc730d76fb918090fa8bc64bce8ea4ad43715d5e9c2c7350db66"),
    ("parakeet_vocab.json", 151122, "7ec60e05f1b24480736ec0eed40900f4626bce1fa9a60fd700ec7e2a59198735"),
];

const WHISPER_FILES: &[File] = &[("ggml-small.bin", 487_601_967, "1be3a9b2063867b937e64e2ec7483364a79917e157fa98c5d94b5c1fffea987b")];

pub const MODELS: &[Model] = &[
    Model {
        id: "parakeet-v3",
        name: "Parakeet v3",
        engine: "parakeet",
        languages: "25 European languages, English included",
        source: "NVIDIA Parakeet TDT 0.6B v3 (CC-BY-4.0), CoreML build by FluidInference",
        base: "https://huggingface.co/FluidInference/parakeet-tdt-0.6b-v3-coreml/resolve/7dd20fe6b1797d35f5e3307e8b1732d9a178edfe",
        dest: "models/parakeet-tdt-0.6b-v3",
        files: PARAKEET_FILES,
    },
    Model {
        id: "whisper-small",
        name: "Whisper small",
        engine: "whisper",
        languages: "99 languages",
        source: "OpenAI Whisper small (MIT) for whisper.cpp",
        base: "https://huggingface.co/ggerganov/whisper.cpp/resolve/5359861c739e955e79d9a303bcbc70fb988958b1",
        dest: "models/ggml-small.bin",
        files: WHISPER_FILES,
    },
];

/// HTTP client for model downloads: follows Hugging Face's redirects (to its own CDN) and nothing
/// else, so a download can never be sent to another host. No keys are ever attached.
pub fn client() -> reqwest::Client {
    let policy = reqwest::redirect::Policy::custom(|a| {
        let ok = a.url().scheme() == "https"
            && a.url().host_str().is_some_and(|h| h == "huggingface.co" || h == "hf.co" || h.ends_with(".huggingface.co") || h.ends_with(".hf.co"));
        if ok && a.previous().len() < 5 {
            a.follow()
        } else {
            a.stop()
        }
    });
    reqwest::Client::builder()
        .redirect(policy)
        .connect_timeout(Duration::from_secs(20))
        .build()
        .expect("the model download client builds")
}

pub fn model(id: &str) -> Option<&'static Model> {
    MODELS.iter().find(|m| m.id == id)
}

impl Model {
    pub fn size(&self) -> u64 {
        self.files.iter().map(|f| f.1).sum()
    }
    pub fn dest(&self, data_dir: &Path) -> PathBuf {
        data_dir.join(self.dest)
    }
    fn part(&self, data_dir: &Path) -> PathBuf {
        data_dir.join("models").join(format!(".{}.part", self.id))
    }
    /// Installed: every file is in place with the right size (hashes were checked before the move).
    pub fn installed(&self, data_dir: &Path) -> bool {
        let dest = self.dest(data_dir);
        if self.files.len() == 1 {
            return std::fs::metadata(&dest).map(|m| m.len() == self.files[0].1).unwrap_or(false);
        }
        self.files.iter().all(|(p, size, _)| std::fs::metadata(dest.join(p)).map(|m| m.len() == *size).unwrap_or(false))
    }
}

#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct Progress {
    /// idle | paused | downloading | checking | preparing | ready | failed
    pub state: &'static str,
    pub done: u64,
    pub total: u64,
    pub error: Option<String>,
}

struct Job {
    progress: Progress,
    cancel: Arc<AtomicBool>,
}

fn jobs() -> &'static Mutex<HashMap<&'static str, Job>> {
    static JOBS: OnceLock<Mutex<HashMap<&'static str, Job>>> = OnceLock::new();
    JOBS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn set(id: &'static str, f: impl FnOnce(&mut Progress)) {
    if let Some(job) = jobs().lock().unwrap().get_mut(id) {
        f(&mut job.progress);
    }
}

pub fn progress(m: &Model, data_dir: &Path) -> Progress {
    progress_locked(m, data_dir, &jobs().lock().unwrap())
}

fn progress_locked(m: &Model, data_dir: &Path, running: &HashMap<&'static str, Job>) -> Progress {
    if let Some(job) = running.get(m.id) {
        if job.progress.state != "ready" {
            return job.progress.clone();
        }
    }
    let total = m.size();
    if m.installed(data_dir) {
        return Progress { state: "ready", done: total, total, error: None };
    }
    // Bytes already in the staging folder from a cancelled or broken download are kept and reused.
    let part = m.part(data_dir);
    let done: u64 = m.files.iter().map(|(p, size, _)| std::fs::metadata(part.join(p)).map(|x| x.len().min(*size)).unwrap_or(0)).sum();
    Progress { state: if done > 0 { "paused" } else { "idle" }, done, total, error: None }
}

/// Bytes free on the disk that holds `dir` (from `df`, which every Mac has).
fn free_bytes(dir: &Path) -> Option<u64> {
    let out = std::process::Command::new("df").arg("-Pk").arg(dir).output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let kb: u64 = text.lines().nth(1)?.split_whitespace().nth(3)?.parse().ok()?;
    Some(kb * 1024)
}

/// Starts the download unless it is already running or done. Returns the progress to show.
pub fn start(m: &'static Model, data_dir: &Path, http: reqwest::Client, warm: Option<Box<dyn FnOnce() -> bool + Send>>) -> Progress {
    // Check and claim under one lock, so two quick presses never start two downloads into the same files.
    let mut running = jobs().lock().unwrap();
    let cur = progress_locked(m, data_dir, &running);
    if matches!(cur.state, "downloading" | "checking" | "preparing" | "ready") {
        return cur;
    }
    let cancel = Arc::new(AtomicBool::new(false));
    let total = m.size();
    let start = Progress { state: "downloading", done: 0, total, error: None };
    running.insert(m.id, Job { progress: start.clone(), cancel: cancel.clone() });
    drop(running);
    let data_dir = data_dir.to_path_buf();
    tokio::spawn(async move {
        let result = download(m, &data_dir, &http, &cancel).await;
        match result {
            Ok(()) => {
                if let Some(warm) = warm {
                    set(m.id, |p| p.state = "preparing");
                    // Loading CoreML models the first time compiles them for this Mac (about half a
                    // minute); doing it now keeps the first caption job fast. A failure here is not fatal.
                    let _ = tokio::task::spawn_blocking(warm).await;
                }
                set(m.id, |p| {
                    p.state = "ready";
                    p.done = p.total;
                });
            }
            Err(e) => set(m.id, |p| {
                p.state = if e == "cancelled" { "idle" } else { "failed" };
                p.error = if e == "cancelled" { None } else { Some(e) };
            }),
        }
    });
    start
}

pub fn cancel(m: &Model) {
    if let Some(job) = jobs().lock().unwrap().get(m.id) {
        job.cancel.store(true, Ordering::Relaxed);
    }
}

fn sha256_file(path: &Path) -> std::io::Result<String> {
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(hex(&h.finalize()))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

async fn download(m: &'static Model, data_dir: &Path, http: &reqwest::Client, cancel: &AtomicBool) -> Result<(), String> {
    let part = m.part(data_dir);
    std::fs::create_dir_all(&part).map_err(|e| e.to_string())?;
    let have: u64 = m.files.iter().map(|(p, size, _)| std::fs::metadata(part.join(p)).map(|x| x.len().min(*size)).unwrap_or(0)).sum();
    let need = m.size().saturating_sub(have);
    if let Some(free) = free_bytes(&part) {
        // Room for the download plus 100 MB to spare.
        if free < need + 100 * 1024 * 1024 {
            return Err(format!("not_enough_space: {} MB free, {} MB needed", free / 1_048_576, need / 1_048_576 + 100));
        }
    }
    set(m.id, |p| p.done = have);
    for (path, size, _) in m.files {
        let file = part.join(path);
        if let Some(dir) = file.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let mut at = std::fs::metadata(&file).map(|x| x.len()).unwrap_or(0);
        if at > *size {
            let _ = std::fs::remove_file(&file);
            at = 0;
        }
        if at < *size {
            fetch(http, &format!("{}/{}", m.base, path), &file, at, *size, m.id, cancel).await?;
        }
    }
    set(m.id, |p| p.state = "checking");
    let files: Vec<(PathBuf, String, &'static str)> = m.files.iter().map(|(p, _, s)| (part.join(p), (*p).to_string(), *s)).collect();
    let bad = tokio::task::spawn_blocking(move || {
        files.into_iter().filter(|(f, _, want)| sha256_file(f).map(|got| got != *want).unwrap_or(true)).map(|(f, p, _)| (f, p)).collect::<Vec<_>>()
    })
    .await
    .map_err(|e| e.to_string())?;
    if !bad.is_empty() {
        // Never keep a file that fails its check; the next try fetches it again.
        for (f, _) in &bad {
            let _ = std::fs::remove_file(f);
        }
        return Err(format!("file_check_failed: {}", bad.iter().map(|b| b.1.as_str()).collect::<Vec<_>>().join(", ")));
    }
    let dest = m.dest(data_dir);
    if m.files.len() == 1 {
        std::fs::rename(part.join(m.files[0].0), &dest).map_err(|e| e.to_string())?;
        let _ = std::fs::remove_dir_all(&part);
    } else {
        if dest.exists() {
            let _ = std::fs::remove_dir_all(&dest);
        }
        std::fs::rename(&part, &dest).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// GETs `url` into `file` from byte `from` on (a Range request when resuming).
async fn fetch(http: &reqwest::Client, url: &str, file: &Path, from: u64, size: u64, id: &'static str, cancel: &AtomicBool) -> Result<(), String> {
    let mut req = http.get(url).timeout(Duration::from_secs(60 * 60));
    if from > 0 {
        req = req.header(reqwest::header::RANGE, format!("bytes={from}-"));
    }
    let mut resp = req.send().await.map_err(|_| "connection_lost".to_string())?;
    let status = resp.status();
    let mut out = if from > 0 && status == reqwest::StatusCode::PARTIAL_CONTENT {
        std::fs::OpenOptions::new().append(true).open(file).map_err(|e| e.to_string())?
    } else if status.is_success() {
        // The server sent the whole file: start this file over.
        set(id, |p| p.done = p.done.saturating_sub(from));
        std::fs::File::create(file).map_err(|e| e.to_string())?
    } else {
        return Err(format!("download_failed: HTTP {}", status.as_u16()));
    };
    let mut written = if status == reqwest::StatusCode::PARTIAL_CONTENT { from } else { 0 };
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err("cancelled".into());
        }
        match resp.chunk().await {
            Ok(Some(bytes)) => {
                written += bytes.len() as u64;
                if written > size {
                    return Err("download_failed: file is larger than expected".into());
                }
                out.write_all(&bytes).map_err(|e| e.to_string())?;
                let n = bytes.len() as u64;
                set(id, |p| p.done += n);
            }
            Ok(None) => break,
            Err(_) => return Err("connection_lost".into()),
        }
    }
    out.flush().map_err(|e| e.to_string())?;
    if written != size {
        return Err("connection_lost".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{extract::State, http::HeaderMap, routing::get, Router};
    use std::sync::atomic::AtomicUsize;

    #[derive(Clone)]
    struct Served {
        files: Arc<HashMap<String, Vec<u8>>>,
        ranged: Arc<AtomicUsize>,
    }

    /// A tiny file server with Range support standing in for Hugging Face.
    async fn serve(files: HashMap<String, Vec<u8>>) -> (String, Arc<AtomicUsize>) {
        async fn file(State(s): State<Served>, axum::extract::Path(p): axum::extract::Path<String>, h: HeaderMap) -> axum::response::Response {
            use axum::response::IntoResponse;
            let Some(body) = s.files.get(&p) else { return axum::http::StatusCode::NOT_FOUND.into_response() };
            if let Some(from) = h.get("range").and_then(|v| v.to_str().ok()).and_then(|v| v.strip_prefix("bytes=")?.strip_suffix('-')?.parse::<usize>().ok()) {
                s.ranged.fetch_add(1, Ordering::Relaxed);
                return (axum::http::StatusCode::PARTIAL_CONTENT, body[from..].to_vec()).into_response();
            }
            body.clone().into_response()
        }
        let ranged = Arc::new(AtomicUsize::new(0));
        let app = Router::new().route("/{*p}", get(file)).with_state(Served { files: Arc::new(files), ranged: ranged.clone() });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (format!("http://{addr}"), ranged)
    }

    fn sha(b: &[u8]) -> &'static str {
        Box::leak(hex(&Sha256::digest(b)).into_boxed_str())
    }

    fn fake(id: &'static str, base: String, dest: &'static str, files: Vec<(&'static str, &[u8])>) -> &'static Model {
        let files: Vec<File> = files.into_iter().map(|(p, b)| (p, b.len() as u64, sha(b))).collect();
        Box::leak(Box::new(Model {
            id,
            name: id,
            engine: "test",
            languages: "",
            source: "",
            base: Box::leak(base.into_boxed_str()),
            dest,
            files: Box::leak(files.into_boxed_slice()),
        }))
    }

    async fn wait(m: &Model, data: &Path) -> Progress {
        for _ in 0..200 {
            let p = progress(m, data);
            if matches!(p.state, "ready" | "failed" | "idle" | "paused") {
                return p;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        progress(m, data)
    }

    fn tmp(name: &str) -> PathBuf {
        let d = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/test-tmp").join(name);
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("models")).unwrap();
        d
    }

    #[tokio::test]
    async fn a_model_folder_is_downloaded_checked_and_moved_into_place() {
        let enc = vec![7u8; 300_000];
        let (base, _) = serve(HashMap::from([("Enc/weights.bin".to_string(), enc.clone()), ("vocab.json".to_string(), b"{}".to_vec())])).await;
        let m = fake("t-folder", base, "models/folder", vec![("Enc/weights.bin", &enc), ("vocab.json", b"{}")]);
        let data = tmp("setup-folder");
        assert_eq!(progress(m, &data).state, "idle");
        start(m, &data, reqwest::Client::new(), None);
        let p = wait(m, &data).await;
        assert_eq!((p.state, p.done, p.total), ("ready", 300_002, 300_002), "{p:?}");
        assert!(m.installed(&data));
        assert_eq!(std::fs::read(data.join("models/folder/Enc/weights.bin")).unwrap(), enc);
        assert!(!data.join("models/.t-folder.part").exists(), "the staging folder is gone");
    }

    #[tokio::test]
    async fn a_file_that_fails_its_check_is_thrown_away_and_never_installed() {
        let good = vec![1u8; 1000];
        let mut bad = good.clone();
        bad[500] = 2;
        let (base, _) = serve(HashMap::from([("model.bin".to_string(), bad)])).await;
        let m = fake("t-bad", base, "models/model.bin", vec![("model.bin", &good)]);
        let data = tmp("setup-bad");
        start(m, &data, reqwest::Client::new(), None);
        let p = wait(m, &data).await;
        assert_eq!(p.state, "failed");
        assert!(p.error.as_deref().unwrap_or("").starts_with("file_check_failed"), "{p:?}");
        assert!(!m.installed(&data) && !data.join("models/model.bin").exists());
        assert!(!data.join("models/.t-bad.part/model.bin").exists(), "the bad file is deleted");
    }

    #[tokio::test]
    async fn a_broken_download_resumes_from_the_bytes_on_disk() {
        let body: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
        let (base, ranged) = serve(HashMap::from([("one.bin".to_string(), body.clone())])).await;
        let m = fake("t-resume", base, "models/one.bin", vec![("one.bin", &body)]);
        let data = tmp("setup-resume");
        let part = data.join("models/.t-resume.part");
        std::fs::create_dir_all(&part).unwrap();
        std::fs::write(part.join("one.bin"), &body[..80_000]).unwrap();
        assert_eq!((progress(m, &data).state, progress(m, &data).done), ("paused", 80_000), "kept bytes show as paused");
        start(m, &data, reqwest::Client::new(), None);
        let p = wait(m, &data).await;
        assert_eq!(p.state, "ready", "{p:?}");
        assert_eq!(ranged.load(Ordering::Relaxed), 1, "asked only for the missing bytes");
        assert_eq!(std::fs::read(data.join("models/one.bin")).unwrap(), body);
    }

    #[tokio::test]
    async fn downloads_follow_hugging_face_redirects_only() {
        // A local server that redirects to an outside host must not be followed.
        async fn hop() -> axum::response::Response {
            use axum::response::IntoResponse;
            (axum::http::StatusCode::FOUND, [("location", "https://example.com/x.bin")]).into_response()
        }
        let app = Router::new().route("/{*p}", get(hop));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let r = client().get(format!("http://{addr}/a")).send().await.unwrap();
        assert_eq!(r.status(), reqwest::StatusCode::FOUND, "not followed off Hugging Face");
    }

    #[test]
    fn the_pinned_models_are_complete_and_sized() {
        let p = model("parakeet-v3").unwrap();
        assert_eq!(p.files.len(), 22);
        assert_eq!(p.size(), 483_106_120);
        assert!(p.base.contains("/resolve/7dd20fe6b1797d35f5e3307e8b1732d9a178edfe"));
        for f in ["Preprocessor.mlmodelc/weights/weight.bin", "Encoder.mlmodelc/weights/weight.bin", "Decoder.mlmodelc/weights/weight.bin", "JointDecisionv3.mlmodelc/weights/weight.bin", "parakeet_vocab.json"] {
            assert!(p.files.iter().any(|x| x.0 == f), "{f}");
        }
        assert!(p.files.iter().all(|x| x.2.len() == 64 && x.2.chars().all(|c| c.is_ascii_hexdigit())));
        let w = model("whisper-small").unwrap();
        assert_eq!((w.size(), w.files[0].2.get(..8)), (487_601_967, Some("1be3a9b2")));
        assert!(model("nope").is_none());
    }
}
