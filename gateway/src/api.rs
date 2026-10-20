//! HTTP routes. Contract: docs/API.md.

use std::net::SocketAddr;

use axum::extract::rejection::JsonRejection;
use axum::extract::{ConnectInfo, DefaultBodyLimit, Path, Request, State};
use axum::http::{header, Method, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tower_http::services::ServeDir;

use crate::assemble::{self, Assemble};
use crate::jobs::{self, now_unix, Job};
use crate::keys::{self, SLOTS};
use crate::media;
use crate::moves;
use crate::pipelines;
use crate::promptcraft;
use crate::providers::{self, SubmitReq};
use crate::reframe;
use crate::setup;
use crate::speech;
use crate::state::AppState;
use crate::studio::{self, Element, Project, StudioError};

const MAX_PROMPT_CHARS: usize = 2000;
/// After @tags are expanded and the style prefix is added.
const MAX_FINAL_PROMPT_CHARS: usize = 4000;
/// A 3D job's words are only its name.
const MAX_TEXTURE_PROMPT_CHARS: usize = 800;
/// A job keeps its request for "Run again" only when it is this small (data: URIs are not kept).
const MAX_SAVED_REQUEST_BYTES: usize = 16 << 10;
const IMAGE_EXT: [&str; 4] = ["png", "jpg", "jpeg", "webp"];

pub(crate) fn fail(status: StatusCode, code: impl AsRef<str>) -> Response {
    (status, Json(json!({"error": code.as_ref()}))).into_response()
}


/// Rejects requests that did not come from the gateway's own pages:
/// - Host must be 127.0.0.1:4170 or localhost:4170 (blocks DNS rebinding), on every route.
/// - Origin, when present, must be the gateway's own origin (blocks cross-site POSTs).
/// - Sec-Fetch-Site, when present on an /api/ request, must be "same-origin" or "none": another website
///   (even one on another local port) cannot make the gateway call a provider with the user's key through a
///   plain GET such as <img src="http://127.0.0.1:4170/api/voices">, which carries no Origin header.
/// - Every non-GET/HEAD request must be Content-Type application/json, so a cross-site
///   "simple" request (form or text/plain) can never reach a handler without a CORS preflight,
///   and the gateway never answers preflights.
///
/// Host and Origin lists come from AppState: the gateway's own local addresses.
async fn guard(State(st): State<AppState>, req: Request, next: Next) -> Response {
    let allowed = |h: &str| st.allowed_hosts.iter().any(|a| h.eq_ignore_ascii_case(a));
    // HTTP/1.1 carries the Host header; HTTP/2 and absolute-form requests carry a URI authority.
    // Whatever is present must be allowed, and at least one must be present.
    let header_host = req.headers().get(header::HOST).map(|v| v.to_str().unwrap_or(""));
    let uri_host = req.uri().authority().map(|a| a.as_str());
    let host_ok = (header_host.is_some() || uri_host.is_some())
        && header_host.is_none_or(allowed)
        && uri_host.is_none_or(allowed);
    if !host_ok {
        return fail(StatusCode::FORBIDDEN, "bad_host");
    }
    // A path that starts with "//" is never one of the studio's pages. The static file service would answer it
    // with a redirect to "//name/", which a browser reads as another host.
    if req.uri().path().starts_with("//") {
        return fail(StatusCode::NOT_FOUND, "not_found");
    }
    // /outputs serves only names the gateway itself writes: never a dot folder (edit and camera-move work), a
    // subfolder or an encoded name.
    if let Some(name) = req.uri().path().strip_prefix("/outputs/") {
        if !media::valid_output_name(name) {
            return fail(StatusCode::NOT_FOUND, "not_found");
        }
    }
    if let Some(origin) = req.headers().get(header::ORIGIN) {
        let ok = origin
            .to_str()
            .is_ok_and(|o| st.allowed_origins.iter().any(|a| o.eq_ignore_ascii_case(a)));
        if !ok {
            return fail(StatusCode::FORBIDDEN, "bad_origin");
        }
    }
    if req.uri().path().starts_with("/api/") {
        if let Some(site) = req.headers().get("sec-fetch-site") {
            let ok = site
                .to_str()
                .is_ok_and(|s| s.eq_ignore_ascii_case("same-origin") || s.eq_ignore_ascii_case("none"));
            if !ok {
                return fail(StatusCode::FORBIDDEN, "bad_site");
            }
        }
    }
    if req.method() != Method::GET && req.method() != Method::HEAD {
        let json = req
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.split(';').next())
            .is_some_and(|m| m.trim().eq_ignore_ascii_case("application/json"));
        if !json {
            return fail(StatusCode::UNSUPPORTED_MEDIA_TYPE, "json_required");
        }
    }
    // Pages, scripts and styles revalidate on every load (a cheap 304 when unchanged), so a browser never runs an
    // old app.js after an update. Chrome's reload only revalidates the page itself, not its scripts.
    let output = req.uri().path().starts_with("/outputs/");
    let page = !req.uri().path().starts_with("/api/") && !output;
    let mut resp = next.run(req).await;
    if page && !resp.headers().contains_key(header::CACHE_CONTROL) {
        resp.headers_mut().insert(header::CACHE_CONTROL, header::HeaderValue::from_static("no-cache"));
    }
    // No other site may frame the studio (a framed Generate button could be clicked into spending money).
    let h = resp.headers_mut();
    h.insert(header::X_FRAME_OPTIONS, header::HeaderValue::from_static("DENY"));
    h.insert(header::X_CONTENT_TYPE_OPTIONS, header::HeaderValue::from_static("nosniff"));
    // Generated files are media, never pages: a sandbox with no scripts, so nothing in them can run as the
    // studio. allow-same-origin only lets the browser's own media page load the file; scripts stay off.
    let csp = if output {
        "sandbox allow-same-origin; default-src 'none'; img-src 'self'; media-src 'self'; style-src 'unsafe-inline'; frame-ancestors 'none'"
    } else {
        // Scripts, fetches and frames only from the studio itself: injected markup could not run code from elsewhere
        // or call out with fetch. (Images stay open: reference pictures can be any https URL.) blob: is the 3D viewer
        // reading a model's own textures.
        "frame-ancestors 'none'; object-src 'none'; base-uri 'none'; form-action 'self'; script-src 'self'; connect-src 'self' blob:; frame-src 'none'"
    };
    h.insert(header::CONTENT_SECURITY_POLICY, header::HeaderValue::from_static(csp));
    // Links out of the studio (provider docs, GitHub) never carry the local address or page path.
    h.insert(header::REFERRER_POLICY, header::HeaderValue::from_static("no-referrer"));
    resp
}

pub fn router(state: AppState) -> Router {
    let outputs = state.ctx.outputs_dir.clone();
    let web = state.paths.web.clone();
    Router::new()
        .route("/api/health", get(health))
        .route("/api/models", get(models))
        .route("/api/presets", get(presets))
        .route("/api/keys", get(list_keys).post(set_key))
        .route("/api/voices", get(voices))
        .route("/api/generate", post(generate))
        .route("/api/jobs", get(list_jobs))
        .route("/api/jobs/{id}", get(get_job))
        .route("/api/uploads", post(upload).layer(DefaultBodyLimit::max(studio::MAX_UPLOAD_BYTES * 4 / 3 + 4096)))
        .route("/api/elements", get(list_elements).post(put_element))
        .route("/api/elements/delete", post(delete_element))
        .route("/api/project", get(get_project).post(set_project))
        .route("/api/assemble", post(assemble_clips))
        .route("/api/edit", get(get_edit).post(set_edit))
        .route("/api/transcribe", post(transcribe))
        .route("/api/pauses", post(find_pauses))
        .route("/api/reframe", post(reframe_path))
        .route("/api/setup", get(setup_status))
        .route("/api/setup/download", post(setup_download))
        .route("/api/setup/cancel", post(setup_cancel))
        .route("/api/pipelines", get(pipelines::list))
        .route("/api/pipelines/runs", get(pipelines::runs).post(pipelines::create))
        .route("/api/pipelines/runs/{id}/step", post(pipelines::step))
        .route("/api/pipelines/runs/{id}/delete", post(pipelines::delete))
        .nest_service("/outputs", ServeDir::new(outputs))
        .fallback_service(ServeDir::new(web).append_index_html_on_directories(true))
        .layer(middleware::from_fn_with_state(state.clone(), guard))
        .with_state(state)
}

async fn health(State(st): State<AppState>) -> Json<Value> {
    Json(json!({"ok": true, "version": env!("CARGO_PKG_VERSION"), "fake_providers": st.fake.is_some()}))
}

/// Every model; each is ready when its provider key is set.
async fn models(State(st): State<AppState>) -> Json<Value> {
    // In test mode every model runs on the local fake provider, so every model is ready.
    let models: Vec<Value> = st.registry.models.iter().map(|m| m.view(st.fake.is_some() || st.keys.get(&m.key).is_some())).collect();
    Json(json!({"models": models}))
}

/// The preset catalog, with each camera move's keyframe plan (moves.rs) for models that take an end picture:
/// {"kind":"crop"} or {"kind":"view","model":"<edit model id>"}, null when the move stays words.
async fn presets(State(st): State<AppState>) -> Json<Value> {
    let mut v = st.presets.json.clone();
    if let Some(cams) = v.get_mut("camera").and_then(Value::as_array_mut) {
        for c in cams {
            let plan = c.get("id").and_then(Value::as_str).and_then(moves::keyframe);
            c["keyframe"] = match plan {
                Some(moves::Keyframe::Crop(_)) => json!({"kind": "crop"}),
                Some(moves::Keyframe::View { model, .. }) => json!({"kind": "view", "model": model}),
                None => Value::Null,
            };
        }
    }
    Json(v)
}

async fn list_keys(State(st): State<AppState>) -> Response {
    let keys: Vec<Value> = SLOTS
        .iter()
        .map(|s| json!({"env": s.env, "provider": s.provider, "set": st.keys.is_set(s.env), "docs": s.docs}))
        .collect();
    Json(json!({"keys": keys})).into_response()
}

/// ElevenLabs voices for the Audio picker, read with the user's key.
async fn voices(State(st): State<AppState>) -> Response {
    let Some(key) = st.keys.get("ELEVENLABS_API_KEY") else {
        return fail(StatusCode::PRECONDITION_FAILED, "key_missing");
    };
    match providers::elevenlabs_voices(&st.ctx.http, &key).await {
        Ok(list) => Json(json!({"voices": list})).into_response(),
        Err(_) => fail(StatusCode::BAD_GATEWAY, "voices_unavailable"),
    }
}

#[derive(Deserialize)]
struct KeyBody {
    env: String,
    value: String,
}

async fn set_key(
    State(st): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    body: Result<Json<KeyBody>, JsonRejection>,
) -> Response {
    if !peer.ip().is_loopback() {
        return fail(StatusCode::FORBIDDEN, "localhost_only");
    }
    let Json(b) = match body {
        Ok(b) => b,
        Err(_) => return fail(StatusCode::BAD_REQUEST, "bad_request"),
    };
    if keys::slot(&b.env).is_none() {
        return fail(StatusCode::BAD_REQUEST, "unknown_slot");
    }
    // Error codes never echo the value.
    if let Err(code) = keys::validate_value(&b.value) {
        return fail(StatusCode::BAD_REQUEST, code);
    }
    match st.keys.set(&b.env, &b.value) {
        Ok(()) => Json(json!({"ok": true})).into_response(),
        Err(_) => fail(StatusCode::INTERNAL_SERVER_ERROR, "write_failed"),
    }
}

#[derive(Deserialize, Serialize, Default, Clone)]
pub(crate) struct GenerateReq {
    pub(crate) model: String,
    pub(crate) mode: String,
    pub(crate) prompt: String,
    pub(crate) camera: Option<String>,
    pub(crate) style: Option<String>,
    pub(crate) aspect: Option<String>,
    pub(crate) duration: Option<u32>,
    pub(crate) resolution: Option<String>,
    pub(crate) seed: Option<i64>,
    /// Native audio on or off, for models that list it (models.toml params.audio); omitted: the model's default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) audio: Option<bool>,
    pub(crate) image_url: Option<String>,
    /// More reference images (models with max_refs). Self-host also adds the images of @tagged Elements.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) image_urls: Option<Vec<String>>,
    /// A prompt recipe (studio::recipe): grid4, grid9, edit, sheet, product, look. The prompt is its detail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) recipe: Option<String>,
    /// Extension: ElevenLabs needs a voice id, and the contract has no field for it.
    pub(crate) voice_id: Option<String>,
    /// Set by pipeline steps ("Step 3 · Video"); never read from a request.
    #[serde(skip)]
    pub(crate) label: Option<String>,
}

/// The requested option if the model allows it, else the first allowed value.
fn choose<T: PartialEq + Clone>(requested: Option<&T>, allowed: &[T], name: &str) -> Result<Option<T>, String> {
    if allowed.is_empty() {
        return Ok(None); // not applicable to this model, so never passed to the provider
    }
    match requested {
        Some(v) if allowed.contains(v) => Ok(Some(v.clone())),
        Some(_) => Err(format!("unsupported_{name}")),
        None => Ok(allowed.first().cloned()),
    }
}

/// Image inputs: https URLs (fetched with SSRF checks in media::load_image), base64 data:image URIs,
/// or a file the gateway saved under /outputs. Plain http and every other scheme are refused.
fn valid_image_src(s: &str) -> bool {
    if s.starts_with("data:image/") {
        return s.split_once(',').is_some_and(|(meta, _)| meta.ends_with(";base64"));
    }
    if let Some(name) = s.strip_prefix("/outputs/") {
        return media::valid_output_name(name);
    }
    media::parse_https_url(s).is_some()
}

fn new_job_id() -> String {
    format!("j_{}", media::random_hex(8))
}

async fn generate(State(st): State<AppState>, body: Result<Json<GenerateReq>, JsonRejection>) -> Response {
    let Json(req) = match body {
        Ok(b) => b,
        Err(_) => return fail(StatusCode::BAD_REQUEST, "bad_request"),
    };
    match submit_generate(&st, req) {
        Ok(job) => (StatusCode::ACCEPTED, Json(json!({"job": job}))).into_response(),
        Err(e) => e.into_response(),
    }
}

/// A refused generate request: the HTTP status and the JSON body ({"error": code, ...}).
pub(crate) struct Refused {
    pub status: StatusCode,
    pub body: Value,
}

impl Refused {
    fn code(status: StatusCode, code: &str) -> Self {
        Refused { status, body: json!({"error": code}) }
    }
}

impl IntoResponse for Refused {
    fn into_response(self) -> Response {
        (self.status, Json(self.body)).into_response()
    }
}

/// Checks a generate request, queues the job and starts the provider call. Shared by POST /api/generate
/// and the pipeline steps, so both follow exactly the same rules.
pub(crate) fn submit_generate(st: &AppState, req: GenerateReq) -> Result<Job, Refused> {
    let fail = |status: StatusCode, code: &str| Refused::code(status, code);
    let Some(model) = st.registry.get(&req.model).cloned() else {
        return Err(fail(StatusCode::BAD_REQUEST, "unknown_model"));
    };
    if !model.modes.iter().any(|m| m == &req.mode) {
        return Err(fail(StatusCode::BAD_REQUEST, "unsupported_mode"));
    }

    let user_prompt = req.prompt.trim();
    if user_prompt.chars().count() > MAX_PROMPT_CHARS {
        return Err(fail(StatusCode::BAD_REQUEST, "prompt_too_long"));
    }
    let recipe_prompt = match &req.recipe {
        Some(r) => match studio::recipe(r, user_prompt) {
            Some(p) => Some(p),
            None if user_prompt.is_empty() => return Err(fail(StatusCode::BAD_REQUEST, "prompt_required")),
            None => return Err(fail(StatusCode::BAD_REQUEST, "unknown_recipe")),
        },
        None => None,
    };
    let prompt = recipe_prompt.as_deref().unwrap_or(user_prompt);
    // 3D from pictures needs no words; any words only name the job (kept short, like a title).
    if req.mode == "i23d" && prompt.chars().count() > MAX_TEXTURE_PROMPT_CHARS {
        return Err(fail(StatusCode::BAD_REQUEST, "prompt_too_long"));
    }
    if prompt.is_empty() && req.mode != "i23d" {
        return Err(fail(StatusCode::BAD_REQUEST, "prompt_required"));
    }
    // @tags become the Element's words, and its images become references. A model that takes Elements beside
    // its start frame (Kling 3.0) gets them as Elements instead, called @Element1, @Element2... in the words.
    let own_image = req.image_url.is_some() || req.image_urls.as_ref().is_some_and(|v| !v.is_empty());
    let (prompt, element_refs, elements) = if req.mode == "i2v" && model.max_elements > 0 && own_image {
        let (p, groups) = st.studio.expand_elements(prompt);
        if groups.len() > model.max_elements as usize {
            return Err(Refused {
                status: StatusCode::BAD_REQUEST,
                body: json!({"error": "too_many_element_tags", "max": model.max_elements, "got": groups.len()}),
            });
        }
        (p, Vec::new(), groups)
    } else {
        let (p, refs) = st.studio.expand(prompt);
        (p, refs, Vec::new())
    };
    let prompt = prompt.as_str();
    if let Some(c) = &req.camera {
        if !st.presets.camera_ids.contains(c) {
            return Err(fail(StatusCode::BAD_REQUEST, "unknown_camera"));
        }
    }
    if let Some(s) = &req.style {
        if !st.presets.style_ids.contains(s) {
            return Err(fail(StatusCode::BAD_REQUEST, "unknown_style"));
        }
    }

    // A look keeps the picture's own frame: no aspect, so the edit model sizes it from the picture.
    let aspect = match choose(req.aspect.as_ref(), &model.params.aspect, "aspect") {
        _ if req.recipe.as_deref() == Some("look") => None,
        Ok(v) => v,
        Err(code) => return Err(fail(StatusCode::BAD_REQUEST, &code)),
    };
    let duration = match choose(req.duration.as_ref(), &model.params.duration, "duration") {
        Ok(v) => v,
        Err(code) => return Err(fail(StatusCode::BAD_REQUEST, &code)),
    };
    let resolution = match choose(req.resolution.as_ref(), &model.params.resolution, "resolution") {
        Ok(v) => v,
        Err(code) => return Err(fail(StatusCode::BAD_REQUEST, &code)),
    };
    let audio = match choose(req.audio.as_ref(), &model.params.audio, "audio") {
        Ok(v) => v,
        Err(code) => return Err(fail(StatusCode::BAD_REQUEST, &code)),
    };

    // References: the image_url, then image_urls, then the images of @tagged Elements, each once.
    let mut refs: Vec<String> = Vec::new();
    for u in req.image_url.iter().chain(req.image_urls.iter().flatten()) {
        if !valid_image_src(u) {
            return Err(fail(StatusCode::BAD_REQUEST, "invalid_image_url"));
        }
        if !refs.contains(u) {
            refs.push(u.clone());
        }
    }
    for u in element_refs {
        if !refs.contains(&u) {
            refs.push(u);
        }
    }
    let (image_url, image_urls) = if matches!(req.mode.as_str(), "i2v" | "i2i" | "i23d") {
        if refs.is_empty() {
            return Err(fail(StatusCode::BAD_REQUEST, "image_url_required"));
        }
        let max = model.max_refs.max(1) as usize;
        if refs.len() > max {
            return Err(Refused { status: StatusCode::BAD_REQUEST, body: json!({"error": "too_many_refs", "max": max, "got": refs.len()}) });
        }
        if model.max_refs > 0 {
            (None, refs)
        } else {
            (refs.pop(), Vec::new())
        }
    } else {
        if !refs.is_empty() {
            return Err(fail(StatusCode::BAD_REQUEST, "refs_need_image_mode"));
        }
        (None, Vec::new())
    };

    // Video starts from an approved still unless the project turns the rule off.
    let project = st.studio.project();
    if project.still_first && req.mode == "t2v" {
        return Err(fail(StatusCode::CONFLICT, "still_first"));
    }

    let voice_id = if model.modality == "audio" {
        match &req.voice_id {
            Some(v) if providers::valid_voice(v) => Some(v.clone()),
            Some(_) => return Err(fail(StatusCode::BAD_REQUEST, "invalid_voice_id")),
            None => return Err(fail(StatusCode::BAD_REQUEST, "voice_id_required")),
        }
    } else {
        None
    };

    // Test mode (AGAVE_FAKE_PROVIDERS=1 at startup) answers every model locally and never needs a key.
    let (adapter, key) = if let Some(fake) = &st.fake {
        (fake.clone() as std::sync::Arc<dyn providers::Provider>, String::new())
    } else {
        let Some(adapter) = providers::adapter_for(&model) else {
            return Err(Refused {
                status: StatusCode::NOT_IMPLEMENTED,
                body: json!({"error": "adapter_not_implemented", "provider": model.provider}),
            });
        };
        // Key check comes after validation. The key value is used only for the outgoing call.
        let Some(key) = st.keys.get(&model.key) else {
            return Err(Refused { status: StatusCode::PRECONDITION_FAILED, body: json!({"error": "missing_key", "key": model.key}) });
        };
        (adapter, key)
    };

    // Camera moves apply to video only. Audio text is spoken as-is, so it gets no added language.
    let final_prompt = match model.modality.as_str() {
        "audio" | "3d" => prompt.to_string(),
        "video" => promptcraft::compose(prompt, req.camera.as_deref(), req.style.as_deref()),
        _ => promptcraft::compose(prompt, None, req.style.as_deref()),
    };
    // The project look goes first: models weigh early words more.
    let prefix = project.style_prefix.trim();
    let final_prompt = if matches!(model.modality.as_str(), "image" | "video") && !prefix.is_empty() {
        format!("{}. {final_prompt}", prefix.trim_end_matches('.'))
    } else {
        final_prompt
    };
    if final_prompt.chars().count() > MAX_FINAL_PROMPT_CHARS {
        return Err(fail(StatusCode::BAD_REQUEST, "prompt_too_long"));
    }
    let request = serde_json::to_value(&req).ok().filter(|v| v.to_string().len() <= MAX_SAVED_REQUEST_BYTES);

    let job = Job {
        id: new_job_id(),
        model: model.id.clone(),
        mode: req.mode.clone(),
        // A 3D job without words is named after its pictures, so its card has a title.
        prompt: if final_prompt.is_empty() {
            let n = image_urls.len().max(usize::from(image_url.is_some()));
            format!("3D model from {n} picture{}", if n == 1 { "" } else { "s" })
        } else {
            final_prompt.clone()
        },
        status: "queued".into(),
        progress: None,
        outputs: Vec::new(),
        error: None,
        created_at: now_unix(),
        request,
    };
    // Kling 3.0 often ignores camera words: a keyframed move (moves.rs) sends where the move ends as the end picture.
    // A view move's edit runs on the same key as the video, or the move stays words.
    let keyframe = match req.camera.as_deref().and_then(moves::keyframe) {
        Some(_) if !(model.end_frame && req.mode == "i2v" && image_url.is_some()) => None,
        Some(moves::Keyframe::Crop(c)) => Some(providers::KeyframePlan::Crop(c)),
        Some(moves::Keyframe::View { model: edit, prompt }) => st
            .registry
            .get(edit)
            .filter(|m| m.key == model.key && m.provider == "fal" && model.provider == "fal")
            .map(|m| providers::KeyframePlan::View { endpoint: m.endpoint.clone(), prompt: moves::view_prompt(prompt) }),
        None => None,
    };
    let sub = SubmitReq {
        endpoint: model.endpoint.clone(),
        mode: req.mode.clone(),
        prompt: final_prompt,
        aspect,
        duration,
        resolution,
        seed: req.seed,
        image_url,
        image_urls,
        voice_id,
        label: req.label.clone(),
        audio,
        elements,
        keyframe,
        end_image_url: None,
    };
    if !st.jobs.insert(job.clone(), adapter.clone(), model.key.clone()) {
        return Err(fail(StatusCode::TOO_MANY_REQUESTS, "too_many_jobs"));
    }
    jobs::spawn_submit(st.clone(), job.id.clone(), adapter, key, sub);
    Ok(job)
}

// ---------------------------------------------------------------- self-host studio

fn studio_fail(e: StudioError) -> Response {
    match e {
        StudioError::Full => fail(StatusCode::CONFLICT, "too_many_elements"),
        StudioError::Io => fail(StatusCode::INTERNAL_SERVER_ERROR, "write_failed"),
        StudioError::Stale => fail(StatusCode::CONFLICT, "edit_changed"),
    }
}

#[derive(Deserialize)]
struct UploadReq {
    /// data:<any>;base64,<bytes>. The real type is read from the bytes.
    data: String,
}

/// Saves an uploaded image, video or audio file under /outputs and returns its local URL.
async fn upload(State(st): State<AppState>, body: Result<Json<UploadReq>, JsonRejection>) -> Response {
    let Ok(Json(b)) = body else {
        return fail(StatusCode::BAD_REQUEST, "bad_request");
    };
    let Some((meta, data)) = b.data.strip_prefix("data:").and_then(|r| r.split_once(',')) else {
        return fail(StatusCode::BAD_REQUEST, "data_uri_required");
    };
    if !meta.ends_with(";base64") {
        return fail(StatusCode::BAD_REQUEST, "data_uri_required");
    }
    use base64::Engine as _;
    let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(data) else {
        return fail(StatusCode::BAD_REQUEST, "bad_base64");
    };
    if bytes.is_empty() || bytes.len() > studio::MAX_UPLOAD_BYTES {
        return fail(StatusCode::PAYLOAD_TOO_LARGE, "file_too_large");
    }
    let Some((ext, kind)) = studio::sniff(&bytes) else {
        return fail(StatusCode::UNSUPPORTED_MEDIA_TYPE, "unsupported_file");
    };
    match media::save(&st.ctx, &bytes, ext, kind).await {
        Ok(out) => (StatusCode::CREATED, Json(json!({"url": out.url, "kind": out.kind}))).into_response(),
        Err(_) => fail(StatusCode::INTERNAL_SERVER_ERROR, "write_failed"),
    }
}

async fn list_elements(State(st): State<AppState>) -> Response {
    Json(json!({"elements": st.studio.elements()})).into_response()
}

#[derive(Deserialize)]
struct ElementReq {
    tag: String,
    name: String,
    kind: String,
    images: Vec<String>,
}

/// Saves an Element (replacing one with the same tag).
async fn put_element(State(st): State<AppState>, body: Result<Json<ElementReq>, JsonRejection>) -> Response {
    let Ok(Json(b)) = body else {
        return fail(StatusCode::BAD_REQUEST, "bad_request");
    };
    let tag = b.tag.trim().to_ascii_lowercase();
    if !studio::valid_tag(&tag) {
        return fail(StatusCode::BAD_REQUEST, "invalid_tag");
    }
    let name = b.name.trim();
    if name.is_empty() || name.chars().count() > studio::MAX_NAME || name.chars().any(char::is_control) {
        return fail(StatusCode::BAD_REQUEST, "invalid_name");
    }
    if !studio::KINDS.contains(&b.kind.as_str()) {
        return fail(StatusCode::BAD_REQUEST, "invalid_kind");
    }
    if b.images.is_empty() || b.images.len() > studio::MAX_ELEMENT_IMAGES {
        return fail(StatusCode::BAD_REQUEST, "images_1_to_4");
    }
    let mut images = Vec::new();
    for u in &b.images {
        if assemble::local_file(&st.ctx.outputs_dir, u, &IMAGE_EXT).is_none() {
            return fail(StatusCode::BAD_REQUEST, "invalid_image");
        }
        if !images.contains(u) {
            images.push(u.clone());
        }
    }
    let e = Element { tag, name: name.to_string(), kind: b.kind, images, created_at: now_unix() };
    match st.studio.put_element(e.clone()) {
        Ok(()) => Json(json!({"element": e})).into_response(),
        Err(err) => studio_fail(err),
    }
}

#[derive(Deserialize)]
struct TagReq {
    tag: String,
}

async fn delete_element(State(st): State<AppState>, body: Result<Json<TagReq>, JsonRejection>) -> Response {
    let Ok(Json(b)) = body else {
        return fail(StatusCode::BAD_REQUEST, "bad_request");
    };
    match st.studio.delete_element(&b.tag.trim().to_ascii_lowercase()) {
        Ok(true) => Json(json!({"ok": true})).into_response(),
        Ok(false) => fail(StatusCode::NOT_FOUND, "not_found"),
        Err(err) => studio_fail(err),
    }
}

async fn get_project(State(st): State<AppState>) -> Response {
    Json(json!({"project": st.studio.project()})).into_response()
}

async fn set_project(State(st): State<AppState>, body: Result<Json<Project>, JsonRejection>) -> Response {
    let Ok(Json(mut p)) = body else {
        return fail(StatusCode::BAD_REQUEST, "bad_request");
    };
    p.style_prefix = p.style_prefix.chars().map(|c| if c.is_control() { ' ' } else { c }).collect::<String>().trim().to_string();
    if p.style_prefix.chars().count() > studio::MAX_STYLE_PREFIX {
        return fail(StatusCode::BAD_REQUEST, "style_prefix_too_long");
    }
    match st.studio.set_project(p.clone()) {
        Ok(()) => Json(json!({"project": p})).into_response(),
        Err(err) => studio_fail(err),
    }
}

/// A timeline clip: a bare /outputs URL (the whole clip) or a URL with a trim.
async fn get_edit(State(st): State<AppState>) -> Response {
    let (edit, rev) = st.studio.edit_rev();
    Json(json!({"edit": edit, "rev": rev})).into_response()
}

const EDIT_FRAMES: [&str; 5] = ["original", "16:9", "9:16", "1:1", "4:5"];
const MAX_CAPTION_CHARS: usize = 120;
const CAPTION_STYLES: [&str; 3] = ["off", "simple", "pop"];
const MAX_WORD_FIXES: usize = 50;

/// Saves the Agave Edit timeline. Only the shape is checked here; files are checked again at render.
#[derive(Deserialize)]
struct EditReq {
    #[serde(flatten)]
    edit: studio::EditDoc,
    /// The revision the page last read. A stale one is refused (409 edit_changed); no rev saves as before.
    #[serde(default)]
    rev: Option<u64>,
}

async fn set_edit(State(st): State<AppState>, body: Result<Json<EditReq>, JsonRejection>) -> Response {
    let Ok(Json(EditReq { edit: mut e, rev })) = body else {
        return fail(StatusCode::BAD_REQUEST, "bad_request");
    };
    if e.clips.len() > assemble::MAX_CLIPS {
        return fail(StatusCode::BAD_REQUEST, "clips_1_to_200");
    }
    if !EDIT_FRAMES.contains(&e.frame.as_str()) {
        return fail(StatusCode::BAD_REQUEST, "invalid_frame");
    }
    if !CAPTION_STYLES.contains(&e.auto_captions.as_str()) {
        return fail(StatusCode::BAD_REQUEST, "invalid_caption_style");
    }
    if e.word_fixes.len() > MAX_WORD_FIXES {
        return fail(StatusCode::BAD_REQUEST, "too_many_word_fixes");
    }
    for f in &mut e.word_fixes {
        f.from = f.from.trim().to_string();
        f.to = f.to.trim().to_string();
        let bad = |t: &str| t.chars().count() > 40 || t.chars().any(char::is_control);
        if f.from.is_empty() || f.from.contains(char::is_whitespace) || bad(&f.from) || bad(&f.to) {
            return fail(StatusCode::BAD_REQUEST, "invalid_word_fix");
        }
    }
    let local = |u: &str| u.strip_prefix("/outputs/").is_some_and(media::valid_output_name);
    let num = |t: f64| t.is_finite() && (0.0..=assemble::MAX_CLIP_SECONDS * 10.0).contains(&t);
    for c in &mut e.clips {
        if !local(&c.url) || !matches!(c.kind.as_str(), "video" | "image") {
            return fail(StatusCode::BAD_REQUEST, "invalid_clip");
        }
        if !num(c.from) || c.to.is_some_and(|t| !num(t)) || c.len.is_some_and(|t| !num(t)) {
            return fail(StatusCode::BAD_REQUEST, "invalid_trim");
        }
        c.caption = c.caption.chars().map(|ch| if ch.is_control() { ' ' } else { ch }).collect();
        if c.caption.chars().count() > MAX_CAPTION_CHARS {
            return fail(StatusCode::BAD_REQUEST, "caption_too_long");
        }
    }
    if e.song_url.as_deref().is_some_and(|u| !local(u)) {
        return fail(StatusCode::BAD_REQUEST, "invalid_audio");
    }
    e.song_name = e.song_name.map(|n| n.chars().filter(|ch| !ch.is_control()).take(120).collect());
    match st.studio.set_edit_checked(e.clone(), rev) {
        Ok(rev) => Json(json!({"edit": e, "rev": rev})).into_response(),
        Err(StudioError::Stale) => {
            let (edit, rev) = st.studio.edit_rev();
            (StatusCode::CONFLICT, Json(json!({"error": "edit_changed", "edit": edit, "rev": rev}))).into_response()
        }
        Err(err) => studio_fail(err),
    }
}

#[derive(Deserialize)]
struct SpeechReq {
    url: String,
    /// Pauses: quieter than this many dBFS (-60 to -20, default -35)...
    #[serde(default)]
    threshold_db: Option<f64>,
    /// ...for at least this many seconds (0.2 to 5, default 0.5).
    #[serde(default)]
    min_pause: Option<f64>,
}

const SPEECH_EXT: [&str; 7] = ["mp4", "mov", "webm", "mp3", "wav", "m4a", "aac"];

/// Word-timed transcript of a local video or audio file (whisper.cpp on this machine; cached).
async fn transcribe(State(st): State<AppState>, body: Result<Json<SpeechReq>, JsonRejection>) -> Response {
    let Ok(Json(b)) = body else {
        return fail(StatusCode::BAD_REQUEST, "bad_request");
    };
    let dir = &st.ctx.outputs_dir;
    let Some(name) = assemble::local_file(dir, &b.url, &SPEECH_EXT) else {
        return fail(StatusCode::BAD_REQUEST, "invalid_file");
    };
    match speech::Speech::new(dir).transcript(dir, &name).await {
        Ok(t) => Json(json!({"transcript": t})).into_response(),
        Err(speech::SpeechError::Unavailable) => fail(StatusCode::SERVICE_UNAVAILABLE, "captions_unavailable"),
        Err(speech::SpeechError::TooLong) => fail(StatusCode::BAD_REQUEST, "too_long"),
        Err(speech::SpeechError::Failed(m)) => {
            // The tool's own message can name local paths; it goes to the log, the page gets the code.
            eprintln!("transcribe: {m}");
            fail(StatusCode::BAD_GATEWAY, "transcribe_failed")
        }
    }
}

/// First setup: which speech engine is ready and how each speech model download stands.
async fn setup_status(State(st): State<AppState>) -> Response {
    let dir = &st.ctx.outputs_dir;
    let sp = speech::Speech::new(dir);
    let data = dir.parent().unwrap_or(dir);
    let engine = match sp.engine() {
        Some(speech::Engine::Parakeet) => json!("parakeet"),
        Some(speech::Engine::Whisper) => json!("whisper"),
        None => json!(null),
    };
    let models: Vec<_> = setup::MODELS
        .iter()
        .map(|m| {
            json!({"id": m.id, "name": m.name, "engine": m.engine, "languages": m.languages, "source": m.source,
                   "size": m.size(), "progress": setup::progress(m, data)})
        })
        .collect();
    Json(json!({"speech": {"engine": engine, "parakeet_tool": sp.speech_tool().is_file(), "whisper_tool": speech::whisper_tool()}, "models": models}))
        .into_response()
}

#[derive(Deserialize)]
struct SetupReq {
    model: String,
}

/// Starts (or resumes) a pinned speech model download in the background.
async fn setup_download(State(st): State<AppState>, body: Result<Json<SetupReq>, JsonRejection>) -> Response {
    let Ok(Json(b)) = body else {
        return fail(StatusCode::BAD_REQUEST, "bad_request");
    };
    let Some(m) = setup::model(&b.model) else {
        return fail(StatusCode::BAD_REQUEST, "unknown_model");
    };
    let dir = st.ctx.outputs_dir.clone();
    let data = dir.parent().unwrap_or(&dir).to_path_buf();
    let sp = speech::Speech::new(&dir);
    if m.engine == "parakeet" && !sp.speech_tool().is_file() {
        return fail(StatusCode::SERVICE_UNAVAILABLE, "speech_tool_missing");
    }
    let warm: Option<Box<dyn FnOnce() -> bool + Send>> =
        if m.engine == "parakeet" { Some(Box::new(move || speech::Speech::new(&dir).warm_parakeet())) } else { None };
    let p = setup::start(m, &data, setup::client(), warm);
    (StatusCode::ACCEPTED, Json(json!({"progress": p}))).into_response()
}

async fn setup_cancel(body: Result<Json<SetupReq>, JsonRejection>) -> Response {
    let Ok(Json(b)) = body else {
        return fail(StatusCode::BAD_REQUEST, "bad_request");
    };
    let Some(m) = setup::model(&b.model) else {
        return fail(StatusCode::BAD_REQUEST, "unknown_model");
    };
    setup::cancel(m);
    Json(json!({"ok": true})).into_response()
}

/// Pauses in a local video or audio file (ffmpeg silencedetect), for "Remove pauses".
async fn find_pauses(State(st): State<AppState>, body: Result<Json<SpeechReq>, JsonRejection>) -> Response {
    let Ok(Json(b)) = body else {
        return fail(StatusCode::BAD_REQUEST, "bad_request");
    };
    let db = b.threshold_db.unwrap_or(-35.0);
    let min = b.min_pause.unwrap_or(0.5);
    if !(-60.0..=-20.0).contains(&db) || !(0.2..=5.0).contains(&min) {
        return fail(StatusCode::BAD_REQUEST, "invalid_settings");
    }
    let dir = &st.ctx.outputs_dir;
    let Some(name) = assemble::local_file(dir, &b.url, &SPEECH_EXT) else {
        return fail(StatusCode::BAD_REQUEST, "invalid_file");
    };
    let path = dir.join(&name);
    let result = async {
        let (len, has_audio) = speech::probe_audio(&path).await?;
        let pauses = if has_audio { speech::silences(&path, db, min, len).await? } else { Vec::new() };
        Ok::<_, speech::SpeechError>((len, has_audio, pauses))
    }
    .await;
    match result {
        Ok((len, has_audio, pauses)) => {
            let pauses: Vec<[f64; 2]> = pauses.into_iter().map(|(a, b)| [a, b]).collect();
            Json(json!({"len": len, "has_audio": has_audio, "pauses": pauses})).into_response()
        }
        Err(speech::SpeechError::Unavailable) => fail(StatusCode::SERVICE_UNAVAILABLE, "ffmpeg_unavailable"),
        Err(_) => fail(StatusCode::BAD_GATEWAY, "pauses_failed"),
    }
}

#[derive(Deserialize)]
struct ReframeReq {
    url: String,
    frame: String,
}

/// The Follow camera path for one clip and output shape, for the editor's preview:
/// {"axis":"x"|"y"|"none","keys":[[source_t, pan],...],"tracked":bool}. Pan 0..1 is where the crop
/// sits across the room it can move (0 = left or top edge). `tracked` is false when the vision tool
/// is not available, so the crop stays centred.
async fn reframe_path(State(st): State<AppState>, body: Result<Json<ReframeReq>, JsonRejection>) -> Response {
    let Ok(Json(b)) = body else {
        return fail(StatusCode::BAD_REQUEST, "bad_request");
    };
    let dir = &st.ctx.outputs_dir;
    let Some((ow, oh)) = assemble::frame_size(&b.frame) else {
        return fail(StatusCode::BAD_REQUEST, "invalid_frame");
    };
    let Some(name) = assemble::video_ok(dir, &b.url) else {
        return fail(StatusCode::BAD_REQUEST, "invalid_file");
    };
    let Some((sw, sh)) = assemble::video_size(&dir.join(&name)).await else {
        return fail(StatusCode::BAD_REQUEST, "invalid_file");
    };
    let (src, out) = (f64::from(sw) / f64::from(sh.max(1)), f64::from(ow) / f64::from(oh));
    let axis = match reframe::axis(src, out) {
        reframe::Axis::X => "x",
        reframe::Axis::Y => "y",
        reframe::Axis::None => "none",
    };
    let track = reframe::Tracker::new(dir).track(dir, &name).await;
    let keys: Vec<[f64; 2]> = track.as_ref().map(|t| reframe::path(t, src, out)).unwrap_or_default().into_iter().map(|(t, p)| [t, p]).collect();
    Json(json!({"axis": axis, "keys": keys, "tracked": track.is_some()})).into_response()
}

#[derive(Deserialize)]
struct TimedCaption {
    url: String,
    start: f64,
    end: f64,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum ClipReq {
    Url(String),
    Trim {
        url: String,
        #[serde(default)]
        from: f64,
        #[serde(default)]
        to: Option<f64>,
    },
}

#[derive(Deserialize)]
struct AssembleReq {
    clips: Vec<ClipReq>,
    #[serde(default)]
    captions: Vec<Option<String>>,
    #[serde(default)]
    audio: Option<String>,
    #[serde(default = "default_true")]
    keep_clip_audio: bool,
    /// "16:9", "9:16", "1:1" or "4:5"; missing keeps the first clip's shape.
    #[serde(default)]
    frame: Option<String>,
    /// Crop to fill the frame instead of fitting with bars.
    #[serde(default)]
    fill: bool,
    /// Auto captions: caption PNGs with their times on the finished edit.
    #[serde(default)]
    timed_captions: Vec<TimedCaption>,
    /// With `fill`, the crop follows the subject.
    #[serde(default)]
    follow: bool,
    /// Take steady background noise out of the clips' own sound.
    #[serde(default)]
    clean_voice: bool,
    /// Bring the finished sound to -14 LUFS.
    #[serde(default)]
    loudness: bool,
}

/// Checks one timeline clip: a video in data/outputs, or a picture shown as a still.
fn timeline_clip(dir: &std::path::Path, c: &ClipReq) -> Option<assemble::Clip> {
    let (url, from, to) = match c {
        ClipReq::Url(u) => (u, 0.0, None),
        ClipReq::Trim { url, from, to } => (url, *from, *to),
    };
    let ok_time = |t: f64| t.is_finite() && (0.0..=assemble::MAX_CLIP_SECONDS * 10.0).contains(&t);
    if !ok_time(from) || to.is_some_and(|t| !ok_time(t) || t <= from) {
        return None;
    }
    if let Some(name) = assemble::video_ok(dir, url) {
        return Some(assemble::Clip { name, still: false, from, to });
    }
    assemble::still_ok(dir, url).map(|name| assemble::Clip { name, still: true, from, to })
}

fn default_true() -> bool {
    true
}

/// Joins finished clips into one video (a local ffmpeg job; it shows in the gallery when done).
async fn assemble_clips(State(st): State<AppState>, body: Result<Json<AssembleReq>, JsonRejection>) -> Response {
    let Ok(Json(b)) = body else {
        return fail(StatusCode::BAD_REQUEST, "bad_request");
    };
    let dir = &st.ctx.outputs_dir;
    if b.clips.is_empty() || b.clips.len() > assemble::MAX_CLIPS {
        return fail(StatusCode::BAD_REQUEST, "clips_1_to_200");
    }
    let Some(clips) = b.clips.iter().map(|c| timeline_clip(dir, c)).collect::<Option<Vec<_>>>() else {
        return fail(StatusCode::BAD_REQUEST, "invalid_clip");
    };
    let frame = match b.frame.as_deref() {
        None | Some("") | Some("original") => None,
        Some(f) => match assemble::frame_size(f) {
            Some(size) => Some(size),
            None => return fail(StatusCode::BAD_REQUEST, "invalid_frame"),
        },
    };
    if b.captions.len() > clips.len() {
        return fail(StatusCode::BAD_REQUEST, "too_many_captions");
    }
    let mut captions = Vec::new();
    for c in &b.captions {
        match c {
            Some(u) => match assemble::caption_ok(dir, u) {
                Some(n) => captions.push(Some(n)),
                None => return fail(StatusCode::BAD_REQUEST, "invalid_caption"),
            },
            None => captions.push(None),
        }
    }
    let audio = match &b.audio {
        Some(u) => match assemble::audio_ok(dir, u) {
            Some(n) => Some(n),
            None => return fail(StatusCode::BAD_REQUEST, "invalid_audio"),
        },
        None => None,
    };
    let n = clips.len();
    if b.timed_captions.len() > assemble::MAX_OVERLAYS {
        return fail(StatusCode::BAD_REQUEST, "too_many_timed_captions");
    }
    let mut overlays = Vec::with_capacity(b.timed_captions.len());
    for c in &b.timed_captions {
        let Some(png) = assemble::caption_ok(dir, &c.url) else {
            return fail(StatusCode::BAD_REQUEST, "invalid_timed_captions");
        };
        overlays.push(assemble::Overlay { png, start: c.start, end: c.end });
    }
    if !assemble::overlays_ok(&overlays) {
        return fail(StatusCode::BAD_REQUEST, "invalid_timed_captions");
    }
    let auto = if overlays.is_empty() { "" } else { " · captions" };
    let shape = b.frame.as_deref().filter(|f| frame.is_some() && !f.is_empty()).map(|f| format!(" · {f}")).unwrap_or_default();
    let extras = [(b.fill && b.follow && frame.is_some(), " · follow"), (b.clean_voice, " · clean voice"), (b.loudness, " · -14 LUFS")]
        .iter()
        .filter(|x| x.0)
        .map(|x| x.1)
        .collect::<String>();
    let plan = assemble::Plan {
        clips,
        captions,
        audio: audio.clone(),
        keep_clip_audio: b.keep_clip_audio,
        frame,
        fill: b.fill,
        overlays,
        follow: b.follow,
        clean_voice: b.clean_voice,
        loudness: b.loudness,
    };
    let sound = match (&audio, b.keep_clip_audio) {
        (Some(_), _) => "new soundtrack",
        (None, true) => "clip sound",
        (None, false) => "no sound",
    };
    let job = Job {
        id: new_job_id(),
        model: "agave/editor".into(),
        mode: "assemble".into(),
        prompt: format!("Edit of {n} clip{}{shape}{auto} · {sound}{extras}", if n == 1 { "" } else { "s" }),
        status: "queued".into(),
        progress: None,
        outputs: Vec::new(),
        error: None,
        created_at: now_unix(),
        request: None,
    };
    let adapter: std::sync::Arc<dyn providers::Provider> = std::sync::Arc::new(Assemble(plan));
    if !st.jobs.insert(job.clone(), adapter.clone(), String::new()) {
        return fail(StatusCode::TOO_MANY_REQUESTS, "too_many_jobs");
    }
    let sub = SubmitReq {
        endpoint: String::new(),
        mode: "assemble".into(),
        prompt: String::new(),
        aspect: None,
        duration: None,
        resolution: None,
        seed: None,
        image_url: None,
        image_urls: Vec::new(),
        voice_id: None,
        label: None,
        audio: None,
        elements: Vec::new(),
        keyframe: None,
        end_image_url: None,
    };
    jobs::spawn_submit(st.clone(), job.id.clone(), adapter, String::new(), sub);
    (StatusCode::ACCEPTED, Json(json!({"job": job}))).into_response()
}

async fn list_jobs(State(st): State<AppState>) -> Response {
    Json(json!({"jobs": st.jobs.list()})).into_response()
}

async fn get_job(State(st): State<AppState>, Path(id): Path<String>) -> Response {
    match st.jobs.get(&id) {
        Some(job) => Json(json!({"job": job})).into_response(),
        None => fail(StatusCode::NOT_FOUND, "not_found"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::{to_bytes, Body};
    use axum::http::Request;
    use tower::ServiceExt;

    fn post_json(uri: &str, body: &str) -> Request<Body> {
        Request::builder()
            .method("POST")
            .uri(uri)
            .header("host", "127.0.0.1:4170")
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    }

    fn get_req(uri: &str) -> Request<Body> {
        Request::builder().uri(uri).header("host", "127.0.0.1:4170").body(Body::empty()).unwrap()
    }

    fn from(addr: [u8; 4]) -> ConnectInfo<SocketAddr> {
        ConnectInfo(SocketAddr::from((addr, 5555)))
    }

    async fn body_json(resp: Response) -> Value {
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.expect("body");
        serde_json::from_slice(&bytes).expect("json")
    }

    #[tokio::test]
    async fn generate_without_key_returns_412() {
        let app = router(AppState::for_tests("api_missing_key"));
        let req = post_json(
            "/api/generate",
            r#"{"model":"fal/flux-schnell","mode":"t2i","prompt":"a red bicycle on a foggy pier"}"#,
        );
        let resp = app.oneshot(req).await.expect("response");
        assert_eq!(resp.status(), StatusCode::PRECONDITION_FAILED);
        let v = body_json(resp).await;
        assert_eq!(v["error"], "missing_key");
        assert_eq!(v["key"], "FAL_KEY");
    }

    #[tokio::test]
    async fn unimplemented_provider_returns_501() {
        let app = router(AppState::for_tests("api_501"));
        let req = post_json(
            "/api/generate",
            r#"{"model":"test/no-adapter","mode":"i2v","prompt":"a lamp","image_url":"https://example.test/a.png"}"#,
        );
        let resp = app.oneshot(req).await.expect("response");
        assert_eq!(resp.status(), StatusCode::NOT_IMPLEMENTED);
        assert_eq!(body_json(resp).await["error"], "adapter_not_implemented");
    }

    #[tokio::test]
    async fn generate_validates_before_key_check() {
        let app = router(AppState::for_tests("api_validate"));
        let cases = [
            (r#"{"model":"nope/x","mode":"t2i","prompt":"a"}"#, "unknown_model"),
            (r#"{"model":"fal/flux-schnell","mode":"t2v","prompt":"a"}"#, "unsupported_mode"),
            (r#"{"model":"fal/flux-schnell","mode":"t2i","prompt":"  "}"#, "prompt_required"),
            (r#"{"model":"fal/flux-schnell","mode":"t2i","prompt":"a","camera":"no_move"}"#, "unknown_camera"),
            (r#"{"model":"fal/flux-schnell","mode":"t2i","prompt":"a","style":"no_look"}"#, "unknown_style"),
            (r#"{"model":"fal/flux-schnell","mode":"t2i","prompt":"a","aspect":"4:3"}"#, "unsupported_aspect"),
            (r#"{"model":"fal/veo-3.1-i2v","mode":"i2v","prompt":"a"}"#, "image_url_required"),
            (r#"{"model":"elevenlabs/multilingual-v2","mode":"tts","prompt":"hi"}"#, "voice_id_required"),
        ];
        for (body, code) in cases {
            let resp = app.clone().oneshot(post_json("/api/generate", body)).await.expect("response");
            assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "{body}");
            assert_eq!(body_json(resp).await["error"], code, "{body}");
        }
    }

    #[tokio::test]
    async fn keys_post_is_loopback_only() {
        let app = router(AppState::for_tests("api_keys_remote"));
        let mut req = post_json("/api/keys", r#"{"env":"FAL_KEY","value":"abcdefgh1234"}"#);
        req.extensions_mut().insert(from([203, 0, 113, 5]));
        let resp = app.oneshot(req).await.expect("response");
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn keys_post_rejects_unknown_slot_and_writes_nothing() {
        let st = AppState::for_tests("api_keys_unknown");
        let app = router(st.clone());
        let mut req = post_json("/api/keys", r#"{"env":"PATH","value":"abcdefgh1234"}"#);
        req.extensions_mut().insert(from([127, 0, 0, 1]));
        let resp = app.oneshot(req).await.expect("response");
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        assert_eq!(body_json(resp).await["error"], "unknown_slot");
        assert!(!st.keys.is_set("PATH"));
        assert!(!st.paths.keys_file.exists());
    }

    #[tokio::test]
    async fn a_saved_key_is_never_sent_back() {
        let st = AppState::for_tests("api_keys_no_echo");
        let mut req = post_json("/api/keys", r#"{"env":"FAL_KEY","value":"abcdefgh1234secret"}"#);
        req.extensions_mut().insert(from([127, 0, 0, 1]));
        assert_eq!(router(st.clone()).oneshot(req).await.expect("response").status(), StatusCode::OK);
        let v = body_json(router(st).oneshot(get_req("/api/keys")).await.expect("response")).await;
        let slot = v["keys"].as_array().unwrap().iter().find(|k| k["env"] == "FAL_KEY").cloned().unwrap();
        assert_eq!(slot["set"], true);
        assert!(!v.to_string().contains("abcdefgh1234secret"), "a saved value is never sent back");
    }

    #[tokio::test]
    async fn keys_post_rejects_bad_value_without_echo() {
        let app = router(AppState::for_tests("api_keys_badvalue"));
        let mut req = post_json("/api/keys", r#"{"env":"FAL_KEY","value":"short"}"#);
        req.extensions_mut().insert(from([127, 0, 0, 1]));
        let resp = app.oneshot(req).await.expect("response");
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        assert_eq!(body_json(resp).await["error"], "value_length");
    }

    #[tokio::test]
    async fn a_stale_page_cannot_write_over_newer_timeline_clips() {
        let st = AppState::for_tests("api_edit_rev");
        let app = router(st.clone());
        let png = upload_png(&app).await;
        let doc = |n: usize| json!({"clips": vec![json!({"url": png, "kind": "image"}); n], "frame": "original", "auto_captions": "off"});
        let v = body_json(app.clone().oneshot(get_req("/api/edit")).await.unwrap()).await;
        assert_eq!(v["rev"], 0);
        // No rev: saves as it always did.
        let (status, v) = post(&app, "/api/edit", doc(1)).await;
        assert_eq!((status, v["rev"].as_u64()), (StatusCode::OK, Some(1)));
        // Something else (a pipeline's Add to edit) saves the timeline meanwhile.
        let mut server = st.studio.edit();
        server.clips.push(server.clips[0].clone());
        st.studio.set_edit(server).unwrap();
        // The page still holds rev 1: refused, and told what the timeline is now.
        let mut stale = doc(1);
        stale["rev"] = json!(1);
        let (status, v) = post(&app, "/api/edit", stale).await;
        assert_eq!((status, v["error"].as_str(), v["rev"].as_u64()), (StatusCode::CONFLICT, Some("edit_changed"), Some(2)));
        assert_eq!(v["edit"]["clips"].as_array().unwrap().len(), 2);
        assert_eq!(st.studio.edit().clips.len(), 2, "the newer clips were kept");
        let mut fresh = doc(3);
        fresh["rev"] = json!(2);
        let (status, v) = post(&app, "/api/edit", fresh).await;
        assert_eq!((status, v["rev"].as_u64()), (StatusCode::OK, Some(3)));
        assert_eq!(st.studio.edit().clips.len(), 3);
    }

    #[tokio::test]
    async fn models_report_ready_false_without_keys() {
        let app = router(AppState::for_tests("api_models"));
        let resp = app
            .oneshot(get_req("/api/models"))
            .await
            .expect("response");
        let v = body_json(resp).await;
        let models = v["models"].as_array().expect("array");
        assert!(models.len() >= 20);
        assert!(models.iter().all(|m| m["ready"] == false));
        assert!(models.iter().all(|m| m.get("endpoint").is_none()), "endpoint is internal");
    }

    #[tokio::test]
    async fn static_files_revalidate() {
        let app = router(AppState::for_tests("api_static_cache"));
        let req = Request::builder().uri("/").header("host", "127.0.0.1:4170").body(Body::empty()).unwrap();
        let resp = app.oneshot(req).await.expect("response");
        assert_eq!(resp.headers().get(header::CACHE_CONTROL).unwrap(), "no-cache");
    }

    #[tokio::test]
    async fn double_slash_paths_never_redirect() {
        let app = router(AppState::for_tests("api_double_slash"));
        for uri in ["//app", "//assets", "//css/", "///app"] {
            let resp = app.clone().oneshot(get_req(uri)).await.expect("response");
            assert_eq!(resp.status(), StatusCode::NOT_FOUND, "{uri}");
            assert!(resp.headers().get(header::LOCATION).is_none(), "{uri}");
        }
    }

    #[tokio::test]
    async fn guard_rejects_foreign_host_on_every_route() {
        let app = router(AppState::for_tests("api_guard_host"));
        for host in ["evil.test", "evil.test:4170", "127.0.0.1.nip.io:4170", "127.0.0.1:80", "localhost"] {
            for uri in ["/api/keys", "/api/jobs", "/", "/outputs/x.png"] {
                let req = Request::builder().uri(uri).header("host", host).body(Body::empty()).unwrap();
                let resp = app.clone().oneshot(req).await.expect("response");
                assert_eq!(resp.status(), StatusCode::FORBIDDEN, "{host} {uri}");
                assert_eq!(body_json(resp).await["error"], "bad_host", "{host} {uri}");
            }
        }
        let absolute = Request::builder()
            .uri("http://evil.test:4170/api/keys")
            .header("host", "127.0.0.1:4170")
            .body(Body::empty())
            .unwrap();
        assert_eq!(app.clone().oneshot(absolute).await.unwrap().status(), StatusCode::FORBIDDEN);
        let no_host = Request::builder().uri("/api/health").body(Body::empty()).unwrap();
        assert_eq!(app.clone().oneshot(no_host).await.unwrap().status(), StatusCode::FORBIDDEN);
        for host in ["127.0.0.1:4170", "localhost:4170", "LOCALHOST:4170"] {
            let req = Request::builder().uri("/api/health").header("host", host).body(Body::empty()).unwrap();
            assert_eq!(app.clone().oneshot(req).await.unwrap().status(), StatusCode::OK, "{host}");
        }
    }

    #[tokio::test]
    async fn guard_rejects_cross_origin_post_before_handler() {
        let st = AppState::for_tests("api_guard_origin");
        let app = router(st.clone());
        for origin in ["https://evil.test", "http://127.0.0.1:4171", "null", "http://localhost"] {
            let mut req = post_json("/api/keys", r#"{"env":"FAL_KEY","value":"abcdefgh1234"}"#);
            req.headers_mut().insert("origin", origin.parse().unwrap());
            req.extensions_mut().insert(from([127, 0, 0, 1]));
            let resp = app.clone().oneshot(req).await.expect("response");
            assert_eq!(resp.status(), StatusCode::FORBIDDEN, "{origin}");
            assert_eq!(body_json(resp).await["error"], "bad_origin", "{origin}");
        }
        assert!(!st.keys.is_set("FAL_KEY"));
        assert!(!st.paths.keys_file.exists());

        let mut req = post_json(
            "/api/generate",
            r#"{"model":"fal/flux-schnell","mode":"t2i","prompt":"a red bicycle"}"#,
        );
        req.headers_mut().insert("origin", "https://evil.test".parse().unwrap());
        let resp = app.clone().oneshot(req).await.expect("response");
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        assert!(st.jobs.list().is_empty());
    }

    /// Pages may run and call only the studio's own scripts and API; generated files never run scripts at all.
    #[tokio::test]
    async fn pages_load_scripts_and_connect_only_to_the_studio() {
        let app = router(AppState::for_tests("api_csp"));
        let csp = |path: &'static str| {
            let app = app.clone();
            async move {
                let req = Request::builder().uri(path).header("host", "127.0.0.1:4170").body(Body::empty()).unwrap();
                let resp = app.oneshot(req).await.expect("response");
                resp.headers()[header::CONTENT_SECURITY_POLICY].to_str().unwrap().to_string()
            }
        };
        let page = csp("/app/").await;
        for d in ["script-src 'self'", "connect-src 'self' blob:", "frame-src 'none'", "object-src 'none'", "frame-ancestors 'none'"] {
            assert!(page.split("; ").any(|x| x == d), "{d} missing from {page}");
        }
        assert!(csp("/outputs/a.mp4").await.starts_with("sandbox allow-same-origin; default-src 'none'"));
    }

    /// Work folders (.edit-*, .keyframe-*) and anything that is not a plain output name are never served.
    #[tokio::test]
    async fn outputs_serve_only_plain_output_names() {
        let st = AppState::for_tests("api_outputs_names");
        let dir = st.ctx.outputs_dir.clone();
        std::fs::create_dir_all(dir.join(".keyframe-abcd")).unwrap();
        std::fs::write(dir.join(".keyframe-abcd/start.jpg"), b"x").unwrap();
        std::fs::write(dir.join("a1b2.png"), b"x").unwrap();
        let app = router(st);
        let get = |path: &'static str| {
            let app = app.clone();
            async move {
                let req = Request::builder().uri(path).header("host", "127.0.0.1:4170").body(Body::empty()).unwrap();
                app.oneshot(req).await.expect("response").status()
            }
        };
        assert_eq!(get("/outputs/a1b2.png").await, StatusCode::OK);
        for path in ["/outputs/.keyframe-abcd/start.jpg", "/outputs/%2ekeyframe-abcd/start.jpg", "/outputs/.keyframe-abcd", "/outputs/"] {
            assert_eq!(get(path).await, StatusCode::NOT_FOUND, "{path}");
        }
    }

    #[tokio::test]
    async fn guard_rejects_cross_site_api_get() {
        let st = AppState::for_tests("api_guard_fetch_site");
        let app = router(st);
        let get = |site: &str| {
            Request::builder()
                .uri("/api/voices")
                .header("host", "127.0.0.1:4170")
                .header("sec-fetch-site", site)
                .body(Body::empty())
                .unwrap()
        };
        for site in ["cross-site", "same-site", "bogus"] {
            let resp = app.clone().oneshot(get(site)).await.expect("response");
            assert_eq!(resp.status(), StatusCode::FORBIDDEN, "{site}");
            assert_eq!(body_json(resp).await["error"], "bad_site", "{site}");
        }
        // The studio's own pages and typed navigations reach the handler (no ElevenLabs key set: 412).
        for site in ["same-origin", "none"] {
            let resp = app.clone().oneshot(get(site)).await.expect("response");
            assert_eq!(resp.status(), StatusCode::PRECONDITION_FAILED, "{site}");
        }
    }

    #[tokio::test]
    async fn guard_allows_same_origin_post() {
        let st = AppState::for_tests("api_guard_same_origin");
        let app = router(st.clone());
        for origin in ["http://127.0.0.1:4170", "http://localhost:4170"] {
            let mut req = post_json("/api/keys", r#"{"env":"FAL_KEY","value":"abcdefgh1234"}"#);
            req.headers_mut().insert("origin", origin.parse().unwrap());
            req.extensions_mut().insert(from([127, 0, 0, 1]));
            let resp = app.clone().oneshot(req).await.expect("response");
            assert_eq!(resp.status(), StatusCode::OK, "{origin}");
        }
        assert!(st.keys.is_set("FAL_KEY"));
    }

    #[tokio::test]
    async fn guard_requires_json_content_type_on_post() {
        let st = AppState::for_tests("api_guard_ctype");
        let app = router(st.clone());
        for ct in [None, Some("text/plain"), Some("application/x-www-form-urlencoded"), Some("multipart/form-data; boundary=x")] {
            let mut b = Request::builder().method("POST").uri("/api/keys").header("host", "127.0.0.1:4170");
            if let Some(ct) = ct {
                b = b.header("content-type", ct);
            }
            let mut req = b.body(Body::from(r#"{"env":"FAL_KEY","value":"abcdefgh1234"}"#)).unwrap();
            req.extensions_mut().insert(from([127, 0, 0, 1]));
            let resp = app.clone().oneshot(req).await.expect("response");
            assert_eq!(resp.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE, "{ct:?}");
        }
        assert!(!st.keys.is_set("FAL_KEY"));
        let mut req = post_json("/api/keys", r#"{"env":"FAL_KEY","value":"abcdefgh1234"}"#);
        req.headers_mut().insert("content-type", "application/json; charset=utf-8".parse().unwrap());
        req.extensions_mut().insert(from([127, 0, 0, 1]));
        assert_eq!(app.oneshot(req).await.unwrap().status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn generate_refuses_non_https_image_urls() {
        let app = router(AppState::for_tests("api_image_url"));
        for url in ["http://169.254.169.254/latest", "file:///etc/passwd", "ftp://x.test/a.png",
                    "/outputs/../config/keys.env", "data:image/png,notbase64", "https://user:pw@x.test/a.png"] {
            let body = json!({"model": "fal/veo-3.1-i2v", "mode": "i2v", "prompt": "a", "image_url": url}).to_string();
            let resp = app.clone().oneshot(post_json("/api/generate", &body)).await.expect("response");
            assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "{url}");
            assert_eq!(body_json(resp).await["error"], "invalid_image_url", "{url}");
        }
    }

    // ------------------------------------------------------------ studio

    const PNG: &[u8] = b"\x89PNG\r\n\x1a\nfake";

    async fn upload_png(app: &Router) -> String {
        use base64::Engine as _;
        let data = format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(PNG));
        let resp = app.clone().oneshot(post_json("/api/uploads", &json!({"data": data}).to_string())).await.unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let v = body_json(resp).await;
        assert_eq!(v["kind"], "image");
        v["url"].as_str().unwrap().to_string()
    }

    async fn post(app: &Router, uri: &str, body: Value) -> (StatusCode, Value) {
        let resp = app.clone().oneshot(post_json(uri, &body.to_string())).await.unwrap();
        let status = resp.status();
        (status, body_json(resp).await)
    }

    #[tokio::test]
    async fn uploads_keep_real_images_and_refuse_everything_else() {
        let app = router(AppState::for_tests("api_upload"));
        let url = upload_png(&app).await;
        assert!(url.starts_with("/outputs/") && url.ends_with(".png"), "{url}");
        let svg = "data:image/png;base64,PHN2ZyBvbmxvYWQ9YWxlcnQoMSk+"; // "<svg onload=alert(1)>" sent as a png
        let (s, v) = post(&app, "/api/uploads", json!({"data": svg})).await;
        assert_eq!((s, v["error"].as_str()), (StatusCode::UNSUPPORTED_MEDIA_TYPE, Some("unsupported_file")));
        let (s, v) = post(&app, "/api/uploads", json!({"data": "https://example.test/a.png"})).await;
        assert_eq!((s, v["error"].as_str()), (StatusCode::BAD_REQUEST, Some("data_uri_required")));
    }

    #[tokio::test]
    async fn elements_turn_tags_into_references() {
        let app = router(AppState::for_tests("api_elements"));
        let img = upload_png(&app).await;
        let (s, _) = post(&app, "/api/elements", json!({"tag": "Mia", "name": "Mia", "kind": "character", "images": [img]})).await;
        assert_eq!(s, StatusCode::OK);
        let (s, v) = post(&app, "/api/elements", json!({"tag": "x", "name": "X", "kind": "character", "images": ["/outputs/nope.png"]})).await;
        assert_eq!((s, v["error"].as_str()), (StatusCode::BAD_REQUEST, Some("invalid_image")));
        // A tag on a text-only model is refused: its images need an image mode.
        let (s, v) = post(&app, "/api/generate", json!({"model": "fal/flux-schnell", "mode": "t2i", "prompt": "@mia in a cafe"})).await;
        assert_eq!((s, v["error"].as_str()), (StatusCode::BAD_REQUEST, Some("refs_need_image_mode")));
        // On a multi-reference model the tag alone is enough reference; it reaches the key check.
        let (s, v) = post(&app, "/api/generate", json!({"model": "fal/nano-banana-edit", "mode": "i2i", "prompt": "@mia in a cafe"})).await;
        assert_eq!((s, v["error"].as_str()), (StatusCode::PRECONDITION_FAILED, Some("missing_key")));
        // Five references are more than the model takes.
        let five: Vec<String> = (0..5).map(|i| format!("https://example.test/{i}.png")).collect();
        let (s, v) = post(&app, "/api/generate", json!({"model": "fal/nano-banana-edit", "mode": "i2i", "prompt": "p", "image_urls": five})).await;
        assert_eq!((s, v["error"].as_str(), v["max"].as_u64()), (StatusCode::BAD_REQUEST, Some("too_many_refs"), Some(4)));
        // Kling 3.0 takes a start frame and the tagged Elements side by side (fal `elements`): no longer one
        // picture too many. Without a start frame the Element's picture is the start frame, as before.
        let kling = |body: Value| post(&app, "/api/generate", body);
        let start = json!({"model": "fal/kling-3-pro-i2v", "mode": "i2v", "prompt": "@mia walks the pier", "image_url": "https://example.test/start.png"});
        let (s, v) = kling(start).await;
        assert_eq!((s, v["error"].as_str()), (StatusCode::PRECONDITION_FAILED, Some("missing_key")));
        let (s, v) = kling(json!({"model": "fal/kling-3-pro-i2v", "mode": "i2v", "prompt": "@mia walks the pier"})).await;
        assert_eq!((s, v["error"].as_str()), (StatusCode::PRECONDITION_FAILED, Some("missing_key")));
        for t in ["b", "c", "d", "e"] {
            let (s, _) = post(&app, "/api/elements", json!({"tag": t, "name": t, "kind": "character", "images": [img]})).await;
            assert_eq!(s, StatusCode::OK);
        }
        let five = json!({"model": "fal/kling-3-pro-i2v", "mode": "i2v", "prompt": "@mia @b @c @d @e", "image_url": "https://example.test/start.png"});
        let (s, v) = kling(five).await;
        assert_eq!((s, v["error"].as_str(), v["max"].as_u64()), (StatusCode::BAD_REQUEST, Some("too_many_element_tags"), Some(4)));
        // Kling 2.5 Turbo takes no Elements: the start frame and the tag's picture are two pictures for a one-picture model.
        let turbo = json!({"model": "fal/kling-2.5-turbo-i2v", "mode": "i2v", "prompt": "@mia walks", "image_url": "https://example.test/start.png"});
        let (s, v) = kling(turbo).await;
        assert_eq!((s, v["error"].as_str()), (StatusCode::BAD_REQUEST, Some("too_many_refs")));
        for t in ["b", "c", "d", "e"] {
            let (s, _) = post(&app, "/api/elements/delete", json!({"tag": t})).await;
            assert_eq!(s, StatusCode::OK);
        }
        let (s, _) = post(&app, "/api/elements/delete", json!({"tag": "mia"})).await;
        assert_eq!(s, StatusCode::OK);
        let resp = app.clone().oneshot(get_req("/api/elements")).await.unwrap();
        assert_eq!(body_json(resp).await["elements"], json!([]));
    }

    #[tokio::test]
    async fn presets_say_which_camera_moves_are_keyframed() {
        let app = router(AppState::for_tests("api_presets_keyframes"));
        let resp = app.clone().oneshot(get_req("/api/presets")).await.unwrap();
        let v = body_json(resp).await;
        let cam = |id: &str| v["camera"].as_array().unwrap().iter().find(|c| c["id"] == id).cloned().unwrap();
        assert_eq!(cam("dolly_in")["keyframe"], json!({"kind": "crop"}));
        assert_eq!(cam("dolly_out")["keyframe"], json!({"kind": "view", "model": "fal/nano-banana-edit"}));
        assert_eq!(cam("orbit_left")["keyframe"]["model"], "fal/seedream-4-edit");
        assert!(cam("crane_up")["keyframe"].is_null(), "crane up stays words");
        for c in v["camera"].as_array().unwrap() {
            if let Some(m) = c["keyframe"]["model"].as_str() {
                let (s, models) = (StatusCode::OK, body_json(app.clone().oneshot(get_req("/api/models")).await.unwrap()).await);
                assert_eq!(s, StatusCode::OK);
                assert!(models["models"].as_array().unwrap().iter().any(|x| x["id"] == m && x["price"]["usd"].is_number()), "{m} must be a priced model");
            }
        }
    }

    #[tokio::test]
    async fn still_first_blocks_text_to_video_until_turned_off() {
        let app = router(AppState::for_tests("api_still_first"));
        let (s, _) = post(&app, "/api/project", json!({"still_first": true})).await;
        assert_eq!(s, StatusCode::OK);
        let t2v = json!({"model": "fal/veo-3.1-t2v", "mode": "t2v", "prompt": "a lighthouse at dusk"});
        let (s, v) = post(&app, "/api/generate", t2v.clone()).await;
        assert_eq!((s, v["error"].as_str()), (StatusCode::CONFLICT, Some("still_first")));
        let i2v = json!({"model": "fal/veo-3.1-i2v", "mode": "i2v", "prompt": "slow push in", "image_url": "https://example.test/a.png"});
        let (s, _) = post(&app, "/api/generate", i2v).await;
        assert_eq!(s, StatusCode::PRECONDITION_FAILED, "a video from a still is allowed");
        let (s, v) = post(&app, "/api/project", json!({"style_prefix": "35mm film,\nsoft light", "still_first": false})).await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(v["project"]["style_prefix"], "35mm film, soft light");
        let (s, _) = post(&app, "/api/generate", t2v).await;
        assert_eq!(s, StatusCode::PRECONDITION_FAILED);
    }

    #[tokio::test]
    async fn meshy_3d_needs_pictures_but_no_words() {
        let app = router(AppState::for_tests("api_meshy"));
        let pic = |n: usize| (0..n).map(|i| format!("https://example.test/{i}.png")).collect::<Vec<_>>();
        let (s, v) = post(&app, "/api/generate", json!({"model": "meshy/image-to-3d", "mode": "i23d", "prompt": ""})).await;
        assert_eq!((s, v["error"].as_str()), (StatusCode::BAD_REQUEST, Some("image_url_required")));
        let (s, v) = post(&app, "/api/generate", json!({"model": "meshy/image-to-3d", "mode": "i23d", "prompt": "", "image_urls": pic(5)})).await;
        assert_eq!((s, v["error"].as_str(), v["max"].as_u64()), (StatusCode::BAD_REQUEST, Some("too_many_refs"), Some(4)));
        // Words are optional; with no Meshy key the request stops at the key check, before any provider call.
        let (s, v) = post(&app, "/api/generate", json!({"model": "meshy/image-to-3d", "mode": "i23d", "prompt": "", "image_urls": pic(4)})).await;
        assert_eq!((s, v["key"].as_str()), (StatusCode::PRECONDITION_FAILED, Some("MESHY_API_KEY")));
    }

    #[tokio::test]
    async fn recipes_need_a_known_name_and_edits_need_a_change() {
        let app = router(AppState::for_tests("api_recipes"));
        let base = |recipe: &str, prompt: &str| {
            json!({"model": "fal/nano-banana-edit", "mode": "i2i", "prompt": prompt, "recipe": recipe, "image_url": "https://example.test/a.png"})
        };
        let (s, v) = post(&app, "/api/generate", base("edit", "")).await;
        assert_eq!((s, v["error"].as_str()), (StatusCode::BAD_REQUEST, Some("prompt_required")));
        let (s, v) = post(&app, "/api/generate", base("collage", "x")).await;
        assert_eq!((s, v["error"].as_str()), (StatusCode::BAD_REQUEST, Some("unknown_recipe")));
        let (s, _) = post(&app, "/api/generate", base("grid4", "")).await;
        assert_eq!(s, StatusCode::PRECONDITION_FAILED, "a grid needs no extra words");
    }

    #[tokio::test]
    async fn assemble_checks_every_file() {
        let app = router(AppState::for_tests("api_assemble"));
        let img = upload_png(&app).await;
        let (s, v) = post(&app, "/api/assemble", json!({"clips": ["/outputs/missing.mp4"]})).await;
        assert_eq!((s, v["error"].as_str()), (StatusCode::BAD_REQUEST, Some("invalid_clip")));
        let (s, v) = post(&app, "/api/assemble", json!({"clips": []})).await;
        assert_eq!((s, v["error"].as_str()), (StatusCode::BAD_REQUEST, Some("clips_1_to_200")));
        let (s, v) = post(&app, "/api/assemble", json!({"clips": [{"url": img, "from": 2.0, "to": 1.0}]})).await;
        assert_eq!((s, v["error"].as_str()), (StatusCode::BAD_REQUEST, Some("invalid_clip")), "a trim must run forwards");
        let (s, v) = post(&app, "/api/assemble", json!({"clips": [{"url": img, "to": 2.0}], "frame": "2:1"})).await;
        assert_eq!((s, v["error"].as_str()), (StatusCode::BAD_REQUEST, Some("invalid_frame")));
        // A picture is a still on the timeline.
        let (s, v) = post(&app, "/api/assemble", json!({"clips": [{"url": img, "to": 2.0}], "frame": "9:16", "fill": true})).await;
        assert_eq!(s, StatusCode::ACCEPTED);
        assert_eq!(v["job"]["prompt"].as_str(), Some("Edit of 1 clip · 9:16 · clip sound"));
    }

    #[tokio::test]
    async fn the_edit_timeline_is_saved_and_checked() {
        let app = router(AppState::for_tests("api_edit"));
        let clip = json!({"url": "/outputs/abc123.mp4", "kind": "video", "from": 1.0, "to": 3.0, "len": 6.0, "caption": "Hello"});
        let (s, v) = post(&app, "/api/edit", json!({"clips": [clip], "frame": "9:16", "fill": true, "keep_sound": true})).await;
        assert_eq!(s, StatusCode::OK, "{v}");
        let resp = app.clone().oneshot(get_req("/api/edit")).await.unwrap();
        let s = resp.status();
        let v = body_json(resp).await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(v["edit"]["clips"][0]["to"].as_f64(), Some(3.0));
        assert_eq!(v["edit"]["frame"].as_str(), Some("9:16"));
        for (bad, err) in [
            (json!({"clips": [{"url": "https://x.test/a.mp4", "kind": "video"}], "frame": "original"}), "invalid_clip"),
            (json!({"clips": [{"url": "/outputs/../a.mp4", "kind": "video"}], "frame": "original"}), "invalid_clip"),
            (json!({"clips": [{"url": "/outputs/a.mp4", "kind": "audio"}], "frame": "original"}), "invalid_clip"),
            (json!({"clips": [{"url": "/outputs/a.mp4", "kind": "video", "from": -1.0}], "frame": "original"}), "invalid_trim"),
            (json!({"clips": [], "frame": "3:2"}), "invalid_frame"),
            (json!({"clips": [{"url": "/outputs/a.mp4", "kind": "video", "caption": "x".repeat(121)}], "frame": "original"}), "caption_too_long"),
            (json!({"clips": [], "frame": "original", "song_url": "file:///etc/passwd"}), "invalid_audio"),
        ] {
            let (s, v) = post(&app, "/api/edit", bad).await;
            assert_eq!((s, v["error"].as_str()), (StatusCode::BAD_REQUEST, Some(err)));
        }
    }

    #[tokio::test]
    async fn speech_tools_take_only_local_media_and_sane_settings() {
        let app = router(AppState::for_tests("api_speech"));
        let img = upload_png(&app).await;
        for uri in ["/api/transcribe", "/api/pauses"] {
            for url in ["https://example.test/a.mp4", "/outputs/../a.mp4", "/outputs/missing.mp4", img.as_str()] {
                let (s, v) = post(&app, uri, json!({"url": url})).await;
                assert_eq!((s, v["error"].as_str()), (StatusCode::BAD_REQUEST, Some("invalid_file")), "{uri} {url}");
            }
        }
        let (s, v) = post(&app, "/api/pauses", json!({"url": "/outputs/a.mp4", "threshold_db": -5.0})).await;
        assert_eq!((s, v["error"].as_str()), (StatusCode::BAD_REQUEST, Some("invalid_settings")));
        let (s, v) = post(&app, "/api/pauses", json!({"url": "/outputs/a.mp4", "min_pause": 0.0})).await;
        assert_eq!((s, v["error"].as_str()), (StatusCode::BAD_REQUEST, Some("invalid_settings")));
        // Timed captions must be local PNGs that run forwards without overlapping.
        let clip = json!([{"url": img, "to": 2.0}]);
        let (s, v) = post(&app, "/api/assemble", json!({"clips": clip, "timed_captions": [{"url": img, "start": 0.5, "end": 1.0}, {"url": img, "start": 0.8, "end": 1.2}]})).await;
        assert_eq!((s, v["error"].as_str()), (StatusCode::BAD_REQUEST, Some("invalid_timed_captions")));
        let (s, v) = post(&app, "/api/assemble", json!({"clips": clip, "timed_captions": [{"url": "/outputs/nope.png", "start": 0.0, "end": 1.0}]})).await;
        assert_eq!((s, v["error"].as_str()), (StatusCode::BAD_REQUEST, Some("invalid_timed_captions")));
        let (s, v) = post(&app, "/api/assemble", json!({"clips": clip, "timed_captions": [{"url": img, "start": 0.0, "end": 1.0}, {"url": img, "start": 1.0, "end": 1.5}]})).await;
        assert_eq!(s, StatusCode::ACCEPTED);
        assert_eq!(v["job"]["prompt"].as_str(), Some("Edit of 1 clip · captions · clip sound"));
        for (body, err) in [
            (json!({"url": "/outputs/a.mp4", "frame": "2:1"}), "invalid_frame"),
            (json!({"url": "/outputs/missing.mp4", "frame": "9:16"}), "invalid_file"),
            (json!({"url": img, "frame": "9:16"}), "invalid_file"),
            (json!({"url": "https://example.com/a.mp4", "frame": "9:16"}), "invalid_file"),
            (json!({"url": "/outputs/../config/keys.env", "frame": "9:16"}), "invalid_file"),
        ] {
            let (s, v) = post(&app, "/api/reframe", body).await;
            assert_eq!((s, v["error"].as_str()), (StatusCode::BAD_REQUEST, Some(err)));
        }
        let (s, v) = post(&app, "/api/assemble", json!({"clips": clip, "frame": "9:16", "fill": true, "follow": true, "clean_voice": true, "loudness": true})).await;
        assert_eq!(s, StatusCode::ACCEPTED);
        assert_eq!(v["job"]["prompt"].as_str(), Some("Edit of 1 clip · 9:16 · clip sound · follow · clean voice · -14 LUFS"));
        let (s, v) = post(&app, "/api/edit", json!({"clips": [], "frame": "original", "auto_captions": "karaoke"})).await;
        assert_eq!((s, v["error"].as_str()), (StatusCode::BAD_REQUEST, Some("invalid_caption_style")));
        for fix in [json!({"from": "two words", "to": "x"}), json!({"from": "", "to": "x"}), json!({"from": "a", "to": "y".repeat(41)})] {
            let (s, v) = post(&app, "/api/edit", json!({"clips": [], "frame": "original", "word_fixes": [fix]})).await;
            assert_eq!((s, v["error"].as_str()), (StatusCode::BAD_REQUEST, Some("invalid_word_fix")));
        }
        let (s, v) = post(&app, "/api/edit", json!({"clips": [], "frame": "original", "auto_captions": "pop", "word_fixes": [{"from": " bamboo ", "to": "Bambu"}]})).await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(v["edit"]["word_fixes"][0]["from"].as_str(), Some("bamboo"));
    }

    #[tokio::test]
    async fn jobs_keep_their_request_for_run_again() {
        let st = AppState::for_tests("api_request_kept");
        let app = router(st.clone());
        st.keys.set("FAL_KEY", "test-key-not-real").unwrap();
        let (s, _) = post(&app, "/api/project", json!({"still_first": false})).await;
        assert_eq!(s, StatusCode::OK);
        // test/offline runs on the always-failing test provider, so nothing leaves the machine.
        let (s, v) = post(&app, "/api/generate", json!({"model": "test/offline", "mode": "t2v", "prompt": "a red bicycle", "seed": 7})).await;
        assert_eq!(s, StatusCode::ACCEPTED);
        assert_eq!(v["job"]["request"]["prompt"], "a red bicycle");
        assert_eq!(v["job"]["request"]["seed"], 7);
        assert!(v["job"]["request"].get("recipe").is_none(), "unset extras are left out");
    }
}
