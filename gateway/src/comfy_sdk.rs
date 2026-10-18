//! COMFY_CLIENT=sdk: the Comfy adapter's calls go through the official Comfy SDK (comfy-sdk 0.5.0) in a small
//! Python sidecar, tools/comfy-sdk/agave_comfy.py, started with uv. It sits behind the same `ComfyTransport` seam as
//! the built-in HTTP client, so submit, poll, output classification, timeouts and size caps stay shared
//! (providers.rs). This file only turns those calls into sidecar commands:
//!   Upload   -> the picture is written to data/comfy-sdk and handed to `submit --image` (the SDK uploads it itself)
//!   Json     -> `submit --workflow <file> [--image-node <id> --image <file>]`  -> {"job_id"}
//!   Get      -> `status <job_id>`  -> {"status", "outputs": [{name, content_type, node_id, type}], "error"}
//!   Download -> `fetch <job_id> --name <name> --out <file> --max-bytes <n>`
//!
//! The sidecar is started with an argument array (never a shell), a cleared environment (PATH, HOME and the
//! COMFY_BASE_URL the gateway already validated; no key), stdin closed, a timeout, and a cap on what it may print.
//! The gateway picks every file path, all under data/comfy-sdk, and checks the sidecar's answers the way it checks
//! the proxy's (job ids, output names, content types).

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Mutex;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::io::AsyncReadExt;

use crate::fsutil;
use crate::keys::KeyStore;
use crate::media;
use crate::providers::{comfy_id, comfy_output_ok, ComfyCall, ComfyTransport, PResult, ProviderError};

/// What the gateway needs to start the sidecar: the setting (read on every call, so Settings changes apply at
/// once), its work folder and the script.
pub struct SdkSetup {
    pub keys: KeyStore,
    pub dir: PathBuf,
    pub script: PathBuf,
}

impl SdkSetup {
    /// True when COMFY_CLIENT says "sdk" (the default is the built-in client).
    pub fn active(&self) -> bool {
        self.keys.get_or_default("COMFY_CLIENT").as_deref() == Some("sdk")
    }
}

const STATUSES: &[&str] = &["queued", "running", "succeeded", "canceling", "canceled", "failed", "expired"];

pub struct SdkTransport {
    program: PathBuf,
    prefix: Vec<OsString>,
    dir: PathBuf,
    path_env: OsString,
    home: Option<OsString>,
    pub(crate) submit_timeout: Duration,
    pub(crate) status_timeout: Duration,
    pub(crate) fetch_timeout: Duration,
    pub(crate) max_json: usize,
    pub(crate) max_download: usize,
    /// Pictures written for this call: the local id the workflow carries -> the file.
    uploads: Mutex<HashMap<String, PathBuf>>,
}

fn failed(msg: impl Into<String>) -> ProviderError {
    ProviderError::Failed(msg.into())
}

fn missing_uv() -> ProviderError {
    failed("COMFY_CLIENT is set to sdk, but uv was not found. Install uv (https://docs.astral.sh/uv/) or set COMFY_CLIENT to builtin in Settings.")
}

fn excerpt(b: &[u8]) -> String {
    String::from_utf8_lossy(b).trim().chars().take(300).map(|c| if c.is_control() { ' ' } else { c }).collect()
}

/// uv on PATH, or in the places its installers put it (a gateway started outside a shell may have a short PATH).
fn find_uv() -> Option<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
    if let Some(home) = std::env::var_os("HOME") {
        let home = PathBuf::from(home);
        dirs.push(home.join(".local/bin"));
        dirs.push(home.join(".cargo/bin"));
    }
    dirs.push(PathBuf::from("/opt/homebrew/bin"));
    dirs.push(PathBuf::from("/usr/local/bin"));
    dirs.into_iter().map(|d| d.join("uv")).find(|p| p.is_file())
}

impl SdkTransport {
    /// The real sidecar: `uv run --quiet --script tools/comfy-sdk/agave_comfy.py ...`.
    pub fn for_setup(setup: &SdkSetup) -> PResult<Self> {
        let uv = find_uv().ok_or_else(missing_uv)?;
        let prefix = vec!["run".into(), "--quiet".into(), "--script".into(), setup.script.clone().into_os_string()];
        Self::new(uv, prefix, &setup.dir)
    }

    /// Any program and leading arguments (tests use a stub script).
    pub fn new(program: PathBuf, prefix: Vec<OsString>, dir: &Path) -> PResult<Self> {
        fsutil::ensure_private_dir(dir).map_err(|_| failed("could not create data/comfy-sdk"))?;
        let mut paths: Vec<PathBuf> = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
        if let Some(d) = program.parent() {
            paths.insert(0, d.to_path_buf());
        }
        let path_env = std::env::join_paths(paths).unwrap_or_default();
        Ok(SdkTransport {
            program,
            prefix,
            dir: dir.to_path_buf(),
            path_env,
            home: std::env::var_os("HOME"),
            submit_timeout: Duration::from_secs(300),
            status_timeout: Duration::from_secs(60),
            fetch_timeout: Duration::from_secs(600),
            max_json: 1 << 20,
            max_download: media::MAX_DOWNLOAD_BYTES,
            uploads: Mutex::new(HashMap::new()),
        })
    }

    fn file(&self, prefix: &str, ext: &str) -> PathBuf {
        self.dir.join(format!("{prefix}-{}.{ext}", media::random_hex(8)))
    }

    /// Runs one sidecar command and returns its JSON object.
    async fn run(&self, origin: &str, args: Vec<OsString>, timeout: Duration) -> PResult<Value> {
        let mut cmd = tokio::process::Command::new(&self.program);
        cmd.args(&self.prefix)
            .args(&args)
            .env_clear()
            .env("PATH", &self.path_env)
            .env("COMFY_BASE_URL", origin)
            .env("NO_COLOR", "1")
            .current_dir(&self.dir)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        if let Some(h) = &self.home {
            cmd.env("HOME", h);
        }
        let mut child = cmd.spawn().map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                missing_uv()
            } else {
                failed(format!("could not start the Comfy SDK client: {e}"))
            }
        })?;
        let (Some(out), Some(err)) = (child.stdout.take(), child.stderr.take()) else {
            return Err(failed("could not read the Comfy SDK client"));
        };
        let max = self.max_json;
        let work = async {
            // The reader is dropped after the cap, so a client that keeps printing stops on a closed pipe.
            let read_out = async move {
                let mut buf = Vec::new();
                let _ = out.take(max as u64 + 1).read_to_end(&mut buf).await;
                buf
            };
            let read_err = async move {
                let mut buf = Vec::new();
                let _ = err.take(16 << 10).read_to_end(&mut buf).await;
                buf
            };
            let (o, e) = tokio::join!(read_out, read_err);
            (o, e, child.wait().await)
        };
        let done = tokio::time::timeout(timeout, work).await;
        let Ok((stdout, stderr, status)) = done else {
            let _ = child.start_kill();
            let _ = child.wait().await;
            return Err(failed(format!("the Comfy SDK client took longer than {} s and was stopped", timeout.as_secs())));
        };
        if stdout.len() > max {
            return Err(failed("the Comfy SDK client printed more than the studio accepts"));
        }
        match status {
            Ok(s) if s.success() => {}
            _ => {
                let why = excerpt(&stderr);
                return Err(failed(if why.is_empty() { "the Comfy SDK client failed".to_string() } else { format!("Comfy SDK client: {why}") }));
            }
        }
        match serde_json::from_slice::<Value>(&stdout) {
            Ok(v) if v.is_object() => Ok(v),
            _ => Err(failed("the Comfy SDK client returned non-JSON")),
        }
    }

    fn take_uploads(&self) -> Vec<PathBuf> {
        self.uploads.lock().unwrap_or_else(|e| e.into_inner()).drain().map(|(_, p)| p).collect()
    }

    async fn submit(&self, origin: &str, body: &Value) -> PResult<Value> {
        let wf = body.get("workflow").filter(|w| w.is_object()).ok_or_else(|| failed("no workflow to send"))?;
        // The LoadImage node that carries a picture written by Upload: the SDK uploads that file itself.
        let mut image: Option<(String, PathBuf)> = None;
        for (node, v) in wf.as_object().into_iter().flatten() {
            let Some(id) = v.pointer("/inputs/image/info/id").and_then(Value::as_str) else { continue };
            if v.pointer("/inputs/image/__type").and_then(Value::as_str) != Some("core/ASSET") {
                continue;
            }
            let file = self.uploads.lock().unwrap_or_else(|e| e.into_inner()).get(id).cloned();
            let (Some(file), true) = (file, node.len() <= 32 && node.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')) else {
                return Err(failed("the workflow names a picture the studio did not prepare"));
            };
            image = Some((node.clone(), file));
        }
        let wf_file = self.file("wf", "json");
        fsutil::write_private(&wf_file, &serde_json::to_vec(wf).unwrap_or_default()).map_err(|_| failed("could not write the workflow"))?;
        let mut args: Vec<OsString> = vec!["submit".into(), "--workflow".into(), wf_file.clone().into()];
        if let Some((node, file)) = &image {
            args.extend(["--image-node".into(), node.into(), "--image".into(), file.clone().into()]);
        }
        let res = self.run(origin, args, self.submit_timeout).await;
        let _ = std::fs::remove_file(&wf_file);
        for f in self.take_uploads() {
            let _ = std::fs::remove_file(f);
        }
        let v = res?;
        let id = v.get("job_id").and_then(Value::as_str).filter(|id| comfy_id(id)).ok_or_else(|| failed("the Comfy SDK client returned an odd job id"))?;
        Ok(json!({"id": id, "status": "queued", "urls": {"self": format!("/api/v2/jobs/{id}")}}))
    }

    async fn status(&self, origin: &str, id: &str) -> PResult<Value> {
        let v = self.run(origin, vec!["status".into(), id.into()], self.status_timeout).await?;
        let status = v.get("status").and_then(Value::as_str).filter(|s| STATUSES.contains(s)).ok_or_else(|| failed("the Comfy SDK client returned an unknown job status"))?;
        let raw = v.get("outputs").and_then(Value::as_array).cloned().unwrap_or_default();
        if raw.len() > 64 {
            return Err(failed("the Comfy SDK client returned too many outputs"));
        }
        let mut outputs = Vec::new();
        for o in &raw {
            // The same checks as a proxy answer: a safe name and a plain content type, or the output is left out.
            if !comfy_output_ok(o) {
                continue;
            }
            let name = o.get("name").and_then(Value::as_str).unwrap_or_default();
            let mut link = reqwest::Url::parse("http://x/").map_err(|_| failed("bad link"))?.join(&format!("/api/v2/jobs/{id}/sdk-output")).map_err(|_| failed("bad link"))?;
            link.query_pairs_mut().append_pair("name", name);
            let rel = format!("{}?{}", link.path(), link.query().unwrap_or(""));
            let mut out = json!({"name": name, "url": rel});
            for k in ["content_type", "type", "node_id"] {
                if let Some(s) = o.get(k).and_then(Value::as_str).filter(|s| s.len() <= 100) {
                    out[k] = json!(s);
                }
            }
            outputs.push(out);
        }
        let mut job = json!({"id": id, "status": status, "outputs": outputs});
        if let Some(e) = v.get("error").and_then(Value::as_str) {
            job["error"] = json!({"message": e.chars().take(300).collect::<String>()});
        }
        Ok(job)
    }

    async fn fetch(&self, origin: &str, id: &str, name: &str) -> PResult<Vec<u8>> {
        let dest = self.file("dl", "bin");
        let args: Vec<OsString> = vec![
            "fetch".into(),
            id.into(),
            "--name".into(),
            name.into(),
            "--out".into(),
            dest.clone().into(),
            "--max-bytes".into(),
            self.max_download.to_string().into(),
        ];
        let res = self.run(origin, args, self.fetch_timeout).await;
        let bytes = res.and_then(|_| {
            let len = std::fs::metadata(&dest).map_err(|_| failed("the Comfy SDK client saved no file"))?.len();
            if len > self.max_download as u64 {
                return Err(failed("the ComfyUI output is larger than the studio accepts"));
            }
            std::fs::read(&dest).map_err(|_| failed("could not read the downloaded output"))
        });
        let _ = std::fs::remove_file(&dest);
        bytes
    }
}

/// The ComfyUI origin a call goes to (already checked by comfy_base/on_base) and the parsed URL.
fn split(url: &str) -> PResult<(String, reqwest::Url)> {
    let u = reqwest::Url::parse(url).map_err(|_| failed("bad ComfyUI link"))?;
    Ok((u.origin().ascii_serialization(), u))
}

/// The job id in /api/v2/jobs/<id>[/sdk-output].
fn job_in(u: &reqwest::Url, tail: Option<&str>) -> PResult<String> {
    let parts: Vec<&str> = u.path().trim_start_matches('/').split('/').collect();
    let ok = match (parts.as_slice(), tail) {
        (["api", "v2", "jobs", id], None) => Some(*id),
        (["api", "v2", "jobs", id, t], Some(want)) if *t == want => Some(*id),
        _ => None,
    };
    ok.filter(|id| comfy_id(id)).map(String::from).ok_or_else(|| failed("not a ComfyUI job link"))
}

#[async_trait]
impl ComfyTransport for SdkTransport {
    async fn send(&self, _http: &reqwest::Client, call: ComfyCall<'_>) -> PResult<(u16, Vec<u8>, Option<String>)> {
        let ok = |v: Value| Ok((200, v.to_string().into_bytes(), Some("application/json".to_string())));
        match call {
            ComfyCall::Upload { bytes, file_path, .. } => {
                if !media::valid_output_name(file_path) || bytes.len() > self.max_download {
                    return Err(failed("bad picture for ComfyUI"));
                }
                let ext = file_path.rsplit('.').next().unwrap_or("png");
                let file = self.file("up", ext);
                fsutil::write_private(&file, &bytes).map_err(|_| failed("could not write the picture"))?;
                let id = format!("agave-local-{}", media::random_hex(8));
                self.uploads.lock().unwrap_or_else(|e| e.into_inner()).insert(id.clone(), file);
                Ok((201, json!({"id": id}).to_string().into_bytes(), Some("application/json".into())))
            }
            ComfyCall::Json { url, body, .. } => {
                let (origin, u) = split(url)?;
                if u.path() != "/api/v2/jobs" {
                    return Err(failed("the Comfy SDK client only submits jobs"));
                }
                let job = self.submit(&origin, body).await?;
                Ok((201, job.to_string().into_bytes(), Some("application/json".into())))
            }
            ComfyCall::Get(url) => {
                let (origin, u) = split(url)?;
                let id = job_in(&u, None)?;
                ok(self.status(&origin, &id).await?)
            }
            ComfyCall::Download(url) => {
                let (origin, u) = split(url)?;
                let id = job_in(&u, Some("sdk-output"))?;
                let name = u.query_pairs().find(|(k, _)| k == "name").map(|(_, v)| v.into_owned()).unwrap_or_default();
                if !comfy_output_ok(&json!({"name": name})) {
                    return Err(failed("refusing an odd ComfyUI output name"));
                }
                Ok((200, self.fetch(&origin, &id, &name).await?, None))
            }
        }
    }
}
