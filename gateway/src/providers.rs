//! Provider adapters. Each adapter submits a job, polls it, and saves finished files locally.
//! Keys are passed in per call and only ever sent to the provider's own host.
//! Endpoint shapes come from each provider's public API docs. Unverified parts are marked UNVERIFIED.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use base64::Engine as _;
use serde_json::{json, Value};

use crate::media::{self, ext_from_url, net, Ctx, Output};
use crate::models::ModelDef;

#[derive(Debug)]
pub enum ProviderError {
    NotImplemented,
    /// Request-level failure. The poller retries a few times before it fails the job.
    Failed(String),
}

pub type PResult<T> = Result<T, ProviderError>;

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Handle {
    #[allow(dead_code)]
    pub request_id: String,
    pub status_url: String,
    pub result_url: Option<String>,
}

#[derive(Clone, Debug)]
pub struct SubmitReq {
    pub endpoint: String,
    #[allow(dead_code)]
    pub mode: String,
    pub prompt: String,
    pub aspect: Option<String>,
    pub duration: Option<u32>,
    pub resolution: Option<String>,
    pub seed: Option<i64>,
    pub image_url: Option<String>,
    /// Several reference images, for models that take a list (models.toml `max_refs`).
    pub image_urls: Vec<String>,
    pub voice_id: Option<String>,
    /// What the request is for, in a few words ("Step 3 · Video"). Only test mode shows it, on its placeholders.
    pub label: Option<String>,
    /// Native audio on or off, for models that list it (models.toml params.audio). None: the model's default.
    pub audio: Option<bool>,
    /// Kling 3.0 Elements: the pictures of each @tagged Element (main view first), called @Element1, @Element2...
    pub elements: Vec<Vec<String>>,
    /// A keyframed camera move (moves.rs) on a model that takes an end picture: how the end picture is made.
    pub keyframe: Option<KeyframePlan>,
    /// The end picture (fal `end_image_url`), made from the start picture when keyframe is set.
    pub end_image_url: Option<String>,
}

/// How Fal::submit makes the end picture of a keyframed camera move.
#[derive(Clone, Debug, PartialEq)]
pub enum KeyframePlan {
    /// The middle of the start picture at this fraction, scaled back (assemble::keyframe_frames).
    Crop(f64),
    /// An image edit of the start picture on fal: the edit model's endpoint and the full prompt.
    View { endpoint: String, prompt: String },
}

pub enum Submitted {
    Queued(Handle),
    Done(Vec<Output>),
}

pub enum Polled {
    Running(Option<f64>),
    Done(Vec<Output>),
    /// Terminal failure reported by the provider.
    Failed(String),
}

#[async_trait]
pub trait Provider: Send + Sync {
    fn poll_interval(&self) -> Duration {
        Duration::from_secs(3)
    }

    async fn submit(&self, ctx: &Ctx, key: &str, req: &SubmitReq) -> PResult<Submitted>;

    /// Only needed by adapters that return `Submitted::Queued`.
    async fn poll(&self, _ctx: &Ctx, _key: &str, _h: &Handle) -> PResult<Polled> {
        Err(ProviderError::NotImplemented)
    }
}

/// Adapter for a model, or None when the provider has no adapter yet (served as 501).
pub fn adapter_for(m: &ModelDef) -> Option<Arc<dyn Provider>> {
    Some(match (m.provider.as_str(), m.modality.as_str()) {
        ("fal", _) => Arc::new(Fal),
        ("replicate", _) => Arc::new(Replicate),
        ("openai", "image") => Arc::new(OpenAiImage),
        ("gemini", "video") => Arc::new(GeminiVeo),
        ("gemini", "image") => Arc::new(GeminiImage),
        ("bfl", _) => Arc::new(Bfl),
        ("elevenlabs", "audio") => Arc::new(ElevenLabs),
        ("meshy", "3d") => Arc::new(Meshy),
        #[cfg(test)]
        ("test", _) => Arc::new(TestFail),
        _ => return None,
    })
}

/// Stands in for the provider of a job restored as failed; it is never polled.
pub struct Stopped;

#[async_trait]
impl Provider for Stopped {
    async fn submit(&self, _ctx: &Ctx, _key: &str, _req: &SubmitReq) -> PResult<Submitted> {
        Err(ProviderError::NotImplemented)
    }
}

/// Test-only provider: every submit fails, so route tests never reach a real API.
#[cfg(test)]
pub struct TestFail;

#[cfg(test)]
#[async_trait]
impl Provider for TestFail {
    async fn submit(&self, _ctx: &Ctx, _key: &str, _req: &SubmitReq) -> PResult<Submitted> {
        Err(ProviderError::Failed("test provider always fails".into()))
    }
}

/// ElevenLabs voice ids go into a URL path, so only a strict charset is allowed.
pub fn valid_voice(v: &str) -> bool {
    !v.is_empty() && v.len() <= 64 && v.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

const GEMINI_BASE: &str = "https://generativelanguage.googleapis.com/v1beta";
const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD;

/// First 300 characters, with control characters flattened to spaces.
fn excerpt(s: &str) -> String {
    s.chars().take(300).map(|c| if c.is_control() { ' ' } else { c }).collect()
}

/// Error bodies are cut to a short excerpt; jobs.rs also scrubs the key from every error string.
async fn error_excerpt(status: reqwest::StatusCode, resp: reqwest::Response) -> ProviderError {
    let body = media::read_capped(resp, 16 << 10).await.unwrap_or_default();
    let text = String::from_utf8_lossy(&body);
    ProviderError::Failed(format!("provider HTTP {}: {}", status.as_u16(), excerpt(&text)))
}

async fn json_body(resp: reqwest::Response) -> PResult<Value> {
    let status = resp.status();
    if !status.is_success() {
        return Err(error_excerpt(status, resp).await);
    }
    let bytes = media::read_capped(resp, media::MAX_JSON_BYTES).await?;
    serde_json::from_slice(&bytes).map_err(|_| ProviderError::Failed("provider returned non-JSON".into()))
}

async fn bytes_body(resp: reqwest::Response) -> PResult<Vec<u8>> {
    let status = resp.status();
    if !status.is_success() {
        return Err(error_excerpt(status, resp).await);
    }
    media::read_capped(resp, media::MAX_DOWNLOAD_BYTES).await
}

pub(crate) fn str_at(v: &Value, key: &str) -> PResult<String> {
    v.get(key)
        .and_then(Value::as_str)
        .map(String::from)
        .ok_or_else(|| ProviderError::Failed(format!("provider response missing {key}")))
}

/// Refuses to send a key to a URL outside the provider's own domain.
pub(crate) fn ensure_host(url: &str, suffix: &str) -> PResult<()> {
    let u = reqwest::Url::parse(url).map_err(|_| ProviderError::Failed("provider sent a bad URL".into()))?;
    let host = u.host_str().unwrap_or("");
    let ok = u.scheme() == "https" && (host == suffix || host.ends_with(&format!(".{suffix}")));
    if ok {
        Ok(())
    } else {
        Err(ProviderError::Failed(format!("refusing provider URL outside {suffix}")))
    }
}

fn default_ext(kind: &str) -> &'static str {
    match kind {
        "video" => "mp4",
        "audio" => "mp3",
        _ => "png",
    }
}

// ------------------------------------------------------------------ fal.ai (queue API)

pub struct Fal;

fn fal_input(r: &SubmitReq) -> Value {
    let aspect = r.aspect.as_deref().unwrap_or("16:9");
    // FLUX and Seedream 4 take a named image_size instead of aspect_ratio (fal API schemas, 2026-10-11).
    let mut body = if r.endpoint.starts_with("fal-ai/flux/") || r.endpoint.starts_with("fal-ai/bytedance/seedream/") {
        // No aspect on an edit (a look): Seedream Edit sizes the result from the picture ("auto", fal schema 2026-10-19).
        let size = match aspect {
            "9:16" => "portrait_16_9",
            "1:1" => "square_hd",
            _ if r.aspect.is_none() && r.endpoint.ends_with("/edit") => "auto",
            _ => "landscape_16_9",
        };
        let mut b = json!({"prompt": r.prompt, "image_size": size});
        if !r.image_urls.is_empty() {
            b["image_urls"] = json!(r.image_urls);
        }
        b
    } else {
        // Generic video shape: duration as a string (Kling and Seedance enums are strings).
        // aspect_ratio only when the model lists aspects (image to video takes the frame from the image).
        let mut b = json!({"prompt": r.prompt});
        if let Some(a) = &r.aspect {
            b["aspect_ratio"] = json!(a);
        }
        // Veo 3.1 takes "8s"; Wan 2.2 has no duration field (81 frames, about 5 s) (fal API schemas, 2026-10-11).
        if let Some(d) = r.duration {
            if r.endpoint.starts_with("fal-ai/veo3") {
                b["duration"] = json!(format!("{d}s"));
            } else if !r.endpoint.starts_with("fal-ai/wan/") {
                b["duration"] = json!(d.to_string());
            }
        }
        if let Some(res) = &r.resolution {
            b["resolution"] = json!(res);
        }
        // Kling 3.0, Kling O3 and Veo 3.1 bill native audio separately (fal pages, 2026-10-19).
        if let Some(a) = r.audio {
            b["generate_audio"] = json!(a);
        }
        if let Some(img) = &r.image_url {
            // Kling 3.0 image to video names the field start_image_url (fal API schema, 2026-10-11).
            let field = if r.endpoint.starts_with("fal-ai/kling-video/v3/") { "start_image_url" } else { "image_url" };
            b[field] = json!(img);
        }
        // Multi-reference edit models (Nano Banana edit, Seedream edit) take image_urls.
        if !r.image_urls.is_empty() {
            b["image_urls"] = json!(r.image_urls);
        }
        if let Some(end) = &r.end_image_url {
            b["end_image_url"] = json!(end);
        }
        // Kling 3.0 Elements: the main view, then 1-3 more pictures (the main view again when there is only one).
        if !r.elements.is_empty() {
            let els: Vec<Value> = r
                .elements
                .iter()
                .filter_map(|imgs| {
                    let front = imgs.first()?;
                    let more: Vec<&String> = if imgs.len() > 1 { imgs[1..].iter().take(3).collect() } else { vec![front] };
                    Some(json!({"frontal_image_url": front, "reference_image_urls": more}))
                })
                .collect();
            b["elements"] = json!(els);
        }
        b
    };
    if let Some(seed) = r.seed {
        body["seed"] = json!(seed);
    }
    body
}

fn fal_outputs(v: &Value) -> Vec<(String, &'static str)> {
    let mut out = Vec::new();
    if let Some(u) = v.pointer("/video/url").and_then(Value::as_str) {
        out.push((u.to_string(), "video"));
    }
    if let Some(arr) = v.get("images").and_then(Value::as_array) {
        for img in arr {
            if let Some(u) = img.get("url").and_then(Value::as_str) {
                out.push((u.to_string(), "image"));
            }
        }
    }
    if let Some(u) = v.pointer("/image/url").and_then(Value::as_str) {
        out.push((u.to_string(), "image"));
    }
    for p in ["/audio/url", "/audio_file/url"] {
        if let Some(u) = v.pointer(p).and_then(Value::as_str) {
            out.push((u.to_string(), "audio"));
            break;
        }
    }
    out
}

#[async_trait]
impl Provider for Fal {
    async fn submit(&self, ctx: &Ctx, key: &str, req: &SubmitReq) -> PResult<Submitted> {
        let mut r = req.clone();
        // A keyframed camera move (moves.rs): the end picture carries the move, because Kling ignores the words.
        if let (Some(plan), Some(start)) = (r.keyframe.clone(), r.image_url.clone()) {
            match plan {
                KeyframePlan::Crop(c) => {
                    let (first, last) = crate::assemble::keyframe_frames(ctx, &start, crate::assemble::EndFrom::Crop(c)).await?;
                    r.image_url = Some(first);
                    r.end_image_url = Some(last);
                }
                KeyframePlan::View { endpoint, prompt } => {
                    // The edit and the video start from the same upright frame, so the two pictures match.
                    let upright = crate::assemble::upright_picture(ctx, &start).await?;
                    let end = fal_view_end(ctx, key, &endpoint, &prompt, &upright).await?;
                    let frames = crate::assemble::keyframe_frames(ctx, &upright, crate::assemble::EndFrom::Picture(&end)).await;
                    // The edit's own file is only a step on the way: it is inlined (or failed) and not kept.
                    if let Some(name) = end.strip_prefix("/outputs/").filter(|n| media::valid_output_name(n)) {
                        let _ = tokio::fs::remove_file(ctx.outputs_dir.join(name)).await;
                    }
                    let (first, last) = frames?;
                    r.image_url = Some(first);
                    r.end_image_url = Some(last);
                }
            }
        }
        // fal cannot read our local /outputs paths, so inline them.
        if let Some(img) = r.image_url.clone() {
            if img.starts_with("/outputs/") {
                r.image_url = Some(media::as_data_uri(ctx, &img).await?);
            }
        }
        for img in r.image_urls.iter_mut().chain(r.elements.iter_mut().flatten()) {
            if img.starts_with("/outputs/") {
                *img = media::as_data_uri(ctx, img).await?;
            }
        }
        let url = format!("https://queue.fal.run/{}", r.endpoint);
        let resp = ctx
            .http
            .post(url.as_str())
            .header("Authorization", format!("Key {key}"))
            .json(&fal_input(&r))
            .send()
            .await
            .map_err(net)?;
        let v = json_body(resp).await?;
        let status_url = str_at(&v, "status_url")?;
        let response_url = str_at(&v, "response_url")?;
        ensure_host(&status_url, "queue.fal.run")?;
        ensure_host(&response_url, "queue.fal.run")?;
        Ok(Submitted::Queued(Handle {
            request_id: str_at(&v, "request_id")?,
            status_url,
            result_url: Some(response_url),
        }))
    }

    async fn poll(&self, ctx: &Ctx, key: &str, h: &Handle) -> PResult<Polled> {
        let auth = format!("Key {key}");
        let resp = ctx
            .http
            .get(h.status_url.as_str())
            .header("Authorization", auth.as_str())
            .send()
            .await
            .map_err(net)?;
        let v = json_body(resp).await?;
        match v.get("status").and_then(Value::as_str).unwrap_or("") {
            "IN_QUEUE" | "IN_PROGRESS" => Ok(Polled::Running(None)),
            "COMPLETED" => {
                if v.get("error").is_some_and(|e| !e.is_null()) {
                    return Ok(Polled::Failed("fal reported an error".into()));
                }
                let result_url = h
                    .result_url
                    .as_deref()
                    .ok_or_else(|| ProviderError::Failed("missing fal result URL".into()))?;
                let resp = ctx
                    .http
                    .get(result_url)
                    .header("Authorization", auth.as_str())
                    .send()
                    .await
                    .map_err(net)?;
                let result = json_body(resp).await?;
                let mut outs = Vec::new();
                for (url, kind) in fal_outputs(&result) {
                    let (bytes, _) = media::fetch(&ctx.http, &url, &[]).await?;
                    let ext = ext_from_url(&url, default_ext(kind));
                    outs.push(media::save(ctx, &bytes, &ext, kind).await?);
                }
                if outs.is_empty() {
                    return Ok(Polled::Failed("fal returned no output file".into()));
                }
                Ok(Polled::Done(outs))
            }
            other => Ok(Polled::Failed(format!("fal status {other}"))),
        }
    }
}

/// The end picture of a view move (moves.rs): an image edit of the start picture on fal, on the same key, waited for
/// here (up to 3 minutes; like the job poller it gives up on the 5th poll error in a row) before the video is sent.
/// Returns the edited picture, saved under /outputs. Once queued the edit is charged by fal even if this then fails.
async fn fal_view_end(ctx: &Ctx, key: &str, endpoint: &str, prompt: &str, start: &str) -> PResult<String> {
    let failed = |why: &str| ProviderError::Failed(format!("The camera move could not make its end picture: {why}"));
    let edit = SubmitReq {
        endpoint: endpoint.to_string(),
        mode: "i2i".into(),
        prompt: prompt.to_string(),
        aspect: None,
        duration: None,
        resolution: None,
        seed: None,
        image_url: None,
        image_urls: vec![start.to_string()],
        voice_id: None,
        label: None,
        audio: None,
        elements: Vec::new(),
        keyframe: None,
        end_image_url: None,
    };
    let Submitted::Queued(handle) = Fal.submit(ctx, key, &edit).await? else {
        return Err(failed("the edit was not queued"));
    };
    let deadline = tokio::time::Instant::now() + Duration::from_secs(180);
    let mut errors = 0;
    loop {
        tokio::time::sleep(Duration::from_secs(2)).await;
        let poll = tokio::time::timeout_at(deadline, Fal.poll(ctx, key, &handle));
        let Ok(result) = poll.await else {
            return Err(failed("the edit took too long"));
        };
        let polled = match result {
            Ok(p) => {
                errors = 0;
                p
            }
            Err(_) if errors < 4 && tokio::time::Instant::now() < deadline => {
                errors += 1;
                continue;
            }
            Err(e) => return Err(e),
        };
        match polled {
            Polled::Running(_) if tokio::time::Instant::now() < deadline => continue,
            Polled::Running(_) => return Err(failed("the edit took too long")),
            Polled::Failed(e) => return Err(failed(&e)),
            Polled::Done(outs) => {
                return outs.into_iter().find(|o| o.kind == "image").map(|o| o.url).ok_or_else(|| failed("no picture came back"))
            }
        }
    }
}

// ------------------------------------------------------------------ Replicate

pub struct Replicate;

#[async_trait]
impl Provider for Replicate {
    async fn submit(&self, ctx: &Ctx, key: &str, req: &SubmitReq) -> PResult<Submitted> {
        let mut input = json!({"prompt": req.prompt, "aspect_ratio": req.aspect.as_deref().unwrap_or("16:9")});
        if let Some(seed) = req.seed {
            input["seed"] = json!(seed);
        }
        let url = format!("https://api.replicate.com/v1/models/{}/predictions", req.endpoint);
        let resp = ctx
            .http
            .post(url.as_str())
            .bearer_auth(key)
            .json(&json!({"input": input}))
            .send()
            .await
            .map_err(net)?;
        let v = json_body(resp).await?;
        let status_url = v
            .pointer("/urls/get")
            .and_then(Value::as_str)
            .ok_or_else(|| ProviderError::Failed("replicate response missing urls.get".into()))?
            .to_string();
        ensure_host(&status_url, "replicate.com")?;
        Ok(Submitted::Queued(Handle { request_id: str_at(&v, "id")?, status_url, result_url: None }))
    }

    async fn poll(&self, ctx: &Ctx, key: &str, h: &Handle) -> PResult<Polled> {
        let resp = ctx.http.get(h.status_url.as_str()).bearer_auth(key).send().await.map_err(net)?;
        let v = json_body(resp).await?;
        match v.get("status").and_then(Value::as_str).unwrap_or("") {
            "starting" | "processing" => Ok(Polled::Running(None)),
            "succeeded" => {
                let urls: Vec<String> = match v.get("output") {
                    Some(Value::String(s)) => vec![s.clone()],
                    Some(Value::Array(a)) => a.iter().filter_map(|x| x.as_str().map(String::from)).collect(),
                    _ => Vec::new(),
                };
                if urls.is_empty() {
                    return Ok(Polled::Failed("replicate returned no output".into()));
                }
                let mut outs = Vec::new();
                for url in urls {
                    let (bytes, _) = media::fetch(&ctx.http, &url, &[]).await?;
                    let ext = ext_from_url(&url, "png");
                    outs.push(media::save(ctx, &bytes, &ext, "image").await?);
                }
                Ok(Polled::Done(outs))
            }
            "failed" | "canceled" => {
                let msg = v.get("error").and_then(Value::as_str).unwrap_or("replicate job did not succeed");
                Ok(Polled::Failed(excerpt(msg)))
            }
            _ => Ok(Polled::Running(None)),
        }
    }
}

// ------------------------------------------------------------------ OpenAI images (synchronous)

pub struct OpenAiImage;

#[async_trait]
impl Provider for OpenAiImage {
    async fn submit(&self, ctx: &Ctx, key: &str, req: &SubmitReq) -> PResult<Submitted> {
        let size = match req.aspect.as_deref().unwrap_or("16:9") {
            "9:16" => "1024x1536",
            "1:1" => "1024x1024",
            _ => "1536x1024",
        };
        let body = json!({"model": req.endpoint, "prompt": req.prompt, "size": size, "quality": "medium", "n": 1});
        let resp = ctx
            .http
            .post("https://api.openai.com/v1/images/generations")
            .bearer_auth(key)
            .json(&body)
            .send()
            .await
            .map_err(net)?;
        let v = json_body(resp).await?;
        let b64 = v
            .pointer("/data/0/b64_json")
            .and_then(Value::as_str)
            .ok_or_else(|| ProviderError::Failed("OpenAI response had no image".into()))?;
        let bytes = B64
            .decode(b64)
            .map_err(|_| ProviderError::Failed("OpenAI image was not valid base64".into()))?;
        Ok(Submitted::Done(vec![media::save(ctx, &bytes, "png", "image").await?]))
    }
}

// ------------------------------------------------------------------ Gemini: Veo (long-running)

pub struct GeminiVeo;

#[async_trait]
impl Provider for GeminiVeo {
    fn poll_interval(&self) -> Duration {
        Duration::from_secs(10)
    }

    async fn submit(&self, ctx: &Ctx, key: &str, req: &SubmitReq) -> PResult<Submitted> {
        let mut instance = json!({"prompt": req.prompt});
        if let Some(img) = &req.image_url {
            // Shape from the Gemini Veo docs' REST example (2026-10-13): instances[0].image.inlineData.
            let (bytes, mime) = media::load_image(ctx, img).await?;
            instance["image"] = json!({"inlineData": {"mimeType": mime, "data": B64.encode(bytes)}});
        }
        let mut params = json!({"aspectRatio": req.aspect.as_deref().unwrap_or("16:9")});
        // Gemini Veo docs (2026-10-13): 1080p and 4k only support 8 seconds.
        let duration = match req.resolution.as_deref() {
            Some("1080p" | "4k") => Some(8),
            _ => req.duration,
        };
        if let Some(d) = duration {
            params["durationSeconds"] = json!(d.to_string());
        }
        if let Some(r) = &req.resolution {
            params["resolution"] = json!(r);
        }
        let url = format!("{GEMINI_BASE}/models/{}:predictLongRunning", req.endpoint);
        let body = json!({"instances": [instance], "parameters": params});
        let resp = ctx
            .http
            .post(url.as_str())
            .header("x-goog-api-key", key)
            .json(&body)
            .send()
            .await
            .map_err(net)?;
        let v = json_body(resp).await?;
        let name = str_at(&v, "name")?;
        Ok(Submitted::Queued(Handle {
            request_id: name.clone(),
            status_url: format!("{GEMINI_BASE}/{name}"),
            result_url: None,
        }))
    }

    async fn poll(&self, ctx: &Ctx, key: &str, h: &Handle) -> PResult<Polled> {
        let resp = ctx
            .http
            .get(h.status_url.as_str())
            .header("x-goog-api-key", key)
            .send()
            .await
            .map_err(net)?;
        let v = json_body(resp).await?;
        if v.get("done").and_then(Value::as_bool) != Some(true) {
            return Ok(Polled::Running(None));
        }
        if let Some(err) = v.get("error") {
            let msg = err.get("message").and_then(Value::as_str).unwrap_or("Veo reported an error");
            return Ok(Polled::Failed(excerpt(msg)));
        }
        let uri = v
            .pointer("/response/generateVideoResponse/generatedSamples/0/video/uri")
            .or_else(|| v.pointer("/response/generatedVideos/0/video/uri"))
            .and_then(Value::as_str);
        // Finished with no video: Veo's safety filters held it back. That is final, not worth retrying.
        let Some(uri) = uri else {
            return Ok(Polled::Failed("Veo finished without a video (usually its safety filters). Try other words.".into()));
        };
        ensure_host(uri, "generativelanguage.googleapis.com")?;
        let (bytes, _) = media::fetch(&ctx.http, uri, &[("x-goog-api-key", key)]).await?;
        Ok(Polled::Done(vec![media::save(ctx, &bytes, "mp4", "video").await?]))
    }
}

// ------------------------------------------------------------------ Gemini: image (synchronous)

pub struct GeminiImage;

/// Reads the image from either SDK shape or the raw REST steps shape.
fn gemini_image_b64(v: &Value) -> Option<String> {
    if let Some(s) = v.pointer("/output_image/data").and_then(Value::as_str) {
        return Some(s.to_string());
    }
    let steps = v.get("steps")?.as_array()?;
    for step in steps {
        if step.get("type").and_then(Value::as_str) != Some("model_output") {
            continue;
        }
        let Some(content) = step.get("content").and_then(Value::as_array) else {
            continue;
        };
        for c in content {
            if c.get("type").and_then(Value::as_str) == Some("image") {
                if let Some(d) = c.get("data").and_then(Value::as_str) {
                    return Some(d.to_string());
                }
            }
        }
    }
    None
}

#[async_trait]
impl Provider for GeminiImage {
    async fn submit(&self, ctx: &Ctx, key: &str, req: &SubmitReq) -> PResult<Submitted> {
        // Request and response shape from the Gemini image-generation docs (Interactions API, 2026-10-13).
        let body = json!({
            "model": req.endpoint,
            "input": [{"type": "text", "text": req.prompt}],
            "response_format": {
                "type": "image",
                "mime_type": "image/png",
                "aspect_ratio": req.aspect.as_deref().unwrap_or("16:9"),
                "image_size": "2K"
            }
        });
        let resp = ctx
            .http
            .post(format!("{GEMINI_BASE}/interactions").as_str())
            .header("x-goog-api-key", key)
            .json(&body)
            .send()
            .await
            .map_err(net)?;
        let v = json_body(resp).await?;
        let b64 = gemini_image_b64(&v).ok_or_else(|| ProviderError::Failed("Gemini response had no image".into()))?;
        let bytes = B64
            .decode(b64)
            .map_err(|_| ProviderError::Failed("Gemini image was not valid base64".into()))?;
        Ok(Submitted::Done(vec![media::save(ctx, &bytes, "png", "image").await?]))
    }
}

// ------------------------------------------------------------------ Black Forest Labs (FLUX)

pub struct Bfl;

#[async_trait]
impl Provider for Bfl {
    async fn submit(&self, ctx: &Ctx, key: &str, req: &SubmitReq) -> PResult<Submitted> {
        let (w, h) = match req.aspect.as_deref().unwrap_or("16:9") {
            "9:16" => (576, 1024),
            "1:1" => (1024, 1024),
            _ => (1024, 576),
        };
        let mut body = json!({"prompt": req.prompt, "width": w, "height": h});
        if let Some(seed) = req.seed {
            body["seed"] = json!(seed);
        }
        let url = format!("https://api.bfl.ai/v1/{}", req.endpoint);
        let resp = ctx
            .http
            .post(url.as_str())
            .header("x-key", key)
            .json(&body)
            .send()
            .await
            .map_err(net)?;
        let v = json_body(resp).await?;
        let polling = str_at(&v, "polling_url")?;
        ensure_host(&polling, "bfl.ai")?;
        Ok(Submitted::Queued(Handle { request_id: str_at(&v, "id")?, status_url: polling, result_url: None }))
    }

    async fn poll(&self, ctx: &Ctx, key: &str, h: &Handle) -> PResult<Polled> {
        let resp = ctx
            .http
            .get(h.status_url.as_str())
            .header("x-key", key)
            .send()
            .await
            .map_err(net)?;
        let v = json_body(resp).await?;
        let status = v.get("status").and_then(Value::as_str).unwrap_or("");
        match status {
            "Ready" => {
                let sample = v
                    .pointer("/result/sample")
                    .and_then(Value::as_str)
                    .ok_or_else(|| ProviderError::Failed("BFL result had no sample URL".into()))?;
                // The signed URL expires in about 10 minutes, so save it now.
                let (bytes, _) = media::fetch(&ctx.http, sample, &[]).await?;
                let ext = ext_from_url(sample, "png");
                Ok(Polled::Done(vec![media::save(ctx, &bytes, &ext, "image").await?]))
            }
            // Final states from BFL's StatusResponse enum (api.bfl.ai/openapi.json, read 2026-10-13); "Failed" is
            // used in their guide. Pending, Reasoning, Generating and anything new keep polling until the age cap.
            "Error" | "Failed" | "Task not found" | "Request Moderated" | "Content Moderated" | "No Card Generated" => {
                Ok(Polled::Failed(format!("BFL {status}")))
            }
            _ => Ok(Polled::Running(v.get("progress").and_then(Value::as_f64))),
        }
    }
}

// ------------------------------------------------------------------ Meshy 3D (image or multi-image to 3D)

/// Image to 3D from 1 picture, Multi-Image to 3D from 2-4 (docs.meshy.ai, read 2026-10-12).
/// Meshy 7.1 ("latest"), 4K PBR textures: 30 credits either way per the Meshy pricing page.
pub struct Meshy;

const MESHY_API: &str = "https://api.meshy.ai/openapi/v1";
/// Formats kept locally, in this order: the viewer's GLB first, then Apple, print and slicer files.
const MESHY_FORMATS: [&str; 4] = ["glb", "usdz", "stl", "3mf"];

fn meshy_input(r: &SubmitReq, images: &[String]) -> (String, Value) {
    let mut body = json!({
        "ai_model": "latest",
        "should_texture": true,
        "enable_pbr": true,
        "texture_resolution": "4k",
        "target_formats": MESHY_FORMATS,
    });
    // No texture_prompt: Meshy bills text texturing separately, and the pictures already carry the look.
    let _ = &r.prompt;
    if images.len() == 1 {
        body["image_url"] = json!(images[0]);
        (format!("{MESHY_API}/image-to-3d"), body)
    } else {
        body["image_urls"] = json!(images);
        (format!("{MESHY_API}/multi-image-to-3d"), body)
    }
}

#[async_trait]
impl Provider for Meshy {
    fn poll_interval(&self) -> Duration {
        Duration::from_secs(5)
    }

    async fn submit(&self, ctx: &Ctx, key: &str, req: &SubmitReq) -> PResult<Submitted> {
        let mut images: Vec<String> = req.image_url.iter().chain(req.image_urls.iter()).cloned().collect();
        if images.is_empty() || images.len() > 4 {
            return Err(ProviderError::Failed("Meshy takes 1 to 4 pictures".into()));
        }
        // Meshy accepts data URIs, so local /outputs pictures are sent inline.
        for img in images.iter_mut() {
            if img.starts_with("/outputs/") {
                *img = media::as_data_uri(ctx, img).await?;
            }
        }
        let (url, body) = meshy_input(req, &images);
        let resp = ctx
            .http
            .post(url.as_str())
            .header("Authorization", format!("Bearer {key}"))
            .json(&body)
            .send()
            .await
            .map_err(net)?;
        let v = json_body(resp).await?;
        let id = str_at(&v, "result")?;
        if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
            return Err(ProviderError::Failed("Meshy returned an odd task id".into()));
        }
        Ok(Submitted::Queued(Handle { status_url: format!("{url}/{id}"), request_id: id, result_url: None }))
    }

    async fn poll(&self, ctx: &Ctx, key: &str, h: &Handle) -> PResult<Polled> {
        ensure_host(&h.status_url, "api.meshy.ai")?;
        let resp = ctx
            .http
            .get(h.status_url.as_str())
            .header("Authorization", format!("Bearer {key}"))
            .send()
            .await
            .map_err(net)?;
        let v = json_body(resp).await?;
        match v.get("status").and_then(Value::as_str).unwrap_or("") {
            "SUCCEEDED" => {
                // Asset links are signed and expire, so everything is saved now (the key is not sent).
                let mut outs = Vec::new();
                for f in MESHY_FORMATS {
                    if let Some(u) = v.pointer(&format!("/model_urls/{f}")).and_then(Value::as_str) {
                        let (bytes, _) = media::fetch(&ctx.http, u, &[]).await?;
                        outs.push(media::save(ctx, &bytes, f, "model").await?);
                    }
                }
                if outs.is_empty() {
                    return Ok(Polled::Failed("Meshy returned no model file".into()));
                }
                if let Some(u) = v.get("thumbnail_url").and_then(Value::as_str) {
                    let (bytes, _) = media::fetch(&ctx.http, u, &[]).await?;
                    outs.push(media::save(ctx, &bytes, &ext_from_url(u, "png"), "image").await?);
                }
                Ok(Polled::Done(outs))
            }
            "FAILED" | "CANCELED" => {
                let msg = v.pointer("/task_error/message").and_then(Value::as_str).unwrap_or("no reason given");
                Ok(Polled::Failed(format!("Meshy: {}", msg.chars().take(200).collect::<String>())))
            }
            _ => Ok(Polled::Running(v.get("progress").and_then(Value::as_f64).map(|p| p / 100.0))),
        }
    }
}

// ------------------------------------------------------------------ ElevenLabs TTS (synchronous)

pub struct ElevenLabs;

#[async_trait]
impl Provider for ElevenLabs {
    async fn submit(&self, ctx: &Ctx, key: &str, req: &SubmitReq) -> PResult<Submitted> {
        let voice = req
            .voice_id
            .as_deref()
            .filter(|v| valid_voice(v))
            .ok_or_else(|| ProviderError::Failed("voice_id missing or invalid".into()))?;
        let url = format!("https://api.elevenlabs.io/v1/text-to-speech/{voice}?output_format=mp3_44100_128");
        let body = json!({"text": req.prompt, "model_id": req.endpoint});
        let resp = ctx
            .http
            .post(url.as_str())
            .header("xi-api-key", key)
            .json(&body)
            .send()
            .await
            .map_err(net)?;
        let bytes = bytes_body(resp).await?;
        Ok(Submitted::Done(vec![media::save(ctx, &bytes, "mp3", "audio").await?]))
    }
}

/// The voices on the user's ElevenLabs account (premade, cloned and library voices), for the studio's picker.
pub async fn elevenlabs_voices(http: &reqwest::Client, key: &str) -> PResult<Vec<Value>> {
    let resp = http
        .get("https://api.elevenlabs.io/v2/voices?page_size=100&sort=name&sort_direction=asc")
        .header("xi-api-key", key)
        .send()
        .await
        .map_err(net)?;
    let bytes = bytes_body(resp).await?;
    let v: Value = serde_json::from_slice(&bytes).map_err(|_| ProviderError::Failed("bad voices response".into()))?;
    Ok(voice_list(&v))
}

fn voice_list(v: &Value) -> Vec<Value> {
    v.get("voices")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(|x| {
                    let id = x.get("voice_id").and_then(Value::as_str).filter(|id| valid_voice(id))?;
                    let name = x.get("name").and_then(Value::as_str).unwrap_or(id);
                    Some(json!({"id": id, "name": name, "category": x.get("category").and_then(Value::as_str)}))
                })
                .collect()
        })
        .unwrap_or_default()
}

// ------------------------------------------------------------------ Test mode (fake providers)

/// Answers every model locally, for trying the studio and its pipelines without keys or spending anything.
/// It never opens a network connection: pictures are placeholders that say TEST OUTPUT and name the step, clips are
/// made from them with the local ffmpeg. Only used when the gateway starts with AGAVE_FAKE_PROVIDERS=1 (state.rs),
/// which also moves all data to data/fake-providers, or in unit tests.
pub struct Fake {
    delay: Duration,
    /// Kinds ("image", "video", "3d") whose next job fails once (AGAVE_FAKE_FAIL), to try "Run again".
    fail_once: std::sync::Mutex<std::collections::HashSet<String>>,
    /// Every request this provider got, newest last (tests check what a step was fed).
    pub seen: std::sync::Mutex<Vec<SubmitReq>>,
}

impl Fake {
    /// `fail` is a comma list of kinds that fail once each: image, video, 3d.
    pub fn new(delay: Duration, fail: &str) -> Self {
        let fail_once = fail.split(',').map(|k| k.trim().to_ascii_lowercase()).filter(|k| !k.is_empty()).collect();
        Fake { delay, fail_once: std::sync::Mutex::new(fail_once), seen: Default::default() }
    }

    /// Makes the next job of this kind fail once.
    #[cfg(test)]
    pub fn fail_next(&self, kind: &str) {
        self.fail_once.lock().unwrap_or_else(|e| e.into_inner()).insert(kind.to_string());
    }

    fn kind(mode: &str) -> &'static str {
        match mode {
            "t2v" | "i2v" => "video",
            "i23d" => "3d",
            "tts" => "audio",
            _ => "image",
        }
    }

    /// A plain placeholder that says TEST OUTPUT and names the step, scaled up to 1280 wide with ffmpeg (kept
    /// small when ffmpeg is missing). Never a real photo, so it cannot be mistaken for a result.
    async fn picture(&self, ctx: &Ctx, req: &SubmitReq, kind: &str) -> PResult<Output> {
        let label = req.label.clone().unwrap_or_else(|| kind.to_string());
        let detail = format!("{kind} · {}", req.endpoint);
        let aspect = req.aspect.as_deref().unwrap_or("16:9");
        let small = crate::placeholder::picture(aspect, &label, &detail, &req.prompt);
        let raw = media::save(ctx, &small, "png", "image").await?;
        let src = ctx.outputs_dir.join(raw.url.trim_start_matches("/outputs/"));
        let name = format!("{}.png", media::random_hex(12));
        let mut cmd = tokio::process::Command::new("ffmpeg");
        cmd.args(["-y", "-v", "error", "-i"]).arg(&src).args(["-vf", "scale=iw*4:ih*4:flags=neighbor"]).arg(ctx.outputs_dir.join(&name));
        if crate::assemble::run(&mut cmd).await.is_err() {
            return Ok(raw);
        }
        let _ = tokio::fs::remove_file(&src).await;
        Ok(Output { url: format!("/outputs/{name}"), kind: "image".into() })
    }

    /// A 3-second clip with a slow push-in over the step's placeholder.
    async fn clip(&self, ctx: &Ctx, req: &SubmitReq) -> PResult<Output> {
        let still = self.picture(ctx, req, "video").await?;
        let src = ctx.outputs_dir.join(still.url.trim_start_matches("/outputs/"));
        let name = format!("{}.mp4", media::random_hex(12));
        let dest = ctx.outputs_dir.join(&name);
        let (w, h) = match req.aspect.as_deref() {
            Some("9:16") => (720, 1280),
            Some("1:1") => (960, 960),
            _ => (1280, 720),
        };
        let vf = format!(
            "scale={w}:{h}:flags=neighbor,zoompan=z='min(zoom+0.0015,1.12)':x='iw/2-(iw/zoom/2)':y='ih/2-(ih/zoom/2)':d=72:s={w}x{h}:fps=24,format=yuv420p"
        );
        let mut cmd = tokio::process::Command::new("ffmpeg");
        cmd.args(["-y", "-v", "error", "-i"]).arg(&src).args(["-vf", vf.as_str(), "-frames:v", "72", "-c:v", "libx264", "-movflags", "+faststart"]).arg(&dest);
        let made = crate::assemble::run(&mut cmd).await;
        let _ = tokio::fs::remove_file(&src).await;
        made?;
        Ok(Output { url: format!("/outputs/{name}"), kind: "video".into() })
    }
}

#[async_trait]
impl Provider for Fake {
    async fn submit(&self, ctx: &Ctx, _key: &str, req: &SubmitReq) -> PResult<Submitted> {
        {
            let mut seen = self.seen.lock().unwrap_or_else(|e| e.into_inner());
            seen.push(req.clone());
            if seen.len() > 200 {
                seen.remove(0);
            }
        }
        tokio::time::sleep(self.delay).await;
        let kind = Self::kind(&req.mode);
        if self.fail_once.lock().unwrap_or_else(|e| e.into_inner()).remove(kind) {
            return Err(ProviderError::Failed("Test mode: this step was set to fail once, to try Run again.".into()));
        }
        let out = match kind {
            "video" => self.clip(ctx, req).await?,
            "audio" => return Err(ProviderError::Failed("Test mode has no fake voice.".into())),
            _ => self.picture(ctx, req, kind).await?,
        };
        Ok(Submitted::Done(vec![out]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(endpoint: &str, aspect: &str) -> SubmitReq {
        SubmitReq {
            endpoint: endpoint.into(),
            mode: "t2i".into(),
            prompt: "a lamp".into(),
            aspect: Some(aspect.into()),
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
        }
    }

    #[test]
    fn fal_flux_uses_image_size() {
        let body = fal_input(&req("fal-ai/flux/schnell", "9:16"));
        assert_eq!(body["image_size"], "portrait_16_9");
        assert!(body.get("aspect_ratio").is_none());
    }

    #[test]
    fn fal_seedream_uses_image_size_and_nano_banana_aspect_ratio() {
        let body = fal_input(&req("fal-ai/bytedance/seedream/v4/text-to-image", "1:1"));
        assert_eq!(body["image_size"], "square_hd");
        assert!(body.get("aspect_ratio").is_none());
        let body = fal_input(&req("fal-ai/nano-banana", "9:16"));
        assert_eq!(body["aspect_ratio"], "9:16");
    }

    #[test]
    fn fal_kling3_sends_elements_beside_the_start_frame() {
        let mut r = req("fal-ai/kling-video/v3/pro/image-to-video", "16:9");
        r.image_url = Some("https://x.test/start.png".into());
        assert!(fal_input(&r).get("elements").is_none());
        r.elements = vec![
            vec!["https://x.test/mia.png".into()],
            (1..=5).map(|i| format!("https://x.test/mug{i}.png")).collect(),
        ];
        r.end_image_url = Some("data:image/jpeg;base64,AAAA".into());
        let b = fal_input(&r);
        assert_eq!(b["start_image_url"], "https://x.test/start.png");
        assert_eq!(b["end_image_url"], "data:image/jpeg;base64,AAAA");
        assert_eq!(b["elements"][0], json!({"frontal_image_url": "https://x.test/mia.png", "reference_image_urls": ["https://x.test/mia.png"]}));
        assert_eq!(b["elements"][1]["frontal_image_url"], "https://x.test/mug1.png");
        assert_eq!(b["elements"][1]["reference_image_urls"], json!(["https://x.test/mug2.png", "https://x.test/mug3.png", "https://x.test/mug4.png"]));
    }

    #[test]
    fn an_edit_without_an_aspect_keeps_the_pictures_frame() {
        let mut r = req("fal-ai/bytedance/seedream/v4/edit", "16:9");
        assert_eq!(fal_input(&r)["image_size"], "landscape_16_9");
        r.aspect = None;
        assert_eq!(fal_input(&r)["image_size"], "auto");
        let mut r = req("fal-ai/nano-banana/edit", "16:9");
        r.aspect = None;
        assert!(fal_input(&r).get("aspect_ratio").is_none(), "Nano Banana Edit's own default is auto");
    }

    #[test]
    fn fal_sends_generate_audio_only_when_the_job_sets_it() {
        let mut r = req("fal-ai/kling-video/v3/pro/text-to-video", "16:9");
        assert!(fal_input(&r).get("generate_audio").is_none(), "unset: the model's own default");
        r.audio = Some(false);
        assert_eq!(fal_input(&r)["generate_audio"], false);
        r.audio = Some(true);
        assert_eq!(fal_input(&r)["generate_audio"], true);
    }

    #[test]
    fn fal_kling3_uses_start_image_url_and_skips_unlisted_aspect() {
        let mut r = req("fal-ai/kling-video/v3/pro/image-to-video", "16:9");
        r.aspect = None;
        r.duration = Some(5);
        r.image_url = Some("https://x.test/a.png".into());
        let b = fal_input(&r);
        assert_eq!(b["start_image_url"], "https://x.test/a.png");
        assert!(b.get("image_url").is_none());
        assert!(b.get("aspect_ratio").is_none());
        assert_eq!(b["duration"], "5");
        let mut s = req("bytedance/seedance-2.5/image-to-video", "16:9");
        s.aspect = None;
        s.image_url = Some("https://x.test/a.png".into());
        assert_eq!(fal_input(&s)["image_url"], "https://x.test/a.png");
    }

    #[test]
    fn fal_veo_duration_has_seconds_suffix_and_wan_has_none() {
        let mut v = req("fal-ai/veo3.1", "16:9");
        v.duration = Some(8);
        assert_eq!(fal_input(&v)["duration"], "8s");
        let mut w = req("fal-ai/wan/v2.2-a14b/text-to-video", "16:9");
        w.duration = Some(5);
        assert!(fal_input(&w).get("duration").is_none());
    }

    #[test]
    fn meshy_picks_single_or_multi_image_and_keeps_print_formats() {
        let mut r = req("openapi/v1", "16:9");
        r.prompt = String::new();
        let (url, body) = meshy_input(&r, &["data:image/png;base64,AA".into()]);
        assert!(url.ends_with("/image-to-3d"));
        assert_eq!(body["image_url"], "data:image/png;base64,AA");
        assert_eq!(body["target_formats"], json!(["glb", "usdz", "stl", "3mf"]));
        r.prompt = "weathered bronze".into();
        let (url, body) = meshy_input(&r, &["a".into(), "b".into(), "c".into()]);
        assert!(url.ends_with("/multi-image-to-3d"));
        assert_eq!(body["image_urls"].as_array().map(Vec::len), Some(3));
        assert!(body.get("texture_prompt").is_none(), "words name the job; they are never billed as texturing");
        assert!(ensure_host("https://api.meshy.ai/openapi/v1/image-to-3d/abc", "api.meshy.ai").is_ok());
    }

    #[test]
    fn host_check_blocks_foreign_urls() {
        assert!(ensure_host("https://queue.fal.run/x/requests/1/status", "queue.fal.run").is_ok());
        assert!(ensure_host("https://evil.test/queue.fal.run", "queue.fal.run").is_err());
        assert!(ensure_host("http://queue.fal.run/x", "queue.fal.run").is_err());
        assert!(ensure_host("https://notqueue.fal.run.evil.test/x", "queue.fal.run").is_err());
    }

    #[test]
    fn voice_ids_are_strict() {
        assert!(valid_voice("21m00Tcm4TlvDq8ikWAM"));
        assert!(!valid_voice("../v1/user"));
        assert!(!valid_voice(""));
    }

    #[test]
    fn voice_list_keeps_only_safe_ids() {
        let v = json!({"voices": [
            {"voice_id": "21m00Tcm4TlvDq8ikWAM", "name": "Rachel", "category": "premade"},
            {"voice_id": "../v1/user", "name": "bad"},
            {"name": "no id"}
        ]});
        assert_eq!(voice_list(&v), vec![json!({"id": "21m00Tcm4TlvDq8ikWAM", "name": "Rachel", "category": "premade"})]);
    }

    #[test]
    fn unimplemented_provider_has_no_adapter() {
        let m = ModelDef {
            id: "runway/x".into(),
            name: "x".into(),
            vendor: "x".into(),
            provider: "runway".into(),
            endpoint: "x".into(),
            modality: "video".into(),
            modes: vec!["i2v".into()],
            key: "RUNWAYML_API_SECRET".into(),
            verified: false,
            params: Default::default(),
            price: None,
            max_refs: 0,
            max_elements: 0,
            end_frame: false,
            provider_credits: None,
        };
        assert!(adapter_for(&m).is_none());
    }

    #[test]
    fn fal_sends_several_references_as_image_urls() {
        let mut r = req("fal-ai/nano-banana/edit", "1:1");
        r.image_urls = vec!["https://a.test/1.png".into(), "https://a.test/2.png".into()];
        let b = fal_input(&r);
        assert_eq!(b["image_urls"], json!(["https://a.test/1.png", "https://a.test/2.png"]));
        assert_eq!(b["aspect_ratio"], "1:1");
        assert!(b.get("image_url").is_none());
        let mut r = req("fal-ai/bytedance/seedream/v4/edit", "9:16");
        r.image_urls = vec!["https://a.test/1.png".into()];
        let b = fal_input(&r);
        assert_eq!((b["image_size"].as_str(), b["image_urls"][0].as_str()), (Some("portrait_16_9"), Some("https://a.test/1.png")));
    }
}
