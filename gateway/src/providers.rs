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
        ("comfy", _) => Arc::new(Comfy::default()),
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

fn str_at(v: &Value, key: &str) -> PResult<String> {
    v.get(key)
        .and_then(Value::as_str)
        .map(String::from)
        .ok_or_else(|| ProviderError::Failed(format!("provider response missing {key}")))
}

/// Refuses to send a key to a URL outside the provider's own domain.
fn ensure_host(url: &str, suffix: &str) -> PResult<()> {
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
        let size = match aspect {
            "9:16" => "portrait_16_9",
            "1:1" => "square_hd",
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
        if let Some(img) = &r.image_url {
            // Kling 3.0 image to video names the field start_image_url (fal API schema, 2026-10-11).
            let field = if r.endpoint.starts_with("fal-ai/kling-video/v3/") { "start_image_url" } else { "image_url" };
            b[field] = json!(img);
        }
        // Multi-reference edit models (Nano Banana edit, Seedream edit) take image_urls.
        if !r.image_urls.is_empty() {
            b["image_urls"] = json!(r.image_urls);
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
        // fal cannot read our local /outputs paths, so inline them.
        if let Some(img) = r.image_url.clone() {
            if img.starts_with("/outputs/") {
                r.image_url = Some(media::as_data_uri(ctx, &img).await?);
            }
        }
        for img in r.image_urls.iter_mut() {
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

// ------------------------------------------------------------------ Comfy API v2 (your own ComfyUI)

/// Runs open-weights workflows on your own ComfyUI through comfy-api-proxy, which serves the Comfy API v2 on
/// http://127.0.0.1:8189 by default with no key (docs.comfy.org: development/comfyui-server/api-proxy and
/// api-reference/v2/overview, openapi-v2.yaml, read 2026-10-18). Nothing is billed: the work runs on your GPU.
///   POST {base}/api/v2/assets   multipart file, content_type, file_path -> Asset {id}   (pictures a workflow loads)
///   POST {base}/api/v2/jobs     {"workflow": <API-format graph>} + Idempotency-Key -> 201 Job {id, status, urls.self}
///   GET  urls.self              Job {status: queued|running|succeeded|canceling|canceled|failed|expired,
///                               progress.value, outputs[{type, content_type, name, url}], error{code, message}}
///   GET  outputs[].url          the bytes ({base}/api/v2/assets/{id}/content; the proxy serves them itself)
/// The "key" this adapter gets is the COMFY_BASE_URL setting (keys.rs gives it a default), never a secret.
pub struct Comfy<T = ComfyHttp> {
    transport: T,
}

impl Default for Comfy<ComfyHttp> {
    fn default() -> Self {
        Comfy { transport: ComfyHttp }
    }
}

/// One HTTP call to the ComfyUI address.
pub enum ComfyCall<'a> {
    Get(&'a str),
    Json { url: &'a str, body: &'a Value, idempotency: &'a str },
    Upload { url: &'a str, bytes: Vec<u8>, content_type: &'a str, file_path: &'a str },
}

/// The calls the Comfy adapter makes, behind a trait so tests answer them without a network.
#[async_trait]
pub trait ComfyTransport: Send + Sync {
    /// Returns the status, the body (capped) and its content type.
    async fn send(&self, http: &reqwest::Client, call: ComfyCall<'_>) -> PResult<(u16, Vec<u8>, Option<String>)>;
}

pub struct ComfyHttp;

#[async_trait]
impl ComfyTransport for ComfyHttp {
    async fn send(&self, http: &reqwest::Client, call: ComfyCall<'_>) -> PResult<(u16, Vec<u8>, Option<String>)> {
        let req = match call {
            ComfyCall::Get(url) => http.get(url),
            ComfyCall::Json { url, body, idempotency } => http.post(url).header("Idempotency-Key", idempotency).json(body),
            ComfyCall::Upload { url, bytes, content_type, file_path } => {
                let part = reqwest::multipart::Part::bytes(bytes)
                    .file_name(file_path.to_string())
                    .mime_str(content_type)
                    .map_err(|_| ProviderError::Failed("bad picture type".into()))?;
                // Field order as the official SDK sends it: content_type, file_path, then the file.
                let form = reqwest::multipart::Form::new()
                    .text("content_type", content_type.to_string())
                    .text("file_path", file_path.to_string())
                    .part("file", part);
                http.post(url).multipart(form)
            }
        };
        // The shared client never follows redirects, so nothing is sent anywhere but the ComfyUI address.
        let resp = req.send().await.map_err(|e| ProviderError::Failed(format!("ComfyUI not reachable: {}", e.without_url())))?;
        let status = resp.status().as_u16();
        let ct = resp.headers().get(reqwest::header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).map(str::to_string);
        let bytes = media::read_capped(resp, media::MAX_DOWNLOAD_BYTES).await?;
        Ok((status, bytes, ct))
    }
}

pub const COMFY_DEFAULT_BASE: &str = "http://127.0.0.1:8189";

/// Checks a COMFY_BASE_URL value: http or https, a host, no user name or password, no query or fragment.
/// Returns it without a trailing slash. It is an outbound address the gateway posts workflows and pictures to,
/// so it can only be set by the owner (keys.env, an environment variable, or POST /api/keys behind guard()),
/// and every link a ComfyUI answer carries must stay on this same origin (`on_base`).
pub fn comfy_base(raw: &str) -> Result<reqwest::Url, &'static str> {
    let raw = raw.trim().trim_end_matches('/');
    let u = reqwest::Url::parse(raw).map_err(|_| "invalid_url")?;
    let ok = matches!(u.scheme(), "http" | "https")
        && u.host_str().is_some_and(|h| !h.is_empty())
        && u.username().is_empty()
        && u.password().is_none()
        && u.query().is_none()
        && u.fragment().is_none();
    if ok {
        Ok(u)
    } else {
        Err("invalid_url")
    }
}

/// A link from a ComfyUI answer (absolute, or host-relative per the v2 docs), only if it stays on the base origin.
fn on_base(base: &reqwest::Url, link: &str) -> PResult<String> {
    let u = base.join(link).map_err(|_| ProviderError::Failed("ComfyUI sent a bad link".into()))?;
    let same = u.scheme() == base.scheme() && u.host_str() == base.host_str() && u.port_or_known_default() == base.port_or_known_default();
    if same {
        Ok(u.to_string())
    } else {
        Err(ProviderError::Failed("refusing a ComfyUI link to another address".into()))
    }
}

fn base_url(key: &str) -> PResult<reqwest::Url> {
    comfy_base(key).map_err(|_| ProviderError::Failed("COMFY_BASE_URL is not a usable http(s) address. Fix it in Settings.".into()))
}

/// Comfy job and asset ids are UUIDs; anything else is refused before it goes into a URL or a workflow.
fn comfy_id(s: &str) -> bool {
    !s.is_empty() && s.len() <= 64 && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// A random UUID-shaped Idempotency-Key (the v2 docs recommend a client-generated UUID).
fn uuid_key() -> String {
    let h = media::random_hex(16);
    format!("{}-{}-4{}-a{}-{}", &h[0..8], &h[8..12], &h[13..16], &h[17..20], &h[20..32])
}

fn comfy_error(status: u16, body: &[u8]) -> ProviderError {
    let v: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
    let code = v.pointer("/error/code").and_then(Value::as_str).unwrap_or("error");
    let msg = v.pointer("/error/message").and_then(Value::as_str).map(excerpt).unwrap_or_else(|| excerpt(&String::from_utf8_lossy(body)));
    ProviderError::Failed(format!("ComfyUI {status} {code}: {msg}"))
}

/// LTX-Video sizes (multiples of 32) from the aspect, and frames from seconds (8n+1, as the node requires).
fn ltxv_size(aspect: Option<&str>) -> (u32, u32) {
    match aspect.unwrap_or("16:9") {
        "9:16" => (512, 768),
        "1:1" => (512, 512),
        _ => (768, 512),
    }
}

fn ltxv_frames(seconds: u32) -> u32 {
    (seconds * 24).div_ceil(8) * 8 + 1
}

const LTXV_NEGATIVE: &str = "low quality, worst quality, deformed, distorted, disfigured, motion smear, motion artifacts, fused fingers, bad anatomy, weird hand, ugly";

/// An uploaded picture: the asset reference the workflow's LoadImage gets.
pub struct ComfyAsset {
    pub id: String,
    pub file_path: String,
    pub hash: Option<String>,
}

/// The API-format workflow for one model. Graphs follow ComfyUI's own templates linked from docs.comfy.org
/// (tutorials/video/ltxv: ltxv_text_to_video.json and ltxv_image_to_video.json; tutorials/flux/flux-1-text-to-image:
/// flux_schnell.json), with node inputs named as on the built-in node pages. The LTX image to video graph matches
/// the one that ran end to end on a ComfyUI with comfy-api-proxy 0.1.6 and ltx-video-2b-v0.9.5 (2026-10-18): same
/// node ids, 20 steps, 768x512, 49 frames for 2 seconds.
fn comfy_workflow(r: &SubmitReq, image: Option<&ComfyAsset>, seed: u64) -> PResult<Value> {
    // The core/ASSET reference shape the official SDK builds: id always, file_path and hash when known.
    let asset = |a: &ComfyAsset| {
        let mut info = json!({"id": a.id, "file_path": a.file_path});
        if let Some(h) = &a.hash {
            info["hash"] = json!(h);
        }
        json!({"__type": "core/ASSET", "info": info})
    };
    match r.endpoint.as_str() {
        "ltxv-0.9.5-t2v" | "ltxv-0.9.5-i2v" => {
            let (w, h) = ltxv_size(r.aspect.as_deref());
            let length = ltxv_frames(r.duration.unwrap_or(2));
            let mut g = json!({
                "38": {"class_type": "CLIPLoader", "inputs": {"clip_name": "t5xxl_fp16.safetensors", "type": "ltxv", "device": "default"}},
                "44": {"class_type": "CheckpointLoaderSimple", "inputs": {"ckpt_name": "ltx-video-2b-v0.9.5.safetensors"}},
                "6": {"class_type": "CLIPTextEncode", "inputs": {"text": r.prompt, "clip": ["38", 0]}},
                "7": {"class_type": "CLIPTextEncode", "inputs": {"text": LTXV_NEGATIVE, "clip": ["38", 0]}},
                "73": {"class_type": "KSamplerSelect", "inputs": {"sampler_name": "euler"}},
                "8": {"class_type": "VAEDecode", "inputs": {"samples": ["72", 0], "vae": ["44", 2]}},
                "80": {"class_type": "CreateVideo", "inputs": {"images": ["8", 0], "fps": 24}},
                "81": {"class_type": "SaveVideo", "inputs": {"video": ["80", 0], "filename_prefix": "video/agave", "format": "auto", "codec": "auto"}}
            });
            // Text to video starts from an empty latent; image to video encodes the picture into the first frames.
            let latent = if r.endpoint.ends_with("-i2v") {
                let a = image.ok_or_else(|| ProviderError::Failed("LTX-Video image to video needs a picture".into()))?;
                g["78"] = json!({"class_type": "LoadImage", "inputs": {"image": asset(a)}});
                g["77"] = json!({"class_type": "LTXVImgToVideo", "inputs": {"positive": ["6", 0], "negative": ["7", 0], "vae": ["44", 2],
                    "image": ["78", 0], "width": w, "height": h, "length": length, "batch_size": 1, "strength": 0.15}});
                g["69"] = json!({"class_type": "LTXVConditioning", "inputs": {"positive": ["77", 0], "negative": ["77", 1], "frame_rate": 25}});
                json!(["77", 2])
            } else {
                g["70"] = json!({"class_type": "EmptyLTXVLatentVideo", "inputs": {"width": w, "height": h, "length": length, "batch_size": 1}});
                g["69"] = json!({"class_type": "LTXVConditioning", "inputs": {"positive": ["6", 0], "negative": ["7", 0], "frame_rate": 25}});
                json!(["70", 0])
            };
            g["71"] = json!({"class_type": "LTXVScheduler", "inputs": {"steps": 20, "max_shift": 2.05, "base_shift": 0.95, "stretch": true, "terminal": 0.1, "latent": latent}});
            g["72"] = json!({"class_type": "SamplerCustom", "inputs": {"model": ["44", 0], "add_noise": true, "noise_seed": seed, "cfg": 3,
                "positive": ["69", 0], "negative": ["69", 1], "sampler": ["73", 0], "sigmas": ["71", 0], "latent_image": latent}});
            Ok(g)
        }
        "flux1-schnell-fp8" => {
            let (w, h) = match r.aspect.as_deref().unwrap_or("16:9") {
                "9:16" => (768, 1344),
                "1:1" => (1024, 1024),
                _ => (1344, 768),
            };
            Ok(json!({
                "30": {"class_type": "CheckpointLoaderSimple", "inputs": {"ckpt_name": "flux1-schnell-fp8.safetensors"}},
                "6": {"class_type": "CLIPTextEncode", "inputs": {"text": r.prompt, "clip": ["30", 1]}},
                "33": {"class_type": "CLIPTextEncode", "inputs": {"text": "", "clip": ["30", 1]}},
                "27": {"class_type": "EmptySD3LatentImage", "inputs": {"width": w, "height": h, "batch_size": 1}},
                "31": {"class_type": "KSampler", "inputs": {"model": ["30", 0], "seed": seed, "steps": 4, "cfg": 1, "sampler_name": "euler",
                    "scheduler": "simple", "positive": ["6", 0], "negative": ["33", 0], "latent_image": ["27", 0], "denoise": 1}},
                "8": {"class_type": "VAEDecode", "inputs": {"samples": ["31", 0], "vae": ["30", 2]}},
                "9": {"class_type": "SaveImage", "inputs": {"images": ["8", 0], "filename_prefix": "agave"}}
            }))
        }
        other => Err(ProviderError::Failed(format!("no ComfyUI workflow for {other}"))),
    }
}

fn ext_for(content_type: &str, name: &str, kind: &str) -> String {
    match content_type.split(';').next().unwrap_or("").trim() {
        "video/mp4" => "mp4".into(),
        "video/webm" => "webm".into(),
        "image/png" => "png".into(),
        "image/jpeg" => "jpg".into(),
        "image/webp" => "webp".into(),
        _ => ext_from_url(name, if kind == "video" { "mp4" } else { "png" }),
    }
}

impl<T: ComfyTransport> Comfy<T> {
    async fn json(&self, ctx: &Ctx, call: ComfyCall<'_>, want: &[u16]) -> PResult<Value> {
        let (status, body, _) = self.transport.send(&ctx.http, call).await?;
        if !want.contains(&status) {
            return Err(comfy_error(status, &body));
        }
        serde_json::from_slice(&body).map_err(|_| ProviderError::Failed("ComfyUI returned non-JSON".into()))
    }

    /// Uploads a picture so the workflow can load it.
    async fn upload(&self, ctx: &Ctx, base: &reqwest::Url, src: &str) -> PResult<ComfyAsset> {
        let (bytes, mime) = media::load_image(ctx, src).await?;
        let ext = match mime.as_str() {
            "image/jpeg" => "jpg",
            "image/webp" => "webp",
            _ => "png",
        };
        let url = format!("{}/api/v2/assets", base.as_str().trim_end_matches('/'));
        let file_path = format!("agave-{}.{ext}", media::random_hex(6));
        let v = self.json(ctx, ComfyCall::Upload { url: &url, bytes, content_type: &mime, file_path: &file_path }, &[200, 201, 202]).await?;
        let id = str_at(&v, "id")?;
        if !comfy_id(&id) {
            return Err(ProviderError::Failed("ComfyUI returned an odd asset id".into()));
        }
        // blake3:<hex> when the server has computed it; anything else is left out of the reference.
        let hash = v.get("hash").and_then(Value::as_str).filter(|h| h.len() <= 80 && h.strip_prefix("blake3:").is_some_and(|x| x.chars().all(|c| c.is_ascii_hexdigit()))).map(String::from);
        Ok(ComfyAsset { id, file_path, hash })
    }
}

#[async_trait]
impl<T: ComfyTransport + 'static> Provider for Comfy<T> {
    fn poll_interval(&self) -> Duration {
        Duration::from_secs(4)
    }

    async fn submit(&self, ctx: &Ctx, key: &str, req: &SubmitReq) -> PResult<Submitted> {
        let base = base_url(key)?;
        let image = match req.image_url.iter().chain(req.image_urls.iter()).next() {
            Some(src) => Some(self.upload(ctx, &base, src).await?),
            None => None,
        };
        let seed = req.seed.map(|s| s.unsigned_abs()).unwrap_or_else(|| u64::from_str_radix(&media::random_hex(6), 16).unwrap_or(0));
        let workflow = comfy_workflow(req, image.as_ref(), seed)?;
        let url = format!("{}/api/v2/jobs", base.as_str().trim_end_matches('/'));
        let body = json!({"workflow": workflow});
        let idem = uuid_key();
        let job = self.json(ctx, ComfyCall::Json { url: &url, body: &body, idempotency: &idem }, &[200, 201]).await?;
        let id = str_at(&job, "id")?;
        if !comfy_id(&id) {
            return Err(ProviderError::Failed("ComfyUI returned an odd job id".into()));
        }
        // Follow the job's own link rather than building one (v2 docs); it must stay on the ComfyUI address.
        let link = job.pointer("/urls/self").and_then(Value::as_str).map(String::from).unwrap_or_else(|| format!("/api/v2/jobs/{id}"));
        let status_url = on_base(&base, &link)?;
        Ok(Submitted::Queued(Handle { request_id: id, status_url, result_url: None }))
    }

    async fn poll(&self, ctx: &Ctx, key: &str, h: &Handle) -> PResult<Polled> {
        let base = base_url(key)?;
        // A job saved before COMFY_BASE_URL changed is not polled at the new address.
        let status_url = on_base(&base, &h.status_url)?;
        let job = self.json(ctx, ComfyCall::Get(&status_url), &[200]).await?;
        match job.get("status").and_then(Value::as_str).unwrap_or("") {
            "succeeded" => {
                let outs = job.get("outputs").and_then(Value::as_array).cloned().unwrap_or_default();
                let pick = |t: &str| outs.iter().find(|o| o.get("type").and_then(Value::as_str) == Some(t)).cloned();
                let Some((out, kind)) = pick("video").map(|o| (o, "video")).or_else(|| pick("image").map(|o| (o, "image"))) else {
                    return Ok(Polled::Failed("ComfyUI finished with no picture or video".into()));
                };
                let link = out.get("url").and_then(Value::as_str).ok_or_else(|| ProviderError::Failed("ComfyUI output has no url".into()))?;
                let url = on_base(&base, link)?;
                let (status, bytes, ct) = self.transport.send(&ctx.http, ComfyCall::Get(&url)).await?;
                if status != 200 {
                    return Err(comfy_error(status, &bytes));
                }
                let ct = out.get("content_type").and_then(Value::as_str).map(String::from).or(ct).unwrap_or_default();
                let name = out.get("name").and_then(Value::as_str).unwrap_or("");
                Ok(Polled::Done(vec![media::save(ctx, &bytes, &ext_for(&ct, name, kind), kind).await?]))
            }
            "failed" | "canceled" | "expired" => {
                let st = job.get("status").and_then(Value::as_str).unwrap_or("failed");
                let msg = job.pointer("/error/message").and_then(Value::as_str).unwrap_or(st);
                Ok(Polled::Failed(format!("ComfyUI: {}", excerpt(msg))))
            }
            _ => Ok(Polled::Running(job.pointer("/progress/value").and_then(Value::as_f64))),
        }
    }
}

// ------------------------------------------------------------------ Test mode (fake providers)

/// Answers every model locally, for trying the studio and its pipelines without keys or spending anything.
/// It never opens a network connection: pictures are copies of the Library previews, clips are made from a
/// picture with the local ffmpeg. Only used when the gateway starts with AGAVE_FAKE_PROVIDERS=1 (state.rs),
/// which also moves all data to data/fake-providers, or in unit tests.
pub struct Fake {
    web: std::path::PathBuf,
    delay: Duration,
    /// Kinds ("image", "video", "3d") whose next job fails once (AGAVE_FAKE_FAIL), to try "Run again".
    fail_once: std::sync::Mutex<std::collections::HashSet<String>>,
    /// Every request this provider got, newest last (tests check what a step was fed).
    pub seen: std::sync::Mutex<Vec<SubmitReq>>,
}

/// A 1x1 PNG, used when a Library preview is not there (unit tests have no web folder).
const TINY_PNG: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52, 0x00, 0x00, 0x00, 0x01,
    0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4, 0x89, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x44, 0x41,
    0x54, 0x78, 0x9c, 0x63, 0x60, 0x60, 0x60, 0xf8, 0x0f, 0x00, 0x01, 0x04, 0x01, 0x00, 0x5f, 0xe5, 0xc3, 0x4b, 0x00, 0x00,
    0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
];

impl Fake {
    /// `fail` is a comma list of kinds that fail once each: image, video, 3d.
    pub fn new(web: std::path::PathBuf, delay: Duration, fail: &str) -> Self {
        let fail_once = fail.split(',').map(|k| k.trim().to_ascii_lowercase()).filter(|k| !k.is_empty()).collect();
        Fake { web, delay, fail_once: std::sync::Mutex::new(fail_once), seen: Default::default() }
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

    /// A Library preview that suits the request, or a 1x1 picture.
    fn picture(&self, req: &SubmitReq, kind: &str) -> (Vec<u8>, &'static str) {
        let p = req.prompt.to_ascii_lowercase();
        let name = if kind == "3d" {
            "print-3d.png"
        } else if p.contains("sheet") {
            "character-sheet.jpg"
        } else if p.contains("product") || p.contains("pedestal") {
            "product-hero.jpg"
        } else {
            "creator-portrait.jpg"
        };
        match std::fs::read(self.web.join("app/library").join(name)) {
            Ok(b) => (b, if name.ends_with(".png") { "png" } else { "jpg" }),
            Err(_) => (TINY_PNG.to_vec(), "png"),
        }
    }

    /// A 3-second clip with a slow push-in, made from the step's own picture when it has a local one.
    async fn clip(&self, ctx: &Ctx, req: &SubmitReq) -> PResult<Output> {
        let local = req.image_url.iter().chain(req.image_urls.iter()).find(|u| u.starts_with("/outputs/")).cloned();
        let src = match local.as_deref().and_then(|u| u.strip_prefix("/outputs/")).filter(|n| media::valid_output_name(n)) {
            Some(name) => ctx.outputs_dir.join(name),
            None => {
                let (bytes, ext) = self.picture(req, "image");
                let out = media::save(ctx, &bytes, ext, "image").await?;
                ctx.outputs_dir.join(out.url.trim_start_matches("/outputs/"))
            }
        };
        let name = format!("{}.mp4", media::random_hex(12));
        let dest = ctx.outputs_dir.join(&name);
        let vf = "scale=1280:720:force_original_aspect_ratio=increase,crop=1280:720,\
                  zoompan=z='min(zoom+0.0015,1.12)':x='iw/2-(iw/zoom/2)':y='ih/2-(ih/zoom/2)':d=72:s=1280x720:fps=24,format=yuv420p";
        let mut cmd = tokio::process::Command::new("ffmpeg");
        cmd.args(["-y", "-v", "error", "-i"]).arg(&src).args(["-vf", vf, "-frames:v", "72", "-c:v", "libx264", "-movflags", "+faststart"]).arg(&dest);
        crate::assemble::run(&mut cmd).await?;
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
            return Err(ProviderError::Failed(format!("Test mode: this {kind} step was set to fail once (AGAVE_FAKE_FAIL).")));
        }
        let out = match kind {
            "video" => self.clip(ctx, req).await?,
            "audio" => return Err(ProviderError::Failed("Test mode has no fake voice.".into())),
            _ => {
                let (bytes, ext) = self.picture(req, kind);
                media::save(ctx, &bytes, ext, "image").await?
            }
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
            provider_credits: None,
        };
        assert!(adapter_for(&m).is_none());
    }

    // ---------------------------------------------------------- Comfy API v2 (your own ComfyUI), no network

    type Answer = (u16, Vec<u8>, Option<String>);
    /// method, url, JSON body, and the Idempotency-Key or upload content type.
    type Call = (String, String, Option<Value>, Option<String>);

    /// Answers Comfy calls from a script and records them.
    #[derive(Default)]
    struct FakeComfy {
        answers: std::sync::Mutex<std::collections::HashMap<String, std::collections::VecDeque<Answer>>>,
        calls: std::sync::Mutex<Vec<Call>>,
    }

    impl FakeComfy {
        fn answer(&self, method: &str, url: &str, status: u16, body: Vec<u8>, ct: Option<&str>) {
            let mut a = self.answers.lock().unwrap();
            a.entry(format!("{method} {url}")).or_default().push_back((status, body, ct.map(String::from)));
        }
        fn answer_json(&self, method: &str, url: &str, status: u16, v: Value) {
            self.answer(method, url, status, v.to_string().into_bytes(), Some("application/json"));
        }
    }

    #[async_trait]
    impl ComfyTransport for FakeComfy {
        async fn send(&self, _http: &reqwest::Client, call: ComfyCall<'_>) -> PResult<(u16, Vec<u8>, Option<String>)> {
            let (method, url, body, extra) = match call {
                ComfyCall::Get(u) => ("GET", u.to_string(), None, None),
                ComfyCall::Json { url, body, idempotency } => ("POST", url.to_string(), Some(body.clone()), Some(idempotency.to_string())),
                ComfyCall::Upload { url, content_type, .. } => ("UPLOAD", url.to_string(), None, Some(content_type.to_string())),
            };
            self.calls.lock().unwrap().push((method.to_string(), url.clone(), body, extra));
            let key = format!("{} {url}", if method == "UPLOAD" { "POST" } else { method });
            self.answers
                .lock()
                .unwrap()
                .get_mut(&key)
                .and_then(|q| q.pop_front())
                .ok_or_else(|| ProviderError::Failed(format!("unexpected call {key}")))
        }
    }

    fn test_ctx(name: &str) -> Ctx {
        Ctx { http: reqwest::Client::new(), outputs_dir: crate::fsutil::test_dir(name) }
    }

    const BASE: &str = "http://127.0.0.1:8189";
    const JOB: &str = "7f3d2c1b-9a8e-4d6f-b012-3c4d5e6f7a8b";
    const ASSET: &str = "9f8a1c0d-2b3e-4f56-8a7b-1c2d3e4f5a6b";

    #[test]
    fn comfy_address_must_be_plain_http_or_https() {
        for ok in ["http://127.0.0.1:8189", "http://127.0.0.1:8189/", "https://gpu.example.net", "http://100.64.1.2:8189/comfy"] {
            assert!(comfy_base(ok).is_ok(), "{ok}");
        }
        for bad in ["", "127.0.0.1:8189", "ftp://127.0.0.1", "file:///etc/passwd", "javascript:alert(1)", "http://user:pw@127.0.0.1:8189",
                    "http://127.0.0.1:8189/?x=1", "http://127.0.0.1:8189/#a", "http://"] {
            assert!(comfy_base(bad).is_err(), "{bad}");
        }
        let base = comfy_base(BASE).unwrap();
        assert_eq!(on_base(&base, "/api/v2/jobs/1").unwrap(), "http://127.0.0.1:8189/api/v2/jobs/1");
        assert!(on_base(&base, "http://127.0.0.1:9999/x").is_err());
        assert!(on_base(&base, "https://127.0.0.1:8189/x").is_err());
        assert!(on_base(&base, "http://evil.test/api/v2/jobs/1").is_err());
        assert!(on_base(&base, "//evil.test/x").is_err());
    }

    #[test]
    fn comfy_workflows_follow_the_ltx_and_flux_templates() {
        let mut r = req("ltxv-0.9.5-t2v", "16:9");
        r.mode = "t2v".into();
        r.duration = Some(4);
        let g = comfy_workflow(&r, None, 7).unwrap();
        assert_eq!(ltxv_frames(2), 49, "2 seconds is the 49 frames of the tested graph");
        assert_eq!(g["70"]["class_type"], "EmptyLTXVLatentVideo");
        assert_eq!((g["70"]["inputs"]["width"].as_u64(), g["70"]["inputs"]["height"].as_u64(), g["70"]["inputs"]["length"].as_u64()), (Some(768), Some(512), Some(97)));
        assert_eq!(g["38"]["inputs"]["type"], "ltxv");
        assert_eq!(g["44"]["inputs"]["ckpt_name"], "ltx-video-2b-v0.9.5.safetensors");
        assert_eq!(g["72"]["inputs"]["noise_seed"], 7);
        assert_eq!(g["6"]["inputs"]["text"], "a lamp");
        assert!(g.get("78").is_none());
        assert_eq!(g["81"]["class_type"], "SaveVideo");
        // Image to video loads the uploaded picture as an asset reference.
        r.endpoint = "ltxv-0.9.5-i2v".into();
        assert!(comfy_workflow(&r, None, 7).is_err());
        let a = ComfyAsset { id: ASSET.into(), file_path: "agave-1.png".into(), hash: None };
        let g = comfy_workflow(&r, Some(&a), 7).unwrap();
        assert_eq!(g["78"]["inputs"]["image"], json!({"__type": "core/ASSET", "info": {"id": ASSET, "file_path": "agave-1.png"}}));
        assert_eq!(g["71"]["inputs"]["steps"], 20);
        assert_eq!(g["77"]["class_type"], "LTXVImgToVideo");
        assert_eq!(g["72"]["inputs"]["latent_image"], json!(["77", 2]));
        assert_eq!(ltxv_frames(5), 121);
        let g = comfy_workflow(&req("flux1-schnell-fp8", "9:16"), None, 1).unwrap();
        assert_eq!((g["27"]["inputs"]["width"].as_u64(), g["27"]["inputs"]["height"].as_u64()), (Some(768), Some(1344)));
        assert_eq!(g["31"]["inputs"]["steps"], 4);
        assert!(comfy_workflow(&req("some-other-workflow", "1:1"), None, 1).is_err());
    }

    #[tokio::test]
    async fn comfy_runs_a_job_from_upload_to_saved_file() {
        let ctx = test_ctx("comfy_flow");
        std::fs::write(ctx.outputs_dir.join("still.png"), TINY_PNG).unwrap();
        let t = FakeComfy::default();
        t.answer_json("POST", &format!("{BASE}/api/v2/assets"), 201, json!({"id": ASSET, "hash": "blake3:9f8a1c0d", "file_path": "x.png"}));
        t.answer_json("POST", &format!("{BASE}/api/v2/jobs"), 201, json!({"id": JOB, "status": "queued", "urls": {"self": format!("/api/v2/jobs/{JOB}")}}));
        let status = format!("{BASE}/api/v2/jobs/{JOB}");
        t.answer_json("GET", &status, 200, json!({"id": JOB, "status": "running", "progress": {"value": 0.5}}));
        let content = format!("{BASE}/api/v2/assets/{ASSET}/content");
        t.answer_json("GET", &status, 200, json!({"id": JOB, "status": "succeeded", "outputs": [
            {"type": "image", "url": "/api/v2/assets/other/content", "content_type": "image/png", "name": "preview.png"},
            {"type": "video", "url": content, "content_type": "video/mp4", "name": "agave_00001.mp4"}]}));
        t.answer("GET", &content, 200, b"fake mp4 bytes".to_vec(), Some("application/octet-stream"));
        let comfy = Comfy { transport: t };
        let mut r = req("ltxv-0.9.5-i2v", "16:9");
        r.mode = "i2v".into();
        r.duration = Some(4);
        r.image_url = Some("/outputs/still.png".into());
        let Submitted::Queued(h) = comfy.submit(&ctx, BASE, &r).await.unwrap() else { panic!("expected a queued job") };
        assert_eq!((h.request_id.as_str(), h.status_url.as_str()), (JOB, status.as_str()));
        assert!(matches!(comfy.poll(&ctx, BASE, &h).await.unwrap(), Polled::Running(Some(p)) if (p - 0.5).abs() < 1e-9));
        let Polled::Done(outs) = comfy.poll(&ctx, BASE, &h).await.unwrap() else { panic!("expected done") };
        assert_eq!(outs[0].kind, "video");
        assert!(outs[0].url.ends_with(".mp4"));
        let saved = std::fs::read(ctx.outputs_dir.join(outs[0].url.trim_start_matches("/outputs/"))).unwrap();
        assert_eq!(saved, b"fake mp4 bytes");
        let calls = comfy.transport.calls.lock().unwrap();
        assert_eq!(calls[0].0, "UPLOAD");
        assert_eq!(calls[0].3.as_deref(), Some("image/png"));
        let (method, url, body, idem) = &calls[1];
        assert_eq!((method.as_str(), url.as_str()), ("POST", format!("{BASE}/api/v2/jobs").as_str()));
        assert_eq!(idem.as_deref().map(str::len), Some(36), "a UUID-shaped Idempotency-Key on every submit");
        let image = &body.as_ref().unwrap()["workflow"]["78"]["inputs"]["image"];
        assert_eq!((image["__type"].as_str(), image["info"]["id"].as_str()), (Some("core/ASSET"), Some(ASSET)));
        assert_eq!(image["info"]["hash"], "blake3:9f8a1c0d", "the hash from the upload answer goes along");
        assert!(calls.iter().all(|c| c.1.starts_with(BASE)), "every call goes to the ComfyUI address only");
    }

    #[tokio::test]
    async fn comfy_refuses_links_off_its_address_and_reports_failures() {
        let ctx = test_ctx("comfy_refuse");
        let t = FakeComfy::default();
        t.answer_json("POST", &format!("{BASE}/api/v2/jobs"), 201, json!({"id": JOB, "status": "queued", "urls": {"self": "http://evil.test/api/v2/jobs/1"}}));
        t.answer_json("POST", &format!("{BASE}/api/v2/jobs"), 422, json!({"error": {"code": "invalid_workflow", "message": "Node 31 (KSampler): value not in list"}}));
        let status = format!("{BASE}/api/v2/jobs/{JOB}");
        t.answer_json("GET", &status, 200, json!({"id": JOB, "status": "succeeded", "outputs": [{"type": "image", "url": "http://evil.test/x.png"}]}));
        t.answer_json("GET", &status, 200, json!({"id": JOB, "status": "failed", "error": {"code": "node_execution_error", "message": "out of memory"}}));
        let comfy = Comfy { transport: t };
        let r = req("flux1-schnell-fp8", "1:1");
        assert!(comfy.submit(&ctx, BASE, &r).await.is_err(), "a job link to another host is refused");
        let Err(ProviderError::Failed(msg)) = comfy.submit(&ctx, BASE, &r).await else { panic!("expected an error") };
        assert!(msg.contains("422") && msg.contains("invalid_workflow") && msg.contains("KSampler"), "{msg}");
        let h = Handle { request_id: JOB.into(), status_url: status.clone(), result_url: None };
        assert!(comfy.poll(&ctx, BASE, &h).await.is_err(), "an output link to another host is refused");
        assert!(matches!(comfy.poll(&ctx, BASE, &h).await.unwrap(), Polled::Failed(m) if m.contains("out of memory")));
        // A job saved before the address changed is not polled at the new address, and a bad address is refused.
        assert!(comfy.poll(&ctx, "http://127.0.0.1:9000", &h).await.is_err());
        assert!(comfy.submit(&ctx, "file:///tmp", &r).await.is_err());
        assert_eq!(comfy.transport.calls.lock().unwrap().len(), 4, "nothing was sent for the refused address");
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
