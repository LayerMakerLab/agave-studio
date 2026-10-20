//! Job store and the background poller (every 3 s). Finished jobs are kept in data/jobs.json, so the
//! gallery survives a restart. Running jobs are also written to data/active-jobs.json with their provider
//! handle (never a key), so a job that is already paid for is picked up again after a restart.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::media::Output;
use crate::providers::{Handle, Polled, Provider, ProviderError, SubmitReq, Submitted};
use crate::state::AppState;

const MAX_JOBS: usize = 500;
/// Finished jobs kept on disk; the oldest go first.
const MAX_SAVED_JOBS: usize = 5_000;
/// Queued + running jobs. Each one may be spending money, so new submits are refused past this.
const MAX_ACTIVE_JOBS: usize = 50;
const MAX_JOB_AGE: Duration = Duration::from_secs(2 * 60 * 60);
const MAX_TRANSIENT_FAILURES: u32 = 5;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Job {
    pub id: String,
    pub model: String,
    pub mode: String,
    pub prompt: String,
    pub status: String,
    pub progress: Option<f64>,
    pub outputs: Vec<Output>,
    pub error: Option<String>,
    pub created_at: u64,
    /// The request as the client sent it, so a failed job can be run again (absent when large).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request: Option<serde_json::Value>,
}

struct Entry {
    job: Job,
    handle: Option<Handle>,
    adapter: Arc<dyn Provider>,
    key_env: String,
    started: Instant,
    last_poll: Option<Instant>,
    failures: u32,
    /// Already copied into the saved list.
    saved: bool,
}

/// A finished job as kept in data/jobs.json.
#[derive(Serialize, Deserialize, Clone)]
struct Saved {
    job: Job,
}

#[derive(Serialize, Deserialize, Default)]
struct JobsFile {
    jobs: Vec<Saved>,
}

/// A queued or running job as kept in data/active-jobs.json.
#[derive(Serialize, Deserialize, Clone)]
struct Active {
    job: Job,
    handle: Option<Handle>,
    key_env: String,
}

#[derive(Serialize, Deserialize, Default)]
struct ActiveFile {
    jobs: Vec<Active>,
}

/// Reads a JSON state file. A missing file is empty; a file that does not parse is moved aside to
/// `<name>.bad` (and reported) so one damaged file never stops the studio from starting.
pub fn read_state<T: serde::de::DeserializeOwned + Default>(path: &std::path::Path) -> Result<T, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => match serde_json::from_str::<T>(&text) {
            Ok(v) => Ok(v),
            Err(e) => {
                let bad = path.with_extension("json.bad");
                eprintln!("{} could not be read ({e}); moved to {} and starting empty", path.display(), bad.display());
                std::fs::rename(path, &bad).map_err(|e| format!("move {}: {e}", path.display()))?;
                Ok(T::default())
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(T::default()),
        Err(e) => Err(format!("read {}: {e}", path.display())),
    }
}

/// One job due for a poll this tick.
pub struct Due {
    pub id: String,
    pub adapter: Arc<dyn Provider>,
    pub handle: Handle,
    pub key_env: String,
}

#[derive(Clone, Default)]
pub struct JobStore {
    inner: Arc<RwLock<HashMap<String, Entry>>>,
    /// Finished jobs from data/jobs.json (none kept when there is no file, as in unit tests).
    saved: Arc<std::sync::Mutex<Vec<Saved>>>,
    path: Option<Arc<PathBuf>>,
    /// Active jobs read at startup, waiting for `restore` (which needs the model registry).
    pending: Arc<std::sync::Mutex<Vec<Active>>>,
    /// The last active-jobs.json written. Held while writing, so writes land in order and an
    /// unchanged list is never written again.
    active_written: Arc<std::sync::Mutex<Vec<u8>>>,
}

fn active_path(jobs_file: &std::path::Path) -> PathBuf {
    jobs_file.with_file_name("active-jobs.json")
}

fn is_active(job: &Job) -> bool {
    job.status == "queued" || job.status == "running"
}

/// Removes the key from a provider-derived message. Provider error bodies can echo request headers, and
/// providers.rs cuts error bodies to a short excerpt first, so a cut can leave the start of the key behind:
/// any piece of 8 or more characters from the start of the key is removed as well.
fn scrub(message: String, key: &str) -> String {
    const MIN_PIECE: usize = 8;
    if key.is_empty() {
        return message;
    }
    let mut out = message.replace(key, "[redacted]");
    let cuts: Vec<usize> = key.char_indices().map(|(i, _)| i).filter(|&i| i >= MIN_PIECE).collect();
    for &end in cuts.iter().rev() {
        let piece = &key[..end];
        if out.contains(piece) {
            out = out.replace(piece, "[redacted]");
        }
    }
    out
}

pub fn now_unix() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// The one place a job becomes "failed".
fn fail_entry(e: &mut Entry, message: String) {
    e.job.status = "failed".into();
    e.job.error = Some(message);
    e.handle = None;
}

impl JobStore {
    /// Loads the finished jobs saved by earlier runs.
    pub fn load(path: PathBuf) -> Result<Self, String> {
        let saved = read_state::<JobsFile>(&path)?.jobs;
        let pending = read_state::<ActiveFile>(&active_path(&path))?.jobs;
        Ok(JobStore {
            saved: Arc::new(std::sync::Mutex::new(saved)),
            path: Some(Arc::new(path)),
            pending: Arc::new(std::sync::Mutex::new(pending)),
            ..Default::default()
        })
    }

    /// Picks up the jobs that were running when the studio last stopped. A job with a provider handle
    /// is polled again (the provider has it and it may be paid for); one that never reached the provider
    /// is shown as failed so it can be run again.
    pub fn restore(&self, registry: &crate::models::Registry) {
        let pending = std::mem::take(&mut *self.pending.lock().unwrap_or_else(|e| e.into_inner()));
        if pending.is_empty() {
            return;
        }
        let now = Instant::now();
        let age = |job: &Job| Duration::from_secs(now_unix().saturating_sub(job.created_at));
        self.with(|m| {
            for a in pending {
                // Playground jobs (fal:<endpoint>) are not in models.toml; their adapter comes from the id.
                let adapter = registry
                    .get(&a.job.model)
                    .and_then(crate::providers::adapter_for)
                    .or_else(|| crate::playground::adapter_for_job(&a.job.model));
                let too_old = age(&a.job) > MAX_JOB_AGE;
                let started = now.checked_sub(age(&a.job)).unwrap_or(now);
                let resumable = adapter.is_some() && a.handle.is_some() && !too_old;
                let (Some(adapter), Some(handle), true) = (adapter, a.handle, resumable) else {
                    let mut job = a.job;
                    job.status = "failed".into();
                    job.error = Some(if too_old { "timed out".into() } else { "The studio closed before this job finished. Run it again.".into() });
                    let adapter: Arc<dyn Provider> = Arc::new(crate::providers::Stopped);
                    m.insert(job.id.clone(), Entry { job, handle: None, adapter, key_env: a.key_env, started, last_poll: None, failures: 0, saved: false });
                    continue;
                };
                let mut job = a.job;
                job.status = "running".into();
                m.insert(job.id.clone(), Entry { job, handle: Some(handle), adapter, key_env: a.key_env, started, last_poll: None, failures: 0, saved: false });
            }
        });
        self.save_finished();
    }

    /// Writes the queued and running jobs (with their provider handles, never keys) to active-jobs.json.
    fn save_active(&self) {
        let Some(path) = &self.path else { return };
        let mut last = self.active_written.lock().unwrap_or_else(|e| e.into_inner());
        let mut jobs: Vec<Active> = self.with(|m| {
            m.values()
                .filter(|e| is_active(&e.job))
                .map(|e| {
                    // Progress changes on every poll and is not needed to resume, so it is left out.
                    let job = Job { progress: None, ..e.job.clone() };
                    Active { job, handle: e.handle.clone(), key_env: e.key_env.clone() }
                })
                .collect()
        });
        jobs.sort_by(|a, b| a.job.id.cmp(&b.job.id));
        match serde_json::to_vec(&ActiveFile { jobs }) {
            Ok(bytes) if bytes == *last => {}
            Ok(bytes) => {
                let path = active_path(path);
                match crate::fsutil::write_private(&path, &bytes) {
                    Ok(()) => *last = bytes,
                    Err(e) => eprintln!("jobs: write {}: {e}", path.display()),
                }
            }
            Err(e) => eprintln!("jobs: encode: {e}"),
        }
    }

    /// Copies newly finished jobs into the saved list and writes it. Best effort: a failed write
    /// only means those jobs drop out of the gallery after the next restart.
    fn save_finished(&self) {
        let Some(path) = &self.path else { return };
        self.save_active();
        let fresh: Vec<Saved> = self.with(|m| {
            m.values_mut()
                .filter(|e| !e.saved && !is_active(&e.job))
                .map(|e| {
                    e.saved = true;
                    Saved { job: e.job.clone() }
                })
                .collect()
        });
        if fresh.is_empty() {
            return;
        }
        let mut saved = self.saved.lock().unwrap_or_else(|e| e.into_inner());
        for s in fresh {
            saved.retain(|x| x.job.id != s.job.id);
            saved.push(s);
        }
        if saved.len() > MAX_SAVED_JOBS {
            saved.sort_by_key(|s| s.job.created_at);
            let extra = saved.len() - MAX_SAVED_JOBS;
            saved.drain(..extra);
        }
        let file = JobsFile { jobs: saved.clone() };
        match serde_json::to_vec(&file) {
            Ok(bytes) => {
                if let Err(e) = crate::fsutil::write_private(path, &bytes) {
                    eprintln!("jobs: write {}: {e}", path.display());
                }
            }
            Err(e) => eprintln!("jobs: encode: {e}"),
        }
    }

    fn with<R>(&self, f: impl FnOnce(&mut HashMap<String, Entry>) -> R) -> R {
        let mut guard = self.inner.write().unwrap_or_else(|e| e.into_inner());
        f(&mut guard)
    }

    /// Drops the oldest finished job when the store is full.
    fn prune(map: &mut HashMap<String, Entry>) {
        while map.len() >= MAX_JOBS {
            let oldest = map
                .iter()
                .filter(|(_, e)| e.job.status == "done" || e.job.status == "failed")
                .min_by_key(|(_, e)| e.job.created_at)
                .map(|(id, _)| id.clone());
            match oldest {
                Some(id) => {
                    map.remove(&id);
                }
                None => break,
            }
        }
    }

    /// Returns false (and stores nothing) when MAX_ACTIVE_JOBS are already queued or running.
    pub fn insert(&self, job: Job, adapter: Arc<dyn Provider>, key_env: String) -> bool {
        let inserted = self.with(|m| {
            Self::prune(m);
            if m.values().filter(|e| is_active(&e.job)).count() >= MAX_ACTIVE_JOBS {
                return false;
            }
            let id = job.id.clone();
            m.insert(
                id,
                Entry {
                    job,
                    handle: None,
                    adapter,
                    key_env,
                    started: Instant::now(),
                    last_poll: None,
                    failures: 0,
                    saved: false,
                },
            );
            true
        });
        if inserted {
            self.save_active();
        }
        inserted
    }

    /// Running and finished jobs, newest first.
    pub fn list(&self) -> Vec<Job> {
        let guard = self.inner.read().unwrap_or_else(|e| e.into_inner());
        let mut jobs: Vec<Job> = guard.values().map(|e| e.job.clone()).collect();
        let saved = self.saved.lock().unwrap_or_else(|e| e.into_inner());
        jobs.extend(saved.iter().filter(|s| !guard.contains_key(&s.job.id)).map(|s| s.job.clone()));
        jobs.sort_by(|a, b| b.created_at.cmp(&a.created_at).then_with(|| a.id.cmp(&b.id)));
        jobs
    }

    pub fn get(&self, id: &str) -> Option<Job> {
        let guard = self.inner.read().unwrap_or_else(|e| e.into_inner());
        if let Some(e) = guard.get(id) {
            return Some(e.job.clone());
        }
        let saved = self.saved.lock().unwrap_or_else(|e| e.into_inner());
        saved.iter().find(|s| s.job.id == id).map(|s| s.job.clone())
    }

    pub fn set_handle(&self, id: &str, handle: Handle) {
        self.with(|m| {
            if let Some(e) = m.get_mut(id) {
                e.handle = Some(handle);
                e.job.status = "running".into();
            }
        });
        self.save_active();
    }

    pub fn set_progress(&self, id: &str, progress: Option<f64>) {
        self.with(|m| {
            if let Some(e) = m.get_mut(id).filter(|e| is_active(&e.job)) {
                e.job.status = "running".into();
                e.job.progress = progress;
                e.failures = 0;
            }
        });
    }

    pub fn set_done(&self, id: &str, outputs: Vec<Output>) {
        self.with(|m| {
            if let Some(e) = m.get_mut(id).filter(|e| is_active(&e.job)) {
                e.job.status = "done".into();
                e.job.progress = Some(1.0);
                e.job.outputs = outputs;
                e.handle = None;
            }
        });
        self.save_finished();
    }

    pub fn set_failed(&self, id: &str, message: impl Into<String>) {
        let message = message.into();
        self.with(|m| {
            if let Some(e) = m.get_mut(id).filter(|e| is_active(&e.job)) {
                fail_entry(e, message);
            }
        });
        self.save_finished();
    }

    /// Counts a transient poll error. The job fails after MAX_TRANSIENT_FAILURES in a row.
    fn record_transient(&self, id: &str, message: String) {
        let give_up = self.with(|m| match m.get_mut(id) {
            Some(e) => {
                e.failures += 1;
                e.failures >= MAX_TRANSIENT_FAILURES
            }
            None => false,
        });
        if give_up {
            self.set_failed(id, format!("gave up after repeated errors: {message}"));
        }
    }

    /// Jobs whose poll interval has elapsed. Also fails jobs past the age cap, except ones whose poll
    /// (and maybe download) is still running: `busy`.
    fn due(&self, busy: &HashSet<String>) -> Vec<Due> {
        let out = self.with(|m| {
            let now = Instant::now();
            let mut out = Vec::new();
            for (id, e) in m.iter_mut() {
                if (e.job.status != "queued" && e.job.status != "running") || busy.contains(id) {
                    continue;
                }
                // The age cap also covers local jobs (no provider handle), so a stuck one never holds a slot.
                if now.duration_since(e.started) > MAX_JOB_AGE {
                    fail_entry(e, "timed out".into());
                    continue;
                }
                let Some(handle) = e.handle.clone() else { continue };
                let interval = e.adapter.poll_interval();
                let ready = e.last_poll.is_none_or(|t| now.duration_since(t) >= interval);
                if ready {
                    e.last_poll = Some(now);
                    out.push(Due {
                        id: id.clone(),
                        adapter: e.adapter.clone(),
                        handle,
                        key_env: e.key_env.clone(),
                    });
                }
            }
            out
        });
        self.save_finished(); // jobs that just timed out
        out
    }
}

/// Runs a provider submit in the background and records the handle or the result.
pub fn spawn_submit(state: AppState, job_id: String, adapter: Arc<dyn Provider>, key: String, req: SubmitReq) {
    tokio::spawn(async move {
        match adapter.submit(&state.ctx, &key, &req).await {
            Ok(Submitted::Queued(handle)) => state.jobs.set_handle(&job_id, handle),
            Ok(Submitted::Done(outputs)) => state.jobs.set_done(&job_id, outputs),
            Err(ProviderError::NotImplemented) => state.jobs.set_failed(&job_id, "adapter_not_implemented"),
            Err(ProviderError::Failed(msg)) => state.jobs.set_failed(&job_id, scrub(msg, &key)),
        }
    });
}

async fn poll_one(state: &AppState, due: Due) {
    let Some(key) = state.keys.get(&due.key_env) else {
        state.jobs.set_failed(&due.id, "missing_key");
        return;
    };
    match due.adapter.poll(&state.ctx, &key, &due.handle).await {
        Ok(Polled::Running(p)) => state.jobs.set_progress(&due.id, p),
        Ok(Polled::Done(outputs)) => state.jobs.set_done(&due.id, outputs),
        Ok(Polled::Failed(msg)) => state.jobs.set_failed(&due.id, scrub(msg, &key)),
        Err(ProviderError::NotImplemented) => state.jobs.set_failed(&due.id, "adapter_not_implemented"),
        Err(ProviderError::Failed(msg)) => state.jobs.record_transient(&due.id, scrub(msg, &key)),
    }
}

/// Polls active jobs every 3 seconds. Each poll runs on its own, so one large download never holds up
/// the others; a job is never polled twice at once.
pub async fn run_poller(state: AppState) {
    let mut tick = tokio::time::interval(Duration::from_secs(3));
    let in_flight: Arc<std::sync::Mutex<HashSet<String>>> = Arc::default();
    loop {
        tick.tick().await;
        let busy = in_flight.lock().unwrap_or_else(|e| e.into_inner()).clone();
        for due in state.jobs.due(&busy) {
            in_flight.lock().unwrap_or_else(|e| e.into_inner()).insert(due.id.clone());
            let st = state.clone();
            let release = Release { set: in_flight.clone(), id: due.id.clone() };
            tokio::spawn(async move {
                let _release = release; // frees the job for the next tick even if the poll panics
                poll_one(&st, due).await;
            });
        }
    }
}

/// Removes a job from the in-flight set when its poll ends, however it ends.
struct Release {
    set: Arc<std::sync::Mutex<HashSet<String>>>,
    id: String,
}

impl Drop for Release {
    fn drop(&mut self) {
        self.set.lock().unwrap_or_else(|e| e.into_inner()).remove(&self.id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Never;
    #[async_trait::async_trait]
    impl Provider for Never {
        async fn submit(&self, _: &crate::media::Ctx, _: &str, _: &SubmitReq) -> crate::providers::PResult<Submitted> {
            Err(ProviderError::NotImplemented)
        }
    }

    fn job(id: &str) -> Job {
        Job {
            id: id.into(),
            model: "m".into(),
            mode: "t2v".into(),
            prompt: "p".into(),
            status: "queued".into(),
            progress: None,
            outputs: Vec::new(),
            error: None,
            created_at: 1,
            request: None,
        }
    }

    #[test]
    fn finished_jobs_survive_a_restart() {
        let dir = crate::fsutil::test_dir("jobs_saved");
        let path = dir.join("jobs.json");
        let store = JobStore::load(path.clone()).unwrap();
        assert!(store.insert(job("done1"), Arc::new(Never), "FAL_KEY".into()));
        assert!(store.insert(job("run1"), Arc::new(Never), "FAL_KEY".into()));
        assert!(store.insert(job("failed1"), Arc::new(Never), "FAL_KEY".into()));
        store.set_done("done1", vec![Output { url: "/outputs/x.png".into(), kind: "image".into() }]);
        store.set_failed("failed1", "boom");
        store.set_failed("failed1", "boom again");
        assert_eq!(store.list().len(), 3, "no duplicate once a live job is saved");
        assert_eq!(store.get("failed1").unwrap().error.as_deref(), Some("boom"), "a job fails only once");
        // Restart: the running job is gone, finished ones stay.
        let again = JobStore::load(path).unwrap();
        let ids: Vec<String> = again.list().into_iter().map(|j| j.id).collect();
        assert_eq!(ids.len(), 2);
        assert!(ids.contains(&"done1".to_string()) && ids.contains(&"failed1".to_string()));
        assert_eq!(again.get("done1").unwrap().outputs[0].url, "/outputs/x.png");
        // The running job had not reached its provider, so it comes back failed, ready to run again.
        again.restore(&crate::models::Registry::load(&PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("models.toml")).unwrap());
        assert_eq!(again.get("run1").unwrap().status, "failed");
    }

    #[test]
    fn paid_jobs_with_a_handle_are_polled_again_after_a_restart() {
        let dir = crate::fsutil::test_dir("jobs_active");
        let path = dir.join("jobs.json");
        let store = JobStore::load(path.clone()).unwrap();
        let mut j = job("paid1");
        j.model = "fal/kling-3-pro-t2v".into();
        j.created_at = now_unix();
        assert!(store.insert(j, Arc::new(Never), "FAL_KEY".into()));
        let handle = Handle { request_id: "r1".into(), status_url: "https://queue.fal.run/x/status".into(), result_url: None };
        store.set_handle("paid1", handle);
        let text = std::fs::read_to_string(dir.join("active-jobs.json")).unwrap();
        // Nothing changed, so the file is not written again (progress is left out on purpose).
        let modified = std::fs::metadata(dir.join("active-jobs.json")).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        store.set_progress("paid1", Some(0.5));
        store.save_active();
        assert_eq!(std::fs::metadata(dir.join("active-jobs.json")).unwrap().modified().unwrap(), modified);
        assert!(text.contains("queue.fal.run") && text.contains("FAL_KEY"), "handle and key name saved, never a key");
        let again = JobStore::load(path).unwrap();
        again.restore(&crate::models::Registry::load(&PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("models.toml")).unwrap());
        assert_eq!(again.get("paid1").unwrap().status, "running");
        let due = again.due(&HashSet::new());
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].handle.status_url, "https://queue.fal.run/x/status");
    }

    #[test]
    fn a_damaged_state_file_is_moved_aside() {
        let dir = crate::fsutil::test_dir("jobs_bad");
        let path = dir.join("jobs.json");
        std::fs::write(&path, b"{ not json").unwrap();
        let store = JobStore::load(path.clone()).unwrap();
        assert!(store.list().is_empty());
        assert!(dir.join("jobs.json.bad").exists() && !path.exists());
    }

    #[test]
    fn scrub_removes_key_from_provider_errors() {
        let msg = "provider HTTP 401: invalid key sk-test-abcdef123456 in header".to_string();
        let out = scrub(msg, "sk-test-abcdef123456");
        assert!(!out.contains("sk-test-abcdef123456"), "{out}");
        assert!(out.contains("[redacted]"));
    }

    #[test]
    fn scrub_removes_a_key_cut_off_by_the_excerpt() {
        // providers.rs keeps the first 300 characters of an error body, which can end in the middle of the key.
        let msg = "provider HTTP 401: bad key sk-test-abcdef1".to_string();
        let out = scrub(msg, "sk-test-abcdef123456");
        assert!(!out.contains("sk-test-abc"), "{out}");
        assert!(out.ends_with("[redacted]"), "{out}");
        // Short fragments that are not the start of the key are left alone.
        assert_eq!(scrub("rate limit: sk- prefix".to_string(), "sk-test-abcdef123456"), "rate limit: sk- prefix");
    }
}
