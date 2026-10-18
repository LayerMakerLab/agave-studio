//! Library pipelines: the studio's existing steps (a picture, Animate, Continue from the last frame, Save as
//! Element, 3D, Add to edit) run in a row, each step fed by the one before. Runs and their step state are kept
//! in data/pipelines.json, so a run survives a reload or a restart. A step that costs money never starts on its
//! own: it waits for the user's click (POST .../step). Free steps (Save as Element, the last frame, Add to edit,
//! and generations on your own ComfyUI, which cost nothing) run as soon as the step before them is done. A failed step stops the run; clicking it again runs that step
//! again, keeping everything before it.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::api::{submit_generate, GenerateReq};
use crate::jobs::now_unix;
use crate::media::{self, Output};
use crate::state::AppState;
use crate::studio::{self, EditClip, Element, StudioError};

/// Runs kept on disk; the oldest finished ones go first.
const MAX_RUNS: usize = 50;
/// The words for the subject also become the Element's words, so they share its limit.
const MAX_SUBJECT: usize = studio::MAX_NAME;

/// Where a generate step takes its picture from.
pub enum From {
    Nothing,
    /// The photo the user starts the run with.
    Start,
    /// The first picture made by an earlier step.
    Step(usize),
}

pub enum Kind {
    /// A paid provider call through the same path as the composer's Generate button.
    Generate {
        model: &'static str,
        mode: &'static str,
        /// `{subject}` and `{tag}` are filled in from the run.
        prompt: &'static str,
        recipe: Option<&'static str>,
        aspect: Option<&'static str>,
        duration: Option<u32>,
        resolution: Option<&'static str>,
        from: From,
    },
    /// Free: the last frame of an earlier step's clip, as a picture (the gallery's Continue).
    LastFrame { from: usize },
    /// Free: saves an earlier step's picture as the run's Element (@tag).
    Element { from: usize },
    /// Free: puts earlier steps' clips on the Edit timeline.
    Edit { from: &'static [usize] },
}

pub struct Step {
    pub title: &'static str,
    pub kind: Kind,
}

pub struct Pipeline {
    pub id: &'static str,
    pub title: &'static str,
    pub blurb: &'static str,
    /// Extra line shown under the steps (for example what a model cannot take).
    pub note: &'static str,
    pub needs_image: bool,
    pub image_hint: &'static str,
    pub subject_hint: &'static str,
    /// Default Element tag for the run.
    pub tag: &'static str,
    pub element_kind: &'static str,
    pub steps: &'static [Step],
}

const fn generate(title: &'static str, model: &'static str, mode: &'static str, prompt: &'static str, from: From) -> Step {
    Step { title, kind: Kind::Generate { model, mode, prompt, recipe: None, aspect: None, duration: None, resolution: None, from } }
}

const fn with(step: Step, aspect: Option<&'static str>, duration: Option<u32>, resolution: Option<&'static str>, recipe: Option<&'static str>) -> Step {
    match step.kind {
        Kind::Generate { model, mode, prompt, from, .. } => {
            Step { title: step.title, kind: Kind::Generate { model, mode, prompt, recipe, aspect, duration, resolution, from } }
        }
        other => Step { title: step.title, kind: other },
    }
}

pub const PIPELINES: &[Pipeline] = &[
    Pipeline {
        id: "product-to-3d",
        title: "Product: photo to hero still, video and 3D model",
        blurb: "One product photo becomes a studio hero still, a slow orbit clip and a 3D model. The hero still is saved as an Element, and the 3D model is made from that Element, so the product stays the same all the way.",
        note: "",
        needs_image: true,
        image_hint: "A clear photo of the product on its own.",
        subject_hint: "a matte black ceramic ramen bowl with a blue rim",
        tag: "product",
        element_kind: "product",
        steps: &[
            with(
                generate(
                    "Hero still",
                    "fal/nano-banana-edit",
                    "i2i",
                    "Studio hero shot of this exact product, {subject}, on a matte stone pedestal, seamless warm gray backdrop, one soft key light from above left and a subtle rim light, centered with clear space above for text. Keep the product unchanged: same shape, colours and details.",
                    From::Start,
                ),
                Some("16:9"),
                None,
                None,
                None,
            ),
            Step { title: "Save as Element", kind: Kind::Element { from: 0 } },
            with(
                generate(
                    "Video",
                    "fal/kling-2.5-turbo-i2v",
                    "i2v",
                    "Slow orbit around {subject} on its pedestal, soft studio light gliding across the surface. The product keeps its exact shape and colours. Steady exposure, no cuts.",
                    From::Step(0),
                ),
                None,
                Some(5),
                None,
                None,
            ),
            generate("3D model", "meshy/image-to-3d", "i23d", "@{tag}", From::Nothing),
        ],
    },
    Pipeline {
        id: "character-to-edit",
        title: "Character: sheet, Element, two clips, edit",
        blurb: "A photo of a character becomes a character sheet saved as an Element. The Element makes the opening still, the still becomes a clip, the clip's last frame starts the next clip, and both clips land on the Edit timeline.",
        note: "",
        needs_image: true,
        image_hint: "A photo of the character with the face and outfit visible.",
        subject_hint: "Mia, a woman in her late 20s with short dark hair",
        tag: "hero",
        element_kind: "character",
        steps: &[
            with(generate("Character sheet", "fal/nano-banana-edit", "i2i", "", From::Start), Some("16:9"), None, None, Some("sheet")),
            Step { title: "Save as Element", kind: Kind::Element { from: 0 } },
            with(
                generate(
                    "Opening still",
                    "fal/nano-banana-edit",
                    "i2i",
                    "@{tag} standing at the window of a small sunlit kitchen in the morning, holding a mug and looking outside. Natural window light, 35mm lens, real skin texture.",
                    From::Nothing,
                ),
                Some("16:9"),
                None,
                None,
                None,
            ),
            with(
                generate(
                    "Clip 1",
                    "fal/kling-o3-i2v",
                    "i2v",
                    "{subject} turns from the window with a small smile and takes a sip from the mug. Slow push-in, steady exposure, no cuts.",
                    From::Step(2),
                ),
                None,
                Some(5),
                None,
                None,
            ),
            Step { title: "Last frame", kind: Kind::LastFrame { from: 3 } },
            with(
                generate(
                    "Clip 2",
                    "fal/kling-o3-i2v",
                    "i2v",
                    "{subject} sets the mug down and walks toward the door, the camera following at shoulder height. Same light, no cuts.",
                    From::Step(4),
                ),
                None,
                Some(5),
                None,
                None,
            ),
            Step { title: "Add to edit", kind: Kind::Edit { from: &[3, 5] } },
        ],
    },
    Pipeline {
        id: "ltx-short-on-comfy",
        title: "Short film on your ComfyUI: look, Element, two LTX-Video shots, edit",
        blurb: "Runs free on your own ComfyUI with open-weights models. FLUX.1 schnell makes the character's look and saves it as an Element, LTX-Video turns that look into a shot, the shot's last frame starts the second shot, and both land on the Edit timeline.",
        note: "Needs ComfyUI with comfy-api-proxy running at the address in Settings (COMFY_BASE_URL, default http://127.0.0.1:8189) and the LTX-Video 0.9.5 and FLUX.1 schnell models installed there. Nothing is billed: it runs on your GPU.",
        needs_image: false,
        image_hint: "",
        subject_hint: "a young woman with short dark hair in a yellow raincoat",
        tag: "lead",
        element_kind: "character",
        steps: &[
            with(
                generate(
                    "Character look",
                    "comfy/flux-schnell",
                    "t2i",
                    "Full-length portrait of {subject}, standing on a rainy city street at dusk, neon reflections on the wet pavement, 35mm film look, natural skin texture.",
                    From::Nothing,
                ),
                Some("16:9"),
                None,
                None,
                None,
            ),
            Step { title: "Save as Element", kind: Kind::Element { from: 0 } },
            with(
                generate(
                    "Shot 1",
                    "comfy/ltxv-i2v",
                    "i2v",
                    "@{tag} walks along a rainy city street at dusk, neon signs reflecting in the puddles, the camera tracking alongside at walking pace. Raindrops catch the light, the coat moves with each step. 35mm film look, soft haze, real-life footage.",
                    From::Step(0),
                ),
                Some("16:9"),
                Some(2),
                None,
                None,
            ),
            Step { title: "Last frame", kind: Kind::LastFrame { from: 2 } },
            with(
                generate(
                    "Shot 2",
                    "comfy/ltxv-i2v",
                    "i2v",
                    "{subject} stops under a flickering neon sign and looks up as the rain eases, the light catching the face. Shallow depth of field, slow push-in, soft haze, real-life footage.",
                    From::Step(3),
                ),
                Some("16:9"),
                Some(2),
                None,
                None,
            ),
            Step { title: "Add to edit", kind: Kind::Edit { from: &[2, 4] } },
        ],
    },
];

pub fn find(id: &str) -> Option<&'static Pipeline> {
    PIPELINES.iter().find(|p| p.id == id)
}

/// A step on your own ComfyUI costs nothing. Every other provider bills the user's own account.
fn is_local(st: &AppState, step: &Step) -> bool {
    matches!(&step.kind, Kind::Generate { model, .. } if st.registry.get(model).is_some_and(|m| m.provider == "comfy"))
}

/// A step that spends money, so it never starts without the user's click.
fn is_paid(st: &AppState, step: &Step) -> bool {
    matches!(step.kind, Kind::Generate { .. }) && !is_local(st, step)
}

fn kind_name(step: &Step) -> &'static str {
    match step.kind {
        Kind::Generate { .. } => "generate",
        Kind::LastFrame { .. } => "last_frame",
        Kind::Element { .. } => "element",
        Kind::Edit { .. } => "edit",
    }
}

// ---------------------------------------------------------------- prices

/// One step's list price: US cents (fal and the other priced providers) or provider credits (Meshy). The same
/// models.toml prices and the same rounding as the composer's cost chip (app.js jobCents): rate times seconds
/// (or once per picture), rounded up to the cent, at least one cent.
#[derive(Default, Debug, PartialEq)]
pub struct Price {
    pub cents: Option<u64>,
    pub credits: Option<(u32, String)>,
    /// Runs on your own ComfyUI: no charge.
    pub local: bool,
}

pub fn step_price(st: &AppState, step: &Step) -> Price {
    let Kind::Generate { model, duration, resolution, .. } = &step.kind else { return Price::default() };
    let Some(m) = st.registry.get(model) else { return Price::default() };
    if m.provider == "comfy" {
        return Price { local: true, ..Price::default() };
    }
    if let Some(p) = &m.price {
        // What the generate path will send: the step's value when the model lists it, else the first listed.
        let res = resolution.map(str::to_string).or_else(|| m.params.resolution.first().cloned());
        let rate = res.as_ref().and_then(|r| p.by_resolution.get(r)).copied().unwrap_or(p.usd);
        let units = if p.per == "second" { f64::from(duration.or_else(|| m.params.duration.first().copied()).unwrap_or(0)) } else { 1.0 };
        if units > 0.0 {
            let micro = (rate * units * 1e6).round();
            return Price { cents: Some(((micro / 1e4).ceil() as u64).max(1)), ..Price::default() };
        }
    }
    Price { credits: m.provider_credits.map(|c| (c, m.vendor.clone())), ..Price::default() }
}

fn price_json(p: &Price) -> Value {
    json!({"cents": p.cents, "credits": p.credits.as_ref().map(|c| c.0), "credits_vendor": p.credits.as_ref().map(|c| c.1.clone()), "local": p.local})
}

/// The full list price of a pipeline: cents, credits per vendor, paid steps with no published price, and the
/// steps that run free on your own ComfyUI.
fn total_json(st: &AppState, p: &Pipeline) -> Value {
    let mut cents = 0u64;
    let mut credits: Vec<(String, u32)> = Vec::new();
    let mut unpriced = 0;
    let local = p.steps.iter().filter(|s| is_local(st, s)).count();
    for s in p.steps.iter().filter(|s| is_paid(st, s)) {
        let price = step_price(st, s);
        match (price.cents, price.credits) {
            (Some(c), _) => cents += c,
            (None, Some((n, vendor))) => match credits.iter_mut().find(|(v, _)| *v == vendor) {
                Some(entry) => entry.1 += n,
                None => credits.push((vendor, n)),
            },
            (None, None) => unpriced += 1,
        }
    }
    let credits: Vec<Value> = credits.into_iter().map(|(vendor, n)| json!({"vendor": vendor, "credits": n})).collect();
    json!({"cents": cents, "credits": credits, "unpriced": unpriced, "local_steps": local, "paid_steps": p.steps.iter().filter(|s| is_paid(st, s)).count()})
}

fn steps_json(st: &AppState, p: &Pipeline) -> Vec<Value> {
    p.steps
        .iter()
        .map(|s| {
            let (model, model_name, vendor) = match &s.kind {
                Kind::Generate { model, .. } => {
                    let m = st.registry.get(model);
                    (Some(*model), m.map(|m| m.name.clone()), m.map(|m| m.vendor.clone()))
                }
                _ => (None, None, None),
            };
            json!({"title": s.title, "kind": kind_name(s), "paid": is_paid(st, s), "model": model, "model_name": model_name,
                   "vendor": vendor, "price": price_json(&step_price(st, s))})
        })
        .collect()
}

/// Key slots a pipeline needs and whether each is set (all count as set in test mode).
fn keys_json(st: &AppState, p: &Pipeline) -> Vec<Value> {
    let mut keys: Vec<&str> = Vec::new();
    for s in p.steps {
        if let Kind::Generate { model, .. } = &s.kind {
            if let Some(m) = st.registry.get(model) {
                if !keys.contains(&m.key.as_str()) {
                    keys.push(m.key.as_str());
                }
            }
        }
    }
    keys.into_iter().map(|k| json!({"env": k, "set": st.fake.is_some() || st.keys.get_or_default(k).is_some()})).collect()
}

fn pipeline_json(st: &AppState, p: &Pipeline) -> Value {
    json!({"id": p.id, "title": p.title, "blurb": p.blurb, "note": p.note, "needs_image": p.needs_image,
           "image_hint": p.image_hint, "subject_hint": p.subject_hint, "tag": p.tag, "element_kind": p.element_kind,
           "steps": steps_json(st, p), "total": total_json(st, p), "keys": keys_json(st, p)})
}

// ---------------------------------------------------------------- runs and their store

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct StepRun {
    /// pending, waiting (a paid step waiting for the user's click), running, done or failed.
    pub status: String,
    #[serde(default)]
    pub job: Option<String>,
    #[serde(default)]
    pub outputs: Vec<Output>,
    #[serde(default)]
    pub error: Option<String>,
    /// The key slot a failed step needs (missing_key), so the app can open Settings at it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Run {
    pub id: String,
    pub pipeline: String,
    pub created_at: u64,
    pub subject: String,
    #[serde(default)]
    pub image: Option<String>,
    pub tag: String,
    /// waiting, running, failed or done.
    pub status: String,
    pub current: usize,
    pub steps: Vec<StepRun>,
}

#[derive(Serialize, Deserialize, Default)]
struct RunsFile {
    runs: Vec<Run>,
}

#[derive(Clone)]
pub struct PipelineStore {
    path: Arc<PathBuf>,
    runs: Arc<Mutex<Vec<Run>>>,
    /// Held while a run changes (create, click, the background check), so two changes never interleave and a
    /// paid step is never sent twice.
    engine: Arc<tokio::sync::Mutex<()>>,
}

impl PipelineStore {
    pub fn load(path: PathBuf) -> Result<Self, String> {
        let runs = crate::jobs::read_state::<RunsFile>(&path)?.runs;
        Ok(PipelineStore { path: Arc::new(path), runs: Arc::new(Mutex::new(runs)), engine: Arc::default() })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<Run>> {
        self.runs.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn list(&self) -> Vec<Run> {
        let mut runs = self.lock().clone();
        runs.sort_by(|a, b| b.created_at.cmp(&a.created_at).then_with(|| b.id.cmp(&a.id)));
        runs
    }

    pub fn get(&self, id: &str) -> Option<Run> {
        self.lock().iter().find(|r| r.id == id).cloned()
    }

    /// Stores a run (replacing the one with its id) and writes the file. A failed write is logged; the run
    /// still works until the next restart.
    fn put(&self, run: Run) {
        let mut runs = self.lock();
        match runs.iter_mut().find(|r| r.id == run.id) {
            Some(r) => *r = run,
            None => runs.push(run),
        }
        while runs.len() > MAX_RUNS {
            let oldest = runs
                .iter()
                .enumerate()
                .filter(|(_, r)| r.status == "done" || r.status == "failed")
                .min_by_key(|(_, r)| r.created_at)
                .map(|(i, _)| i);
            match oldest {
                Some(i) => {
                    runs.remove(i);
                }
                None => break,
            }
        }
        self.write(&runs);
    }

    fn remove(&self, id: &str) -> bool {
        let mut runs = self.lock();
        let before = runs.len();
        runs.retain(|r| r.id != id);
        let gone = runs.len() != before;
        if gone {
            self.write(&runs);
        }
        gone
    }

    fn write(&self, runs: &[Run]) {
        match serde_json::to_vec(&json!({"runs": runs})) {
            Ok(bytes) => {
                if let Err(e) = crate::fsutil::write_private(&self.path, &bytes) {
                    eprintln!("pipelines: write {}: {e}", self.path.display());
                }
            }
            Err(e) => eprintln!("pipelines: encode: {e}"),
        }
    }
}

fn fail_step(run: &mut Run, i: usize, message: impl Into<String>) {
    let s = &mut run.steps[i];
    s.status = "failed".into();
    s.error = Some(message.into());
    run.status = "failed".into();
}

fn fill(template: &str, run: &Run) -> String {
    template.replace("{subject}", &run.subject).replace("{tag}", &run.tag)
}

/// The first output of step `i` with one of these kinds, in the order the kinds are given.
fn first_output(run: &Run, i: usize, kinds: &[&str]) -> Option<Output> {
    let outs = &run.steps.get(i)?.outputs;
    kinds.iter().find_map(|k| outs.iter().find(|o| o.kind == *k)).cloned()
}

fn output_of(run: &Run, i: usize, kinds: &[&str]) -> Option<String> {
    first_output(run, i, kinds).map(|o| o.url)
}

/// Moves a run forward as far as it can go without spending money: picks up finished jobs, runs free steps,
/// and stops at a paid step (waiting for a click), a running job, a failure or the end.
async fn advance(st: &AppState, run: &mut Run) {
    let Some(p) = find(&run.pipeline) else {
        run.status = "failed".into();
        return;
    };
    while run.status != "done" && run.status != "failed" {
        let i = run.current;
        if i >= p.steps.len() || i >= run.steps.len() {
            run.status = "done".into();
            return;
        }
        match run.steps[i].status.as_str() {
            "pending" if is_paid(st, &p.steps[i]) => {
                run.steps[i].status = "waiting".into();
                run.status = "waiting".into();
                return;
            }
            "pending" => start(st, p, run, i).await,
            "waiting" => {
                run.status = "waiting".into();
                return;
            }
            "running" => {
                let job = run.steps[i].job.as_deref().and_then(|id| st.jobs.get(id));
                let Some(job) = job else {
                    fail_step(run, i, "The job for this step was lost. Run it again.");
                    return;
                };
                match job.status.as_str() {
                    "done" if job.outputs.is_empty() => fail_step(run, i, "The step finished with no output."),
                    "done" => {
                        run.steps[i].outputs = job.outputs;
                        run.steps[i].status = "done".into();
                        run.current += 1;
                        run.status = "running".into();
                    }
                    "failed" => fail_step(run, i, job.error.unwrap_or_else(|| "The step failed.".into())),
                    _ => {
                        run.status = "running".into();
                        return;
                    }
                }
            }
            "done" => run.current += 1,
            _ => {
                run.status = "failed".into();
                return;
            }
        }
    }
}

/// Runs one free step now (they are all local and quick).
async fn run_free(st: &AppState, p: &Pipeline, run: &mut Run, i: usize) {
    let result: Result<Vec<Output>, String> = match &p.steps[i].kind {
        Kind::Element { from } => match output_of(run, *from, &["image"]) {
            Some(img) => {
                let e = Element { tag: run.tag.clone(), name: run.subject.clone(), kind: p.element_kind.into(), images: vec![img.clone()], created_at: now_unix() };
                match st.studio.put_element(e) {
                    Ok(()) => Ok(vec![Output { url: img, kind: "image".into() }]),
                    Err(StudioError::Full) => Err("The studio already holds 200 Elements. Delete one, then run this step again.".into()),
                    Err(StudioError::Io) => Err("The Element could not be saved.".into()),
                }
            }
            None => Err(format!("Step {} made no picture to save.", from + 1)),
        },
        Kind::LastFrame { from } => match output_of(run, *from, &["video"]) {
            Some(clip) => last_frame(st, &clip).await.map(|o| vec![o]),
            None => Err(format!("Step {} made no clip.", from + 1)),
        },
        Kind::Edit { from } => {
            let mut doc = st.studio.edit();
            let mut added = Vec::new();
            for &j in from.iter() {
                let Some(out) = first_output(run, j, &["video", "image"]) else { continue };
                if doc.clips.len() >= crate::assemble::MAX_CLIPS {
                    break;
                }
                doc.clips.push(EditClip { url: out.url.clone(), kind: out.kind.clone(), from: 0.0, to: None, len: None, caption: String::new() });
                added.push(out);
            }
            if added.is_empty() {
                Err("There was nothing to add to the edit.".into())
            } else {
                st.studio.set_edit(doc).map(|()| added).map_err(|_| "The Edit timeline could not be saved.".to_string())
            }
        }
        Kind::Generate { .. } => Err("not a free step".into()),
    };
    match result {
        Ok(outputs) => {
            let s = &mut run.steps[i];
            s.outputs = outputs;
            s.status = "done".into();
            s.error = None;
            run.current += 1;
            run.status = "running".into();
        }
        Err(msg) => fail_step(run, i, msg),
    }
}

/// The last frame of a local clip as a PNG under /outputs (ffmpeg, argument list, no shell).
async fn last_frame(st: &AppState, url: &str) -> Result<Output, String> {
    let dir = &st.ctx.outputs_dir;
    let name = crate::assemble::video_ok(dir, url).ok_or("The clip is missing.")?;
    let out = format!("{}.png", media::random_hex(12));
    let mut cmd = tokio::process::Command::new("ffmpeg");
    // From half a second before the end, every frame overwrites the same file, so the last one stays.
    cmd.args(["-y", "-v", "error", "-sseof", "-0.5", "-i"]).arg(dir.join(&name)).args(["-update", "1"]).arg(dir.join(&out));
    crate::assemble::run(&mut cmd).await.map_err(|_| "Could not take the last frame of the clip.".to_string())?;
    if !dir.join(&out).is_file() {
        return Err("Could not take the last frame of the clip.".into());
    }
    Ok(Output { url: format!("/outputs/{out}"), kind: "image".into() })
}

/// Starts step `i`: a generation goes to its provider through the composer's own generate path; a free local
/// step runs now. Paid steps only get here from the user's click.
async fn start(st: &AppState, p: &Pipeline, run: &mut Run, i: usize) {
    run.steps[i] = StepRun { status: "pending".into(), ..Default::default() };
    run.status = "running".into();
    let Kind::Generate { model, mode, prompt, recipe, aspect, duration, resolution, from } = &p.steps[i].kind else {
        return run_free(st, p, run, i).await;
    };
    let image_url = match from {
        From::Nothing => None,
        From::Start => run.image.clone(),
        From::Step(j) => match output_of(run, *j, &["image"]) {
            Some(u) => Some(u),
            None => return fail_step(run, i, format!("Step {} made no picture to start from.", j + 1)),
        },
    };
    let req = GenerateReq {
        model: model.to_string(),
        mode: mode.to_string(),
        prompt: fill(prompt, run),
        aspect: aspect.map(String::from),
        duration: *duration,
        resolution: resolution.map(String::from),
        image_url,
        recipe: recipe.map(String::from),
        ..Default::default()
    };
    match submit_generate(st, req) {
        Ok(job) => {
            run.steps[i].status = "running".into();
            run.steps[i].job = Some(job.id);
        }
        Err(refused) => {
            let code = refused.body.get("error").and_then(Value::as_str).unwrap_or("refused").to_string();
            run.steps[i].key = refused.body.get("key").and_then(Value::as_str).map(String::from);
            fail_step(run, i, code);
        }
    }
}

/// The user's click on step `i`: sends a paid step, or runs a failed step again (keeping the steps before it).
async fn click(st: &AppState, run: &mut Run, i: usize) -> Result<(), &'static str> {
    let Some(p) = find(&run.pipeline) else { return Err("unknown_pipeline") };
    let ready = i == run.current && i < run.steps.len() && matches!(run.steps[i].status.as_str(), "waiting" | "failed");
    if !ready {
        return Err("step_not_ready");
    }
    start(st, p, run, i).await;
    advance(st, run).await;
    Ok(())
}

/// Moves every running run forward (finished jobs, then any free steps after them).
pub async fn tick(st: &AppState) {
    let _engine = st.pipelines.engine.lock().await;
    for mut run in st.pipelines.list().into_iter().filter(|r| r.status == "running") {
        let before = serde_json::to_string(&run).unwrap_or_default();
        advance(st, &mut run).await;
        if serde_json::to_string(&run).unwrap_or_default() != before {
            st.pipelines.put(run);
        }
    }
}

/// Checks running pipelines every 2 seconds, so a run moves on even with no page open.
pub async fn run_ticker(st: AppState) {
    let mut every = tokio::time::interval(Duration::from_secs(2));
    loop {
        every.tick().await;
        tick(&st).await;
    }
}

fn run_json(st: &AppState, run: &Run) -> Value {
    let p = find(&run.pipeline);
    let mut defs = p.map(|p| steps_json(st, p)).unwrap_or_default().into_iter();
    let steps: Vec<Value> = run
        .steps
        .iter()
        .map(|s| {
            let mut v = defs.next().unwrap_or_else(|| json!({}));
            v["status"] = json!(s.status);
            v["job"] = json!(s.job);
            v["outputs"] = json!(s.outputs);
            v["error"] = json!(s.error);
            v["key"] = json!(s.key);
            v
        })
        .collect();
    json!({"id": run.id, "pipeline": run.pipeline, "title": p.map(|p| p.title), "created_at": run.created_at,
           "subject": run.subject, "image": run.image, "tag": run.tag, "status": run.status, "current": run.current,
           "steps": steps, "total": p.map(|p| total_json(st, p))})
}

// ---------------------------------------------------------------- routes

fn fail(status: StatusCode, code: &str) -> Response {
    (status, Json(json!({"error": code}))).into_response()
}

/// GET /api/pipelines: every pipeline with its steps, prices and the keys it needs.
pub async fn list(State(st): State<AppState>) -> Response {
    let list: Vec<Value> = PIPELINES.iter().map(|p| pipeline_json(&st, p)).collect();
    Json(json!({"pipelines": list})).into_response()
}

/// GET /api/pipelines/runs: every run, newest first, after picking up finished jobs.
pub async fn runs(State(st): State<AppState>) -> Response {
    tick(&st).await;
    let runs: Vec<Value> = st.pipelines.list().iter().map(|r| run_json(&st, r)).collect();
    Json(json!({"runs": runs})).into_response()
}

#[derive(Deserialize)]
pub struct CreateReq {
    pipeline: String,
    subject: String,
    #[serde(default)]
    image: Option<String>,
    #[serde(default)]
    tag: Option<String>,
}

/// POST /api/pipelines/runs: makes a run. A first step that costs money waits for a click; free local steps start.
pub async fn create(State(st): State<AppState>, body: Result<Json<CreateReq>, JsonRejection>) -> Response {
    let Ok(Json(b)) = body else { return fail(StatusCode::BAD_REQUEST, "bad_request") };
    let Some(p) = find(&b.pipeline) else { return fail(StatusCode::BAD_REQUEST, "unknown_pipeline") };
    let subject: String = b.subject.chars().map(|c| if c.is_control() { ' ' } else { c }).collect::<String>().trim().to_string();
    if subject.is_empty() || subject.chars().count() > MAX_SUBJECT {
        return fail(StatusCode::BAD_REQUEST, "invalid_subject");
    }
    let image = match b.image.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(u) if crate::assemble::still_ok(&st.ctx.outputs_dir, u).is_some() => Some(u.to_string()),
        Some(_) => return fail(StatusCode::BAD_REQUEST, "invalid_image"),
        None if p.needs_image => return fail(StatusCode::BAD_REQUEST, "image_required"),
        None => None,
    };
    let tag = b.tag.as_deref().unwrap_or(p.tag).trim().trim_start_matches('@').to_ascii_lowercase();
    if !studio::valid_tag(&tag) {
        return fail(StatusCode::BAD_REQUEST, "invalid_tag");
    }
    let _engine = st.pipelines.engine.lock().await;
    // A run never replaces an Element the user already has.
    if st.studio.elements().iter().any(|e| e.tag == tag) {
        return fail(StatusCode::CONFLICT, "tag_in_use");
    }
    let taken = st.pipelines.list().iter().any(|r| r.tag == tag && r.status != "done" && r.status != "failed");
    if taken {
        return fail(StatusCode::CONFLICT, "tag_in_use");
    }
    let mut run = Run {
        id: format!("p_{}", media::random_hex(8)),
        pipeline: p.id.into(),
        created_at: now_unix(),
        subject,
        image,
        tag,
        status: "running".into(),
        current: 0,
        steps: p.steps.iter().map(|_| StepRun { status: "pending".into(), ..Default::default() }).collect(),
    };
    advance(&st, &mut run).await;
    st.pipelines.put(run.clone());
    (StatusCode::CREATED, Json(json!({"run": run_json(&st, &run)}))).into_response()
}

#[derive(Deserialize)]
pub struct StepReq {
    step: usize,
}

fn valid_run_id(id: &str) -> bool {
    id.strip_prefix("p_").is_some_and(|h| h.len() == 16 && h.chars().all(|c| c.is_ascii_hexdigit()))
}

/// POST /api/pipelines/runs/{id}/step {"step": n}: the user's click on step n (the current step, waiting or
/// failed). A refused provider request is recorded on the step, so the answer is the run either way.
pub async fn step(State(st): State<AppState>, Path(id): Path<String>, body: Result<Json<StepReq>, JsonRejection>) -> Response {
    let Ok(Json(b)) = body else { return fail(StatusCode::BAD_REQUEST, "bad_request") };
    if !valid_run_id(&id) {
        return fail(StatusCode::NOT_FOUND, "not_found");
    }
    let _engine = st.pipelines.engine.lock().await;
    let Some(mut run) = st.pipelines.get(&id) else { return fail(StatusCode::NOT_FOUND, "not_found") };
    // A job that finished since the last check moves the run on first, so the click lands on the right step.
    advance(&st, &mut run).await;
    let clicked = click(&st, &mut run, b.step).await;
    st.pipelines.put(run.clone());
    match clicked {
        Ok(()) => Json(json!({"run": run_json(&st, &run)})).into_response(),
        Err(code) => (StatusCode::CONFLICT, Json(json!({"error": code, "run": run_json(&st, &run)}))).into_response(),
    }
}

/// POST /api/pipelines/runs/{id}/delete: forgets a run that is not running. Its files and Element stay.
pub async fn delete(State(st): State<AppState>, Path(id): Path<String>) -> Response {
    if !valid_run_id(&id) {
        return fail(StatusCode::NOT_FOUND, "not_found");
    }
    let _engine = st.pipelines.engine.lock().await;
    match st.pipelines.get(&id) {
        Some(r) if r.status == "running" => fail(StatusCode::CONFLICT, "run_busy"),
        Some(_) if st.pipelines.remove(&id) => Json(json!({"ok": true})).into_response(),
        _ => fail(StatusCode::NOT_FOUND, "not_found"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::Fake;
    use axum::body::{to_bytes, Body};
    use axum::http::Request;
    use tower::ServiceExt;

    fn ffmpeg_ok() -> bool {
        std::process::Command::new("ffmpeg").arg("-version").output().is_ok()
    }

    /// Test state whose every model is answered by the local fake provider (no network, no key).
    fn fake_state(dir: PathBuf, delay_ms: u64) -> AppState {
        let mut st = AppState::for_tests_in(dir.clone());
        st.fake = Some(Arc::new(Fake::new(dir.join("no-web"), Duration::from_millis(delay_ms), "")));
        st
    }

    fn fake(st: &AppState) -> &Fake {
        st.fake.as_deref().unwrap()
    }

    async fn call(st: &AppState, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
        let mut b = Request::builder().method(method).uri(uri).header("host", "127.0.0.1:4170");
        if body.is_some() {
            b = b.header("content-type", "application/json");
        }
        let req = b.body(body.map(|v| Body::from(v.to_string())).unwrap_or_else(Body::empty)).unwrap();
        let resp = crate::api::router(st.clone()).oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    fn start_image(st: &AppState) -> String {
        std::fs::create_dir_all(&st.ctx.outputs_dir).unwrap();
        std::fs::write(st.ctx.outputs_dir.join("start.png"), b"\x89PNG\r\n\x1a\nfake").unwrap();
        "/outputs/start.png".into()
    }

    async fn create(st: &AppState, pipeline: &str, image: Option<String>) -> Value {
        let (status, v) = call(st, "POST", "/api/pipelines/runs", Some(json!({"pipeline": pipeline, "subject": "a blue ceramic bowl", "image": image}))).await;
        assert_eq!(status, StatusCode::CREATED, "{v}");
        v["run"].clone()
    }

    async fn click(st: &AppState, id: &str, step: usize) -> (StatusCode, Value) {
        call(st, "POST", &format!("/api/pipelines/runs/{id}/step"), Some(json!({"step": step}))).await
    }

    /// Polls GET /api/pipelines/runs (which moves runs on) until the run settles: waiting, failed or done.
    async fn settle(st: &AppState, id: &str) -> Value {
        for _ in 0..300 {
            let (_, v) = call(st, "GET", "/api/pipelines/runs", None).await;
            let run = v["runs"].as_array().unwrap().iter().find(|r| r["id"] == id).cloned().unwrap();
            if run["status"] != "running" {
                return run;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("run {id} never settled");
    }

    fn out(run: &Value, step: usize) -> String {
        run["steps"][step]["outputs"][0]["url"].as_str().unwrap_or_default().to_string()
    }

    #[tokio::test]
    async fn the_full_list_price_is_shown_before_anything_runs() {
        let st = fake_state(crate::fsutil::test_dir("pipe_price"), 0);
        let (status, v) = call(&st, "GET", "/api/pipelines", None).await;
        assert_eq!(status, StatusCode::OK);
        let p = |id: &str| v["pipelines"].as_array().unwrap().iter().find(|p| p["id"] == id).cloned().unwrap();
        // models.toml list prices, rounded up per step like the composer: Nano Banana edit $0.0398 -> 4 cents,
        // Kling 2.5 Turbo 5 s x $0.07 -> 35 cents, Meshy 30 credits.
        let product = p("product-to-3d");
        assert_eq!(product["total"]["cents"], 39);
        assert_eq!(product["total"]["credits"], json!([{"vendor": "Meshy", "credits": 30}]));
        assert_eq!(product["total"]["paid_steps"], 3);
        assert_eq!(product["steps"][1]["paid"], false, "Save as Element is free");
        // Two sheets/stills at 4 cents and two Kling O3 clips at 5 s x $0.084 = 42 cents.
        assert_eq!(p("character-to-edit")["total"]["cents"], 92);
        // The LTX pipeline runs on your own ComfyUI: nothing to pay, every generation marked local.
        let ltx = p("ltx-short-on-comfy");
        assert_eq!((ltx["total"]["cents"].as_u64(), ltx["total"]["paid_steps"].as_u64(), ltx["total"]["local_steps"].as_u64()), (Some(0), Some(0), Some(3)));
        assert_eq!(ltx["steps"][2]["price"]["local"], true);
        assert_eq!(ltx["steps"][2]["model"], "comfy/ltxv-i2v");
        // Every model a pipeline names exists and accepts the step's options.
        for p in PIPELINES {
            for s in p.steps {
                if let Kind::Generate { model, mode, aspect, duration, .. } = &s.kind {
                    let m = st.registry.get(model).unwrap_or_else(|| panic!("{model} missing"));
                    assert!(m.modes.iter().any(|x| x == mode), "{model} {mode}");
                    assert!(aspect.is_none_or(|a| m.params.aspect.iter().any(|x| x == a)), "{model} aspect");
                    assert!(duration.is_none_or(|d| m.params.duration.contains(&d)), "{model} duration");
                }
            }
        }
    }

    #[tokio::test]
    async fn paid_steps_wait_for_a_click_and_each_output_feeds_the_next() {
        if !ffmpeg_ok() {
            return;
        }
        let st = fake_state(crate::fsutil::test_dir("pipe_chain"), 0);
        let img = start_image(&st);
        let run = create(&st, "product-to-3d", Some(img.clone())).await;
        let id = run["id"].as_str().unwrap().to_string();
        // Created, but nothing sent: the first step is paid.
        assert_eq!((run["status"].as_str(), run["steps"][0]["status"].as_str()), (Some("waiting"), Some("waiting")));
        assert!(st.jobs.list().is_empty() && fake(&st).seen.lock().unwrap().is_empty());
        assert_eq!(click(&st, &id, 2).await.0, StatusCode::CONFLICT, "only the current step can be clicked");

        assert_eq!(click(&st, &id, 0).await.0, StatusCode::OK);
        let run = settle(&st, &id).await;
        // The hero still is done, Save as Element ran on its own, and the paid video waits.
        assert_eq!(run["current"], 2);
        assert_eq!(run["steps"][1]["status"], "done");
        assert_eq!(run["steps"][2]["status"], "waiting");
        assert_eq!(fake(&st).seen.lock().unwrap().len(), 1, "no paid step started without a click");
        let hero = out(&run, 0);
        assert_eq!(fake(&st).seen.lock().unwrap()[0].image_urls, vec![img], "step 1 starts from the user's photo");
        let el = st.studio.elements().into_iter().find(|e| e.tag == "product").unwrap();
        assert_eq!((el.images, el.name.as_str()), (vec![hero.clone()], "a blue ceramic bowl"));

        click(&st, &id, 2).await;
        let run = settle(&st, &id).await;
        assert_eq!(fake(&st).seen.lock().unwrap()[1].image_url.as_deref(), Some(hero.as_str()), "the video starts from the hero still");
        assert_eq!(run["steps"][2]["outputs"][0]["kind"], "video");
        assert_eq!(run["steps"][3]["status"], "waiting");

        click(&st, &id, 3).await;
        let run = settle(&st, &id).await;
        // The 3D step names only @product: the Element brings the hero still along.
        let seen = fake(&st).seen.lock().unwrap();
        assert_eq!(seen[2].image_urls, vec![hero]);
        assert_eq!(seen[2].prompt, "a blue ceramic bowl");
        assert_eq!(run["status"], "done");
    }

    #[tokio::test]
    async fn a_failed_step_stops_the_run_and_run_again_starts_at_that_step() {
        if !ffmpeg_ok() {
            return;
        }
        let st = fake_state(crate::fsutil::test_dir("pipe_fail"), 0);
        let img = start_image(&st);
        let id = create(&st, "product-to-3d", Some(img)).await["id"].as_str().unwrap().to_string();
        click(&st, &id, 0).await;
        settle(&st, &id).await;
        fake(&st).fail_next("video");
        click(&st, &id, 2).await;
        let run = settle(&st, &id).await;
        assert_eq!(run["status"], "failed");
        assert_eq!(run["steps"][2]["status"], "failed");
        assert!(run["steps"][2]["error"].as_str().unwrap().contains("fail once"));
        assert_eq!((run["steps"][0]["status"].as_str(), run["steps"][1]["status"].as_str()), (Some("done"), Some("done")));
        assert_eq!(run["steps"][3]["status"], "pending", "nothing after the failure ran");
        assert_eq!(click(&st, &id, 0).await.0, StatusCode::CONFLICT, "Run again is only for the failed step");

        let (status, _) = click(&st, &id, 2).await;
        assert_eq!(status, StatusCode::OK);
        let run = settle(&st, &id).await;
        assert_eq!(run["steps"][2]["status"], "done");
        assert_eq!(run["steps"][3]["status"], "waiting");
        let seen = fake(&st).seen.lock().unwrap();
        assert_eq!(seen.iter().filter(|r| r.mode == "i2i").count(), 1, "the hero still was not made again");
        assert_eq!(seen.iter().filter(|r| r.mode == "i2v").count(), 2);
    }

    #[tokio::test]
    async fn a_refused_step_records_why_and_can_be_run_again() {
        let st = AppState::for_tests("pipe_refused"); // no fake provider and no keys
        let img = start_image(&st);
        let id = create(&st, "product-to-3d", Some(img)).await["id"].as_str().unwrap().to_string();
        let (status, v) = click(&st, &id, 0).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(v["run"]["status"], "failed");
        assert_eq!((v["run"]["steps"][0]["error"].as_str(), v["run"]["steps"][0]["key"].as_str()), (Some("missing_key"), Some("FAL_KEY")));
        assert!(st.jobs.list().is_empty(), "nothing reached a provider");
        assert_eq!(click(&st, &id, 0).await.1["run"]["steps"][0]["status"], "failed", "it can be clicked again");
    }

    #[tokio::test]
    async fn runs_survive_a_restart_even_mid_step() {
        if !ffmpeg_ok() {
            return;
        }
        let dir = crate::fsutil::test_dir("pipe_restart");
        let st = fake_state(dir.clone(), 0);
        let img = start_image(&st);
        let id = create(&st, "product-to-3d", Some(img)).await["id"].as_str().unwrap().to_string();
        click(&st, &id, 0).await;
        let before = settle(&st, &id).await;
        drop(st);

        // A restart on the same data: the run is where it was, with its outputs.
        let again = fake_state(dir.clone(), 30_000);
        again.jobs.restore(&again.registry);
        let run = settle(&again, &id).await;
        assert_eq!((run["status"].as_str(), run["current"].as_u64()), (Some("waiting"), Some(2)));
        assert_eq!(out(&run, 0), out(&before, 0));

        // Click the video, then "restart" while its job is still on its way (the fake takes 30 s).
        click(&again, &id, 2).await;
        assert_eq!(again.pipelines.get(&id).unwrap().steps[2].status, "running");
        drop(again);
        let third = fake_state(dir, 0);
        third.jobs.restore(&third.registry);
        let run = settle(&third, &id).await;
        assert_eq!(run["status"], "failed", "a job the studio lost on the way fails the step, it is not sent again");
        assert!(run["steps"][2]["error"].as_str().unwrap().contains("Run it again"), "{}", run["steps"][2]["error"]);
        click(&third, &id, 2).await;
        let run = settle(&third, &id).await;
        assert_eq!(run["steps"][2]["status"], "done");
        assert_eq!(out(&run, 0), out(&before, 0), "earlier steps kept");
    }

    #[tokio::test]
    async fn local_steps_run_on_their_own_and_chain_through_the_last_frame_into_the_edit() {
        if !ffmpeg_ok() {
            return;
        }
        let st = fake_state(crate::fsutil::test_dir("pipe_ltx"), 0);
        let run = create(&st, "ltx-short-on-comfy", None).await;
        let id = run["id"].as_str().unwrap().to_string();
        assert_eq!(run["steps"][0]["status"], "running", "a free local step starts with the run");
        let run = settle(&st, &id).await;
        assert_eq!(run["status"], "done", "{run}");
        let seen = fake(&st).seen.lock().unwrap();
        assert_eq!(seen.len(), 3);
        assert_eq!(seen[1].image_url.as_deref(), Some(out(&run, 0).as_str()), "shot 1 starts from the look");
        assert!(seen[1].prompt.starts_with("a blue ceramic bowl walks"), "@lead became the Element's words: {}", seen[1].prompt);
        assert_eq!(seen[2].image_url.as_deref(), Some(out(&run, 3).as_str()), "shot 2 starts from shot 1's last frame");
        assert!(out(&run, 3).ends_with(".png"));
        let edit: Vec<String> = st.studio.edit().clips.into_iter().map(|c| c.url).collect();
        assert_eq!(edit, vec![out(&run, 2), out(&run, 4)]);
    }

    #[tokio::test]
    async fn runs_check_their_inputs_and_never_replace_an_element() {
        let st = fake_state(crate::fsutil::test_dir("pipe_inputs"), 0);
        let post = |body: Value| call(&st, "POST", "/api/pipelines/runs", Some(body));
        assert_eq!(post(json!({"pipeline": "nope", "subject": "x"})).await.1["error"], "unknown_pipeline");
        assert_eq!(post(json!({"pipeline": "product-to-3d", "subject": "x"})).await.1["error"], "image_required");
        assert_eq!(post(json!({"pipeline": "product-to-3d", "subject": "x", "image": "/outputs/missing.png"})).await.1["error"], "invalid_image");
        assert_eq!(post(json!({"pipeline": "ltx-short-on-comfy", "subject": " "})).await.1["error"], "invalid_subject");
        assert_eq!(post(json!({"pipeline": "ltx-short-on-comfy", "subject": "x".repeat(61)})).await.1["error"], "invalid_subject");
        assert_eq!(post(json!({"pipeline": "ltx-short-on-comfy", "subject": "x", "tag": "Bad Tag"})).await.1["error"], "invalid_tag");
        let img = start_image(&st);
        st.studio.put_element(Element { tag: "mine".into(), name: "My cat".into(), kind: "character".into(), images: vec![img], created_at: 1 }).unwrap();
        let (status, v) = post(json!({"pipeline": "ltx-short-on-comfy", "subject": "x", "tag": "@mine"})).await;
        assert_eq!((status, v["error"].as_str()), (StatusCode::CONFLICT, Some("tag_in_use")));
        assert_eq!(call(&st, "POST", "/api/pipelines/runs/p_zz/step", Some(json!({"step": 0}))).await.0, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn a_run_can_be_forgotten_when_it_is_not_running() {
        let st = fake_state(crate::fsutil::test_dir("pipe_delete"), 0);
        let img = start_image(&st);
        let id = create(&st, "product-to-3d", Some(img)).await["id"].as_str().unwrap().to_string();
        let (status, _) = call(&st, "POST", &format!("/api/pipelines/runs/{id}/delete"), Some(json!({}))).await;
        assert_eq!(status, StatusCode::OK);
        assert!(st.pipelines.get(&id).is_none());
        let saved = std::fs::read_to_string(st.paths.pipelines_file.clone()).unwrap();
        assert!(!saved.contains(&id));
    }
}
