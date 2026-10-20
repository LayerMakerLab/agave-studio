//! Playground: any fal model with its own form.
//!
//! The model list comes from fal's catalog (api.fal.ai/v1/models) and each model's form from its OpenAPI input
//! schema (same endpoint, expand=openapi-3.0). A run is checked against that schema here, then goes through the
//! job queue like every other job, so it shows in the gallery and survives a restart. Prices come from fal's
//! pricing API on your key; nothing here guesses a price.
//!
//! Test mode (AGAVE_FAKE_PROVIDERS=1) reads a captured sample of real fal models (playground-sample.json) instead
//! of the network, and runs go to the fake provider.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use base64::Engine as _;
use serde::Deserialize;
use serde_json::{json, Map, Value};

use crate::jobs::{self, now_unix, Job};
use crate::media::{self, Ctx, Output};
use crate::providers::{Handle, PResult, Polled, Provider, ProviderError, SubmitReq, Submitted};
use crate::state::AppState;

const FAL_API: &str = "https://api.fal.ai/v1";
/// Jobs from the Playground carry this before the fal endpoint id, so they are never mistaken for models.toml ids.
pub const JOB_PREFIX: &str = "fal:";
const CATALOG_TTL: Duration = Duration::from_secs(10 * 60);
const SPEC_TTL: Duration = Duration::from_secs(6 * 60 * 60);
const PRICE_TTL: Duration = Duration::from_secs(60 * 60);
const MAX_CACHED: usize = 300;
const PAGE_SIZE: u32 = 40;
/// Largest local file sent inline to fal (as a data: URI), and the most for one run.
pub const MAX_INLINE_FILE: u64 = 20 << 20;
const MAX_INLINE_TOTAL: u64 = 40 << 20;
const MAX_TEXT_CHARS: usize = 20_000;
const MAX_JSON_FIELD_BYTES: usize = 64 << 10;
const MAX_LIST_ITEMS: usize = 16;
const MAX_OUTPUT_FILES: usize = 24;
const MAX_SAVED_REQUEST_BYTES: usize = 16 << 10;
/// Fields never shown or sent: sync_mode makes fal answer with inline data instead of file URLs.
const HIDDEN: &[&str] = &["sync_mode"];
/// Fields that stay outside More settings even when optional.
const MAIN: &[&str] = &[
    "prompt", "text", "image_size", "aspect_ratio", "duration", "resolution", "num_images", "generate_audio", "voice",
    "voice_id",
];
/// Catalog categories whose models make files. Others (training, llm, vision...) are listed but not runnable.
pub const CATEGORIES: &[(&str, &str)] = &[
    ("text-to-image", "Text to image"),
    ("image-to-image", "Edit image"),
    ("text-to-video", "Text to video"),
    ("image-to-video", "Image to video"),
    ("video-to-video", "Edit video"),
    ("audio-to-video", "Audio to video"),
    ("text-to-speech", "Voice"),
    ("text-to-audio", "Music and sound"),
    ("audio-to-audio", "Edit audio"),
    ("speech-to-speech", "Change voice"),
    ("video-to-audio", "Sound for video"),
    ("image-to-3d", "Image to 3D"),
    ("text-to-3d", "Text to 3D"),
    ("3d-to-3d", "Edit 3D"),
];

// ---------------------------------------------------------------- form spec

#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Text,
    LongText,
    Integer,
    Number,
    Boolean,
    Choice,
    Media,
    MediaList,
    Json,
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct Field {
    pub name: String,
    pub title: String,
    pub description: String,
    pub kind: Kind,
    /// image, video, audio or file, for media fields.
    pub media: Option<String>,
    pub required: bool,
    pub default: Option<Value>,
    pub example: Option<Value>,
    pub choices: Vec<Value>,
    /// A choice that also takes a JSON object (fal's custom image_size {width, height}).
    pub custom_json: bool,
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub min_strict: bool,
    pub max_strict: bool,
    pub max_len: Option<usize>,
    pub advanced: bool,
    /// fal's own note on file limits ("Max file size: 50.0MB, Min width: 300px...").
    pub limits: Option<String>,
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct Spec {
    pub id: String,
    pub name: String,
    pub category: String,
    pub description: String,
    pub status: String,
    pub thumb: Option<String>,
    pub page: String,
    pub fields: Vec<Field>,
    /// File kinds the model answers with (image, video, audio, model).
    pub outputs: Vec<String>,
    pub runnable: bool,
    pub why_not: Option<String>,
}

/// A fal endpoint id: owner/name[/more], plain path characters only, so it is safe in a URL path.
pub fn valid_endpoint(id: &str) -> bool {
    let segs: Vec<&str> = id.split('/').collect();
    id.len() <= 200
        && (2..=8).contains(&segs.len())
        && segs.iter().all(|s| {
            !s.is_empty()
                && !s.starts_with('.')
                && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        })
        && !id.contains("..")
}

fn clean_text(s: &str, max: usize) -> String {
    let flat: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max {
        return flat;
    }
    let cut: String = flat.chars().take(max).collect();
    format!("{}…", cut.trim_end())
}

/// Follows a local "#/components/schemas/X" reference (a few levels at most).
fn resolve<'a>(schema: &'a Value, root: &'a Value) -> &'a Value {
    let mut cur = schema;
    for _ in 0..8 {
        let Some(r) = cur.get("$ref").and_then(Value::as_str) else { return cur };
        let Some(name) = r.strip_prefix("#/components/schemas/") else { return cur };
        match root.pointer(&format!("/components/schemas/{name}")) {
            Some(next) => cur = next,
            None => return cur,
        }
    }
    cur
}

/// The non-null alternatives of anyOf/oneOf (or the schema itself), each resolved; and whether null is allowed.
fn alternatives<'a>(p: &'a Value, root: &'a Value) -> (Vec<&'a Value>, bool) {
    let p = resolve(p, root);
    let list = p.get("anyOf").or_else(|| p.get("oneOf")).and_then(Value::as_array);
    let Some(list) = list else {
        if let Some(all) = p.get("allOf").and_then(Value::as_array).filter(|a| a.len() == 1) {
            return (vec![resolve(&all[0], root)], false);
        }
        return (vec![p], false);
    };
    let mut alts = Vec::new();
    let mut nullable = false;
    for a in list {
        let a = resolve(a, root);
        if a.get("type").and_then(Value::as_str) == Some("null") {
            nullable = true;
        } else {
            alts.push(a);
        }
    }
    (alts, nullable)
}

fn media_hint(p: &Value) -> Option<String> {
    let ui = p.pointer("/ui/field").or_else(|| p.get("_fal_ui_field")).and_then(Value::as_str)?;
    match ui {
        "image" | "video" | "audio" | "file" => Some(ui.to_string()),
        "image_url" => Some("image".into()),
        _ => None,
    }
}

/// image/video/audio from a field name like start_image_url or audio_urls, else file.
fn media_from_name(name: &str) -> String {
    let n = name.to_ascii_lowercase();
    for k in ["image", "video", "audio"] {
        if n.contains(k) {
            return k.to_string();
        }
    }
    if n.contains("mask") {
        return "image".into();
    }
    "file".into()
}

fn number_bounds(p: &Value) -> (Option<f64>, Option<f64>, bool, bool) {
    // OpenAPI 3.0 writes exclusiveMinimum as a flag on minimum; JSON Schema writes it as the bound itself.
    let mut min = p.get("minimum").and_then(Value::as_f64);
    let mut max = p.get("maximum").and_then(Value::as_f64);
    let mut min_strict = p.get("exclusiveMinimum").and_then(Value::as_bool).unwrap_or(false);
    let mut max_strict = p.get("exclusiveMaximum").and_then(Value::as_bool).unwrap_or(false);
    if let Some(v) = p.get("exclusiveMinimum").and_then(Value::as_f64) {
        min = Some(v);
        min_strict = true;
    }
    if let Some(v) = p.get("exclusiveMaximum").and_then(Value::as_f64) {
        max = Some(v);
        max_strict = true;
    }
    (min, max, min_strict, max_strict)
}

fn compile_field(name: &str, raw: &Value, root: &Value, required: bool) -> Field {
    let outer = resolve(raw, root);
    let (alts, nullable) = alternatives(raw, root);
    let title = outer.get("title").and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| name.replace('_', " "));
    let description = outer.get("description").and_then(Value::as_str).map(|d| clean_text(d, 600)).unwrap_or_default();
    let default = outer.get("default").cloned().filter(|v| !v.is_null());
    let example = outer.get("examples").and_then(Value::as_array).and_then(|e| e.first()).cloned();
    let mut f = Field {
        name: name.to_string(),
        title,
        description,
        kind: Kind::Json,
        media: None,
        required: required && !nullable,
        default,
        example,
        choices: Vec::new(),
        custom_json: false,
        min: None,
        max: None,
        min_strict: false,
        max_strict: false,
        max_len: None,
        advanced: false,
        limits: None,
    };
    // An enum among the alternatives makes a choice; an object beside it may still be sent as JSON.
    if let Some(e) = alts.iter().find_map(|a| a.get("enum").and_then(Value::as_array)) {
        f.kind = Kind::Choice;
        f.choices = e.iter().filter(|v| !v.is_null()).cloned().collect();
        f.custom_json = alts.iter().any(|a| a.get("type").and_then(Value::as_str) == Some("object") || a.get("properties").is_some());
        return f;
    }
    if alts.len() != 1 {
        return f; // several shapes: edited as JSON
    }
    let p = alts[0];
    f.limits = p.get("limit_description").and_then(Value::as_str).map(|s| clean_text(s, 300));
    match p.get("type").and_then(Value::as_str) {
        Some("string") => {
            f.max_len = p.get("maxLength").and_then(Value::as_u64).map(|n| n as usize);
            if let Some(m) = media_hint(p) {
                f.kind = Kind::Media;
                f.media = Some(if m == "file" { media_from_name(name) } else { m });
            } else if name.ends_with("_url") && name != "webhook_url" {
                f.kind = Kind::Media;
                f.media = Some(media_from_name(name));
            } else if name.contains("prompt") || name == "text" || f.max_len.is_some_and(|n| n > 300) {
                f.kind = Kind::LongText;
            } else {
                f.kind = Kind::Text;
            }
        }
        Some("integer") | Some("number") => {
            f.kind = if p.get("type").and_then(Value::as_str) == Some("integer") { Kind::Integer } else { Kind::Number };
            (f.min, f.max, f.min_strict, f.max_strict) = number_bounds(p);
        }
        Some("boolean") => f.kind = Kind::Boolean,
        Some("array") => {
            let items = p.get("items").map(|i| resolve(i, root));
            let item_media = items.and_then(|i| {
                (i.get("type").and_then(Value::as_str) == Some("string"))
                    .then(|| media_hint(i).or_else(|| name.ends_with("_urls").then(|| media_from_name(name))))
                    .flatten()
            });
            if let Some(m) = item_media {
                f.kind = Kind::MediaList;
                f.media = Some(if m == "file" { media_from_name(name) } else { m });
                f.limits = items.and_then(|i| i.get("limit_description")).and_then(Value::as_str).map(|s| clean_text(s, 300));
                f.max_len = p.get("maxItems").and_then(Value::as_u64).map(|n| n as usize);
            }
        }
        _ => {}
    }
    f
}

/// The POST operation's input schema and the result's output schema, from a fal queue OpenAPI document.
fn io_schemas<'a>(openapi: &'a Value, id: &str) -> (Option<&'a Value>, Option<&'a Value>) {
    let paths = openapi.get("paths").and_then(Value::as_object);
    let mut input = None;
    let mut output = None;
    for (path, ops) in paths.into_iter().flatten() {
        if path.contains("/requests/") {
            if path.ends_with("{request_id}") {
                output = ops.pointer("/get/responses/200/content/application~1json/schema");
            }
        } else if path.trim_start_matches('/') == id || input.is_none() {
            if let Some(s) = ops.pointer("/post/requestBody/content/application~1json/schema") {
                input = Some(s);
            }
        }
    }
    (input.map(|s| resolve(s, openapi)), output.map(|s| resolve(s, openapi)))
}

/// File kinds an output schema can contain: properties that are fal File objects (or lists of them).
fn output_kinds(out: &Value, root: &Value) -> Vec<String> {
    let mut kinds = Vec::new();
    let props = out.get("properties").and_then(Value::as_object);
    for (name, p) in props.into_iter().flatten() {
        let (alts, _) = alternatives(p, root);
        for a in alts {
            let target = if a.get("type").and_then(Value::as_str) == Some("array") {
                a.get("items").map(|i| resolve(i, root)).unwrap_or(a)
            } else {
                a
            };
            let is_file = target.pointer("/properties/url").is_some();
            if !is_file {
                continue;
            }
            let n = name.to_ascii_lowercase();
            let kind = if n.contains("video") {
                "video"
            } else if n.contains("audio") || n.contains("speech") || n.contains("voice") {
                "audio"
            } else if n.contains("mesh") || n.contains("glb") || n.contains("model") || n.contains("3d") {
                "model"
            } else if n.contains("image") || n.contains("mask") {
                "image"
            } else {
                "file"
            };
            if !kinds.iter().any(|k| k == kind) {
                kinds.push(kind.to_string());
            }
        }
    }
    kinds
}

/// Turns one catalog entry (with expand=openapi-3.0) into the form spec.
pub fn compile(model: &Value, test_mode: bool) -> Option<Spec> {
    let id = model.get("endpoint_id").and_then(Value::as_str)?.to_string();
    if !valid_endpoint(&id) {
        return None;
    }
    let md = model.get("metadata").cloned().unwrap_or(Value::Null);
    let s = |k: &str| md.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let thumb = if test_mode { None } else { md.get("thumbnail_url").and_then(Value::as_str).filter(|u| media::parse_https_url(u).is_some()).map(str::to_string) };
    let mut spec = Spec {
        name: if s("display_name").is_empty() { id.clone() } else { clean_text(&s("display_name"), 120) },
        category: s("category"),
        description: clean_text(&s("description"), 600),
        status: s("status"),
        thumb,
        page: format!("https://fal.ai/models/{id}"),
        fields: Vec::new(),
        outputs: Vec::new(),
        runnable: false,
        why_not: None,
        id,
    };
    let Some(openapi) = model.get("openapi").filter(|o| o.is_object()) else {
        spec.why_not = Some("fal did not publish an input form for this model.".into());
        return Some(spec);
    };
    let (input, output) = io_schemas(openapi, &spec.id);
    let Some(input) = input else {
        spec.why_not = Some("fal did not publish an input form for this model.".into());
        return Some(spec);
    };
    let required: Vec<&str> = input.get("required").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).collect();
    let props = input.get("properties").and_then(Value::as_object);
    let mut order: Vec<String> = input
        .get("x-fal-order-properties")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect();
    for k in props.into_iter().flatten().map(|(k, _)| k) {
        if !order.contains(k) {
            order.push(k.clone());
        }
    }
    for name in order {
        if HIDDEN.contains(&name.as_str()) {
            continue;
        }
        let Some(raw) = props.and_then(|p| p.get(&name)) else { continue };
        let mut f = compile_field(&name, raw, openapi, required.contains(&name.as_str()));
        f.advanced = !(f.required || matches!(f.kind, Kind::Media | Kind::MediaList) || MAIN.contains(&name.as_str()));
        spec.fields.push(f);
    }
    // What you type first, then the files it needs, then the main options; More settings keep fal's order.
    spec.fields.sort_by_key(|f| match (f.kind, f.advanced) {
        (_, true) => 4,
        (Kind::LongText, _) if f.name == "prompt" || f.name == "text" => 0,
        (Kind::Media | Kind::MediaList, _) if f.required => 1,
        (Kind::Media | Kind::MediaList, _) => 2,
        _ => 3,
    });
    spec.outputs = output.map(|o| output_kinds(o, openapi)).unwrap_or_default();
    if spec.status == "deprecated" {
        spec.why_not = Some("fal has retired this model.".into());
    } else if spec.outputs.is_empty() {
        spec.why_not = Some("This model answers with text or data, and the Playground shows files only for now.".into());
    } else {
        spec.runnable = true;
    }
    Some(spec)
}

// ---------------------------------------------------------------- input checks

/// A refused run: the error code, and the field it is about.
#[derive(Debug, PartialEq)]
pub struct Bad {
    pub code: &'static str,
    pub field: Option<String>,
}

fn bad(code: &'static str, field: &str) -> Bad {
    Bad { code, field: Some(field.to_string()) }
}

fn num_eq(a: &Value, b: &Value) -> bool {
    match (a.as_f64(), b.as_f64()) {
        (Some(x), Some(y)) => x == y,
        _ => a == b,
    }
}

/// A local file the run will send inline: its name under data/outputs.
fn local_name(s: &str) -> Option<&str> {
    s.strip_prefix("/outputs/").filter(|n| media::valid_output_name(n))
}

fn valid_media_value(v: &Value, locals: &mut Vec<String>) -> bool {
    let Some(s) = v.as_str() else { return false };
    if let Some(name) = local_name(s) {
        locals.push(name.to_string());
        return true;
    }
    if let Some(rest) = s.strip_prefix("data:") {
        return rest.split_once(',').is_some_and(|(meta, _)| meta.ends_with(";base64"));
    }
    media::parse_https_url(s).is_some()
}

/// Local files named anywhere inside a JSON field (Kling elements and the like).
fn nested_locals(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::String(s) => {
            if let Some(n) = local_name(s) {
                out.push(n.to_string());
            }
        }
        Value::Array(a) => a.iter().for_each(|x| nested_locals(x, out)),
        Value::Object(o) => o.values().for_each(|x| nested_locals(x, out)),
        _ => {}
    }
}

/// Checks a run against the form spec. Returns the input to send (nulls dropped) and the local files it names.
pub fn check_input(spec: &Spec, input: &Map<String, Value>) -> Result<(Map<String, Value>, Vec<String>), Bad> {
    let mut out = Map::new();
    let mut locals = Vec::new();
    for (k, v) in input {
        let Some(f) = spec.fields.iter().find(|f| &f.name == k) else {
            return Err(bad("unknown_field", k));
        };
        if v.is_null() {
            continue;
        }
        let ok = match f.kind {
            Kind::Text | Kind::LongText => v.as_str().is_some_and(|s| {
                let n = s.chars().count();
                n <= MAX_TEXT_CHARS && f.max_len.is_none_or(|m| n <= m)
            }),
            Kind::Integer | Kind::Number => {
                let n = if f.kind == Kind::Integer { v.as_i64().map(|i| i as f64) } else { v.as_f64() };
                n.is_some_and(|n| {
                    let lo = f.min.is_none_or(|m| if f.min_strict { n > m } else { n >= m });
                    let hi = f.max.is_none_or(|m| if f.max_strict { n < m } else { n <= m });
                    lo && hi
                })
            }
            Kind::Boolean => v.is_boolean(),
            Kind::Choice => f.choices.iter().any(|c| num_eq(c, v)) || (f.custom_json && v.is_object()),
            Kind::Media => valid_media_value(v, &mut locals),
            Kind::MediaList => v.as_array().is_some_and(|a| {
                a.len() <= f.max_len.unwrap_or(MAX_LIST_ITEMS).min(MAX_LIST_ITEMS) && a.iter().all(|x| valid_media_value(x, &mut locals))
            }),
            Kind::Json => {
                let small = serde_json::to_vec(v).map(|b| b.len() <= MAX_JSON_FIELD_BYTES).unwrap_or(false);
                if small {
                    nested_locals(v, &mut locals);
                }
                small
            }
        };
        if !ok {
            return Err(bad("invalid_value", k));
        }
        out.insert(k.clone(), v.clone());
    }
    for f in spec.fields.iter().filter(|f| f.required) {
        if !out.contains_key(&f.name) {
            return Err(bad("field_required", &f.name));
        }
    }
    Ok((out, locals))
}

// ---------------------------------------------------------------- prices

/// What fal says a run costs: the list price per unit and fal's own average per run, when it gives them.
#[derive(Clone, Debug, serde::Serialize)]
pub struct Price {
    pub unit_price: Option<f64>,
    pub unit: Option<String>,
    pub currency: Option<String>,
    /// fal's historical average cost of one call (pricing/estimate, historical_api_price).
    pub average: Option<f64>,
    pub checked_at: u64,
    pub test: bool,
}

// ---------------------------------------------------------------- cache

#[derive(Default)]
pub struct Cache {
    catalog: Mutex<HashMap<String, (Instant, Value)>>,
    specs: Mutex<HashMap<String, (Instant, Arc<Spec>)>>,
    prices: Mutex<HashMap<String, (Instant, Price)>>,
    /// Test mode: the captured sample (catalog entries with schemas, and prices).
    sample: Option<Value>,
}

fn get_fresh<V: Clone>(m: &Mutex<HashMap<String, (Instant, V)>>, k: &str, ttl: Duration) -> Option<V> {
    let g = m.lock().unwrap_or_else(|e| e.into_inner());
    g.get(k).filter(|(t, _)| t.elapsed() < ttl).map(|(_, v)| v.clone())
}

fn put<V>(m: &Mutex<HashMap<String, (Instant, V)>>, k: String, v: V) {
    let mut g = m.lock().unwrap_or_else(|e| e.into_inner());
    if g.len() >= MAX_CACHED {
        if let Some(oldest) = g.iter().min_by_key(|(_, (t, _))| *t).map(|(k, _)| k.clone()) {
            g.remove(&oldest);
        }
    }
    g.insert(k, (Instant::now(), v));
}

impl Cache {
    /// The Playground cache; in test mode it holds the captured sample instead of reaching fal.
    pub fn new(test_mode: bool) -> Self {
        let sample = test_mode.then(|| serde_json::from_str(include_str!("../playground-sample.json")).unwrap_or(Value::Null));
        Cache { sample, ..Default::default() }
    }
}

// ---------------------------------------------------------------- fal reads

enum Fetch {
    Ok(Value),
    NotFound,
    Busy,
    Failed,
}

/// GET from fal's platform API. The key, when set, goes only to api.fal.ai (it lifts fal's rate limit).
async fn fal_get(ctx: &Ctx, key: Option<&str>, path: &str, query: &[(&str, String)]) -> Fetch {
    let mut req = ctx.http.get(format!("{FAL_API}{path}")).query(query).timeout(Duration::from_secs(30));
    if let Some(k) = key {
        req = req.header("Authorization", format!("Key {k}"));
    }
    let Ok(resp) = req.send().await else { return Fetch::Failed };
    match resp.status().as_u16() {
        200 => match media::read_capped(resp, 8 << 20).await {
            Ok(b) => serde_json::from_slice(&b).map(Fetch::Ok).unwrap_or(Fetch::Failed),
            Err(_) => Fetch::Failed,
        },
        404 => Fetch::NotFound,
        429 => Fetch::Busy,
        _ => Fetch::Failed,
    }
}

fn fail(status: StatusCode, code: &str) -> Response {
    (status, Json(json!({"error": code}))).into_response()
}

fn catalog_entry(m: &Value, test_mode: bool) -> Option<Value> {
    let id = m.get("endpoint_id").and_then(Value::as_str).filter(|i| valid_endpoint(i))?;
    let md = m.get("metadata").cloned().unwrap_or(Value::Null);
    let s = |k: &str| md.get(k).and_then(Value::as_str).unwrap_or("");
    let category = s("category");
    let thumb = if test_mode { None } else { md.get("thumbnail_url").and_then(Value::as_str).filter(|u| media::parse_https_url(u).is_some()) };
    Some(json!({
        "id": id,
        "name": if s("display_name").is_empty() { id.to_string() } else { clean_text(s("display_name"), 120) },
        "category": category,
        "description": clean_text(s("description"), 240),
        "thumb": thumb,
        "makes_files": CATEGORIES.iter().any(|(c, _)| *c == category),
    }))
}

#[derive(Deserialize)]
pub struct CatalogQuery {
    q: Option<String>,
    category: Option<String>,
    cursor: Option<String>,
}

/// GET /api/fal/models?q=&category=&cursor= : one page of fal's catalog (active models only).
pub async fn models(State(st): State<AppState>, Query(q): Query<CatalogQuery>) -> Response {
    let text = q.q.unwrap_or_default().trim().to_string();
    let category = q.category.unwrap_or_default();
    let cursor = q.cursor.unwrap_or_default();
    if text.chars().count() > 100 || cursor.len() > 400 || !cursor.chars().all(|c| c.is_ascii_alphanumeric() || "-_=+/.".contains(c)) {
        return fail(StatusCode::BAD_REQUEST, "bad_query");
    }
    if !category.is_empty() && !CATEGORIES.iter().any(|(c, _)| *c == category) {
        return fail(StatusCode::BAD_REQUEST, "unknown_category");
    }
    let cats: Vec<Value> = CATEGORIES.iter().map(|(id, label)| json!({"id": id, "label": label})).collect();
    if let Some(sample) = &st.playground.sample {
        let needle = text.to_ascii_lowercase();
        let list: Vec<Value> = sample
            .get("models")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|m| category.is_empty() || m.pointer("/metadata/category").and_then(Value::as_str) == Some(category.as_str()))
            .filter(|m| needle.is_empty() || m.to_string().to_ascii_lowercase().contains(&needle))
            .filter_map(|m| catalog_entry(m, true))
            .collect();
        return Json(json!({"models": list, "next_cursor": null, "has_more": false, "categories": cats, "test": true})).into_response();
    }
    let cache_key = format!("{text}\u{1}{category}\u{1}{cursor}");
    if let Some(v) = get_fresh(&st.playground.catalog, &cache_key, CATALOG_TTL) {
        return Json(v).into_response();
    }
    let mut query = vec![("limit", PAGE_SIZE.to_string()), ("status", "active".to_string())];
    if !text.is_empty() {
        query.push(("q", text));
    }
    if !category.is_empty() {
        query.push(("category", category));
    }
    if !cursor.is_empty() {
        query.push(("cursor", cursor));
    }
    let key = st.keys.get("FAL_KEY");
    match fal_get(&st.ctx, key.as_deref(), "/models", &query).await {
        Fetch::Ok(v) => {
            let list: Vec<Value> = v.get("models").and_then(Value::as_array).into_iter().flatten().filter_map(|m| catalog_entry(m, false)).collect();
            let next = v.get("next_cursor").and_then(Value::as_str).filter(|c| c.len() <= 400);
            let out = json!({"models": list, "next_cursor": next, "has_more": next.is_some(), "categories": cats});
            put(&st.playground.catalog, cache_key, out.clone());
            Json(out).into_response()
        }
        Fetch::Busy => fail(StatusCode::SERVICE_UNAVAILABLE, "catalog_busy"),
        _ => fail(StatusCode::BAD_GATEWAY, "catalog_unavailable"),
    }
}

/// The form spec for one model, from the cache or fal (or the test sample).
pub async fn spec_for(st: &AppState, id: &str) -> Result<Arc<Spec>, Response> {
    if !valid_endpoint(id) {
        return Err(fail(StatusCode::BAD_REQUEST, "bad_model_id"));
    }
    if let Some(sample) = &st.playground.sample {
        let m = sample.get("models").and_then(Value::as_array).into_iter().flatten().find(|m| m.get("endpoint_id").and_then(Value::as_str) == Some(id));
        return m.and_then(|m| compile(m, true)).map(Arc::new).ok_or_else(|| fail(StatusCode::NOT_FOUND, "unknown_fal_model"));
    }
    if let Some(s) = get_fresh(&st.playground.specs, id, SPEC_TTL) {
        return Ok(s);
    }
    let key = st.keys.get("FAL_KEY");
    let query = [("endpoint_id", id.to_string()), ("expand", "openapi-3.0".to_string())];
    match fal_get(&st.ctx, key.as_deref(), "/models", &query).await {
        Fetch::Ok(v) => {
            let m = v.get("models").and_then(Value::as_array).into_iter().flatten().find(|m| m.get("endpoint_id").and_then(Value::as_str) == Some(id));
            let Some(spec) = m.and_then(|m| compile(m, false)) else {
                return Err(fail(StatusCode::NOT_FOUND, "unknown_fal_model"));
            };
            let spec = Arc::new(spec);
            put(&st.playground.specs, id.to_string(), spec.clone());
            Ok(spec)
        }
        Fetch::NotFound => Err(fail(StatusCode::NOT_FOUND, "unknown_fal_model")),
        Fetch::Busy => Err(fail(StatusCode::SERVICE_UNAVAILABLE, "catalog_busy")),
        Fetch::Failed => Err(fail(StatusCode::BAD_GATEWAY, "catalog_unavailable")),
    }
}

#[derive(Deserialize)]
pub struct IdQuery {
    id: String,
}

/// GET /api/fal/model?id= : the model's form.
pub async fn model(State(st): State<AppState>, Query(q): Query<IdQuery>) -> Response {
    match spec_for(&st, &q.id).await {
        Ok(spec) => Json(json!({"model": *spec})).into_response(),
        Err(r) => r,
    }
}

/// fal's list price and average per run for one model (needs the key). None when fal gives neither.
async fn price_for(st: &AppState, id: &str) -> Option<Price> {
    if let Some(sample) = &st.playground.sample {
        let p = sample.pointer(&format!("/prices/{}", id.replace('~', "~0").replace('/', "~1")))?;
        return Some(Price {
            unit_price: p.get("unit_price").and_then(Value::as_f64),
            unit: p.get("unit").and_then(Value::as_str).map(str::to_string),
            currency: p.get("currency").and_then(Value::as_str).map(str::to_string),
            average: None,
            checked_at: now_unix(),
            test: true,
        });
    }
    if let Some(p) = get_fresh(&st.playground.prices, id, PRICE_TTL) {
        return Some(p);
    }
    let key = st.keys.get("FAL_KEY")?;
    let list_query = [("endpoint_id", id.to_string())];
    let list = fal_get(&st.ctx, Some(&key), "/models/pricing", &list_query);
    let estimate = async {
        let body = json!({"estimate_type": "historical_api_price", "endpoints": {id: {"call_quantity": 1}}});
        let resp = st
            .ctx
            .http
            .post(format!("{FAL_API}/models/pricing/estimate"))
            .header("Authorization", format!("Key {key}"))
            .timeout(Duration::from_secs(30))
            .json(&body)
            .send()
            .await
            .ok()?;
        if !resp.status().is_success() {
            return None;
        }
        let b = media::read_capped(resp, 1 << 20).await.ok()?;
        let v: Value = serde_json::from_slice(&b).ok()?;
        v.get("total_cost").and_then(Value::as_f64).filter(|c| c.is_finite() && *c >= 0.0)
    };
    let (list, average) = tokio::join!(list, estimate);
    let row = match list {
        Fetch::Ok(v) => v.get("prices").and_then(Value::as_array).and_then(|a| a.iter().find(|p| p.get("endpoint_id").and_then(Value::as_str) == Some(id)).cloned()),
        _ => None,
    };
    let unit_price = row.as_ref().and_then(|r| r.get("unit_price")).and_then(Value::as_f64).filter(|p| p.is_finite() && *p >= 0.0);
    if unit_price.is_none() && average.is_none() {
        return None;
    }
    let price = Price {
        unit_price,
        unit: row.as_ref().and_then(|r| r.get("unit")).and_then(Value::as_str).map(|u| clean_text(u, 40)),
        currency: row.as_ref().and_then(|r| r.get("currency")).and_then(Value::as_str).map(|c| clean_text(c, 3)),
        average,
        checked_at: now_unix(),
        test: false,
    };
    put(&st.playground.prices, id.to_string(), price.clone());
    Some(price)
}

/// GET /api/fal/price?id= : fal's list price and average per run.
pub async fn price(State(st): State<AppState>, Query(q): Query<IdQuery>) -> Response {
    if !valid_endpoint(&q.id) {
        return fail(StatusCode::BAD_REQUEST, "bad_model_id");
    }
    if st.playground.sample.is_none() && st.keys.get("FAL_KEY").is_none() {
        return fail(StatusCode::PRECONDITION_FAILED, "missing_key");
    }
    match price_for(&st, &q.id).await {
        Some(p) => Json(json!({"price": p})).into_response(),
        None => fail(StatusCode::BAD_GATEWAY, "price_unavailable"),
    }
}

// ---------------------------------------------------------------- runs

#[derive(Deserialize)]
pub struct RunReq {
    id: String,
    input: Map<String, Value>,
}

fn fake_mode(category: &str) -> &'static str {
    if category.ends_with("-to-video") {
        "t2v"
    } else if category.ends_with("-to-3d") {
        "i23d"
    } else if category.ends_with("-to-audio") || category.ends_with("-to-speech") {
        "tts"
    } else {
        "t2i"
    }
}

/// POST /api/fal/run {id, input} : checks the input against the model's form and queues the job.
pub async fn run(State(st): State<AppState>, body: Result<Json<RunReq>, axum::extract::rejection::JsonRejection>) -> Response {
    let Ok(Json(req)) = body else {
        return fail(StatusCode::BAD_REQUEST, "bad_request");
    };
    let spec = match spec_for(&st, &req.id).await {
        Ok(s) => s,
        Err(r) => return r,
    };
    if !spec.runnable {
        return (StatusCode::BAD_REQUEST, Json(json!({"error": "not_runnable", "why": spec.why_not}))).into_response();
    }
    let (input, locals) = match check_input(&spec, &req.input) {
        Ok(v) => v,
        Err(b) => return (StatusCode::BAD_REQUEST, Json(json!({"error": b.code, "field": b.field}))).into_response(),
    };
    // Local files go inline, so each is checked for size now rather than failing after the job starts.
    let mut total = 0u64;
    for name in &locals {
        let Ok(meta) = tokio::fs::metadata(st.ctx.outputs_dir.join(name)).await else {
            return (StatusCode::BAD_REQUEST, Json(json!({"error": "file_missing"}))).into_response();
        };
        if meta.len() > MAX_INLINE_FILE {
            return (StatusCode::PAYLOAD_TOO_LARGE, Json(json!({"error": "file_too_large", "max_mb": MAX_INLINE_FILE >> 20}))).into_response();
        }
        total += meta.len();
    }
    if total > MAX_INLINE_TOTAL {
        return (StatusCode::PAYLOAD_TOO_LARGE, Json(json!({"error": "files_too_large", "max_mb": MAX_INLINE_TOTAL >> 20}))).into_response();
    }
    let (adapter, key): (Arc<dyn Provider>, String) = if let Some(fake) = &st.fake {
        (fake.clone() as Arc<dyn Provider>, String::new())
    } else {
        let Some(key) = st.keys.get("FAL_KEY") else {
            return (StatusCode::PRECONDITION_FAILED, Json(json!({"error": "missing_key", "key": "FAL_KEY"}))).into_response();
        };
        (Arc::new(FalRun { input: Value::Object(input.clone()) }) as Arc<dyn Provider>, key)
    };
    let prompt = ["prompt", "text"].iter().find_map(|k| input.get(*k).and_then(Value::as_str)).map(|p| clean_text(p, 2000)).unwrap_or_else(|| spec.name.clone());
    let price = price_for(&st, &spec.id).await;
    let request = json!({"playground": {"id": spec.id, "input": input}, "name": spec.name, "price": price});
    let job = Job {
        id: format!("j_{}", media::random_hex(8)),
        model: format!("{JOB_PREFIX}{}", spec.id),
        mode: spec.category.clone(),
        prompt: prompt.clone(),
        status: "queued".into(),
        progress: None,
        outputs: Vec::new(),
        error: None,
        created_at: now_unix(),
        request: Some(request).filter(|v| v.to_string().len() <= MAX_SAVED_REQUEST_BYTES),
    };
    let aspect = input.get("aspect_ratio").and_then(Value::as_str).filter(|a| matches!(*a, "16:9" | "9:16" | "1:1")).map(str::to_string);
    let sub = SubmitReq {
        endpoint: spec.id.clone(),
        mode: fake_mode(&spec.category).into(),
        prompt,
        aspect,
        duration: None,
        resolution: None,
        seed: None,
        image_url: None,
        image_urls: Vec::new(),
        voice_id: None,
        label: Some(spec.name.clone()),
        audio: None,
        elements: Vec::new(),
        keyframe: None,
        end_image_url: None,
    };
    if !st.jobs.insert(job.clone(), adapter.clone(), "FAL_KEY".into()) {
        return fail(StatusCode::TOO_MANY_REQUESTS, "too_many_jobs");
    }
    jobs::spawn_submit(st.clone(), job.id.clone(), adapter, key, sub);
    (StatusCode::ACCEPTED, Json(json!({"job": job}))).into_response()
}

/// The adapter for a Playground job picked up again after a restart (it is only polled).
pub fn adapter_for_job(model: &str) -> Option<Arc<dyn Provider>> {
    let id = model.strip_prefix(JOB_PREFIX)?;
    valid_endpoint(id).then(|| Arc::new(FalRun { input: Value::Null }) as Arc<dyn Provider>)
}

// ---------------------------------------------------------------- fal adapter for Playground runs

/// Sends the checked input to fal's queue as it is, and saves every file in the result.
pub struct FalRun {
    input: Value,
}

fn mime_for(name: &str) -> Option<&'static str> {
    Some(match name.rsplit('.').next()?.to_ascii_lowercase().as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "mp4" | "m4v" => "video/mp4",
        "mov" => "video/quicktime",
        "webm" => "video/webm",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "m4a" | "aac" => "audio/mp4",
        "ogg" | "opus" => "audio/ogg",
        "flac" => "audio/flac",
        "glb" => "model/gltf-binary",
        _ => return None,
    })
}

/// Replaces /outputs/<name> strings anywhere in the input with data: URIs (fal cannot read the studio's files).
async fn inline_locals(ctx: &Ctx, v: &mut Value) -> PResult<()> {
    match v {
        Value::String(s) => {
            if let Some(name) = local_name(s).map(str::to_string) {
                let mime = mime_for(&name).ok_or_else(|| ProviderError::Failed("That file type cannot be sent to fal.".into()))?;
                let path = ctx.outputs_dir.join(&name);
                let size = tokio::fs::metadata(&path).await.map_err(|_| ProviderError::Failed("A picked file is gone. Pick it again.".into()))?.len();
                if size > MAX_INLINE_FILE {
                    return Err(ProviderError::Failed(format!("A picked file is over {} MB.", MAX_INLINE_FILE >> 20)));
                }
                let bytes = tokio::fs::read(&path).await.map_err(|_| ProviderError::Failed("A picked file could not be read.".into()))?;
                *s = format!("data:{mime};base64,{}", base64::engine::general_purpose::STANDARD.encode(bytes));
            }
        }
        Value::Array(a) => {
            for x in a {
                Box::pin(inline_locals(ctx, x)).await?;
            }
        }
        Value::Object(o) => {
            for x in o.values_mut() {
                Box::pin(inline_locals(ctx, x)).await?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// fal's validation answer ({"detail": [{"loc": [..., "image_url"], "msg": "..."}]}) as one plain line.
fn fal_detail(body: &str) -> String {
    let v: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let detail = v.get("detail").or_else(|| v.get("error"));
    let line = match detail {
        Some(Value::Array(items)) => items
            .iter()
            .take(3)
            .map(|d| {
                let field = d.get("loc").and_then(Value::as_array).and_then(|l| l.last()).and_then(Value::as_str).unwrap_or("");
                let msg = d.get("msg").and_then(Value::as_str).unwrap_or("not accepted");
                if field.is_empty() { msg.to_string() } else { format!("{field}: {msg}") }
            })
            .collect::<Vec<_>>()
            .join("; "),
        Some(Value::String(s)) => s.clone(),
        Some(other) => other.get("message").and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| other.to_string()),
        None => body.to_string(),
    };
    clean_text(&line.chars().map(|c| if c.is_control() { ' ' } else { c }).collect::<String>(), 300)
}

async fn fal_json(resp: reqwest::Response) -> Result<Value, (u16, String)> {
    let status = resp.status().as_u16();
    let body = media::read_capped(resp, media::MAX_JSON_BYTES).await.map_err(|_| (status, "response too large".to_string()))?;
    if !(200..300).contains(&status) {
        return Err((status, fal_detail(&String::from_utf8_lossy(&body))));
    }
    serde_json::from_slice(&body).map_err(|_| (status, "fal answered with something other than JSON".into()))
}

fn queue_url(s: &str) -> PResult<String> {
    let u = media::parse_https_url(s).ok_or_else(|| ProviderError::Failed("fal sent a bad URL".into()))?;
    if u.host_str() == Some("queue.fal.run") {
        Ok(s.to_string())
    } else {
        Err(ProviderError::Failed("refusing a fal URL outside queue.fal.run".into()))
    }
}

/// Every file in a fal result: objects with a url (fal File), with their content type when given.
fn result_files(v: &Value, depth: usize, out: &mut Vec<(String, Option<String>)>) {
    if depth > 8 || out.len() >= MAX_OUTPUT_FILES {
        return;
    }
    match v {
        Value::Object(o) => {
            if let Some(u) = o.get("url").and_then(Value::as_str) {
                out.push((u.to_string(), o.get("content_type").and_then(Value::as_str).map(str::to_string)));
                return;
            }
            o.values().for_each(|x| result_files(x, depth + 1, out));
        }
        Value::Array(a) => a.iter().for_each(|x| result_files(x, depth + 1, out)),
        _ => {}
    }
}

/// The kind and file extension to save a result file as, or None for files the studio does not keep.
fn kind_and_ext(url: &str, content_type: Option<&str>) -> Option<(&'static str, String)> {
    let ct = content_type.unwrap_or("").to_ascii_lowercase();
    let ext = media::ext_from_url(url, "");
    let by_ext = match ext.as_str() {
        "png" | "jpg" | "jpeg" | "webp" | "gif" | "avif" | "heic" => Some("image"),
        "mp4" | "m4v" | "webm" | "mov" => Some("video"),
        "mp3" | "wav" | "m4a" | "aac" | "ogg" | "opus" | "flac" => Some("audio"),
        "glb" | "usdz" | "stl" | "3mf" | "obj" | "fbx" => Some("model"),
        _ => None,
    };
    let by_ct = if ct.starts_with("image/") && !ct.contains("svg") {
        Some(("image", match ct.as_str() {
            "image/jpeg" | "image/jpg" => "jpg",
            "image/webp" => "webp",
            "image/gif" => "gif",
            "image/avif" => "avif",
            _ => "png",
        }))
    } else if ct.starts_with("video/") {
        Some(("video", "mp4"))
    } else if ct.starts_with("audio/") {
        Some(("audio", if ct.contains("wav") { "wav" } else { "mp3" }))
    } else if ct == "model/gltf-binary" {
        Some(("model", "glb"))
    } else {
        None
    };
    match (by_ext, by_ct) {
        (Some(k), _) => Some((k, ext)),
        (None, Some((k, e))) => Some((k, e.to_string())),
        _ => None,
    }
}

#[async_trait]
impl Provider for FalRun {
    async fn submit(&self, ctx: &Ctx, key: &str, req: &SubmitReq) -> PResult<Submitted> {
        let mut input = self.input.clone();
        inline_locals(ctx, &mut input).await?;
        let resp = ctx
            .http
            .post(format!("https://queue.fal.run/{}", req.endpoint).as_str())
            .header("Authorization", format!("Key {key}"))
            .json(&input)
            .send()
            .await
            .map_err(media::net)?;
        let v = fal_json(resp).await.map_err(|(s, msg)| ProviderError::Failed(format!("fal refused the run (HTTP {s}): {msg}")))?;
        let get = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_string).ok_or_else(|| ProviderError::Failed(format!("fal response missing {k}")));
        Ok(Submitted::Queued(Handle {
            request_id: get("request_id")?,
            status_url: queue_url(&get("status_url")?)?,
            result_url: Some(queue_url(&get("response_url")?)?),
        }))
    }

    async fn poll(&self, ctx: &Ctx, key: &str, h: &Handle) -> PResult<Polled> {
        let auth = format!("Key {key}");
        let resp = ctx.http.get(h.status_url.as_str()).header("Authorization", auth.as_str()).send().await.map_err(media::net)?;
        let v = fal_json(resp).await.map_err(|(s, msg)| ProviderError::Failed(format!("fal status HTTP {s}: {msg}")))?;
        match v.get("status").and_then(Value::as_str).unwrap_or("") {
            "IN_QUEUE" | "IN_PROGRESS" => return Ok(Polled::Running(None)),
            "COMPLETED" => {}
            other => return Ok(Polled::Failed(format!("fal status {}", clean_text(other, 40)))),
        }
        if let Some(err) = v.get("error").filter(|e| !e.is_null()) {
            let msg = err.as_str().map(str::to_string).unwrap_or_else(|| err.to_string());
            return Ok(Polled::Failed(format!("fal: {}", fal_detail(&json!({"error": msg}).to_string()))));
        }
        let result_url = h.result_url.as_deref().ok_or_else(|| ProviderError::Failed("missing fal result URL".into()))?;
        let resp = ctx.http.get(result_url).header("Authorization", auth.as_str()).send().await.map_err(media::net)?;
        let result = match fal_json(resp).await {
            Ok(r) => r,
            // fal turned the input down (422 and the like): final, with fal's own words.
            Err((s, msg)) if (400..500).contains(&s) && s != 429 => return Ok(Polled::Failed(format!("fal: {msg}"))),
            Err((s, msg)) => return Err(ProviderError::Failed(format!("fal result HTTP {s}: {msg}"))),
        };
        let mut files = Vec::new();
        result_files(&result, 0, &mut files);
        let mut outs: Vec<Output> = Vec::new();
        for (url, ct) in files {
            let Some((kind, ext)) = kind_and_ext(&url, ct.as_deref()) else { continue };
            let bytes = if let Some(rest) = url.strip_prefix("data:") {
                let Some((_, data)) = rest.split_once(";base64,") else { continue };
                let Ok(b) = base64::engine::general_purpose::STANDARD.decode(data) else { continue };
                b
            } else {
                media::fetch(&ctx.http, &url, &[]).await?.0
            };
            outs.push(media::save(ctx, &bytes, &ext, kind).await?);
        }
        if outs.is_empty() {
            return Ok(Polled::Failed("fal finished but returned no image, video, audio or 3D file.".into()));
        }
        Ok(Polled::Done(outs))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Value {
        serde_json::from_str(include_str!("../playground-sample.json")).expect("sample parses")
    }

    fn sample_spec(id: &str) -> Spec {
        let s = sample();
        let m = s["models"].as_array().unwrap().iter().find(|m| m["endpoint_id"] == id).expect("in sample").clone();
        compile(&m, true).expect("compiles")
    }

    #[test]
    fn endpoint_ids_are_plain_paths() {
        for ok in ["fal-ai/flux/schnell", "google/nano-banana-2.1/edit", "fal-ai/kling-video/v2.5-turbo/pro/image-to-video", "tripo3d/h3.1/image-to-3d"] {
            assert!(valid_endpoint(ok), "{ok}");
        }
        for bad in ["flux", "fal-ai/../keys", "fal-ai//x", "/fal-ai/x", "fal-ai/x/", "fal-ai/x?y=1", "fal-ai/x#y", "fal-ai/.x", "a/b c", "fal-ai/x%2f"] {
            assert!(!valid_endpoint(bad), "{bad}");
        }
    }

    #[test]
    fn every_sample_model_compiles_and_runs() {
        let s = sample();
        for m in s["models"].as_array().unwrap() {
            let spec = compile(m, true).expect("compiles");
            assert!(spec.runnable, "{} not runnable: {:?}", spec.id, spec.why_not);
            assert!(!spec.fields.is_empty(), "{}", spec.id);
            assert!(spec.thumb.is_none(), "test mode never loads fal thumbnails");
            assert!(spec.fields.iter().all(|f| f.name != "sync_mode"));
        }
    }

    #[test]
    fn flux_form_has_a_prompt_and_size_choice() {
        let spec = sample_spec("fal-ai/flux/schnell");
        let prompt = spec.fields.iter().find(|f| f.name == "prompt").expect("prompt");
        assert_eq!(prompt.kind, Kind::LongText);
        assert!(prompt.required && !prompt.advanced);
        let size = spec.fields.iter().find(|f| f.name == "image_size").expect("image_size");
        assert_eq!(size.kind, Kind::Choice);
        assert!(size.custom_json, "image_size also takes {{width, height}}");
        assert!(!size.advanced);
        assert_eq!(spec.outputs, vec!["image"]);
        let steps = spec.fields.iter().find(|f| f.name == "num_inference_steps").expect("steps");
        assert_eq!(steps.kind, Kind::Integer);
        assert!(steps.advanced && steps.min.is_some() && steps.max.is_some());
    }

    #[test]
    fn prompt_and_files_come_first() {
        let spec = sample_spec("fal-ai/trellis-2");
        assert_eq!(spec.fields[0].name, "image_url", "the picture a 3D model is made from leads the form");
        let spec = sample_spec("fal-ai/kling-video/v2.5-turbo/pro/image-to-video");
        let names: Vec<&str> = spec.fields.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(&names[..2], &["prompt", "image_url"]);
        let first_adv = spec.fields.iter().position(|f| f.advanced).unwrap();
        assert!(spec.fields[first_adv..].iter().all(|f| f.advanced), "More settings come last");
    }

    #[test]
    fn image_to_video_needs_its_picture() {
        let spec = sample_spec("fal-ai/kling-video/v2.5-turbo/pro/image-to-video");
        let img = spec.fields.iter().find(|f| f.kind == Kind::Media).expect("media field");
        assert_eq!(img.media.as_deref(), Some("image"));
        assert!(img.required);
        let mut input = Map::new();
        input.insert("prompt".into(), json!("a slow push in"));
        assert_eq!(check_input(&spec, &input).unwrap_err(), bad("field_required", &img.name));
        input.insert(img.name.clone(), json!("http://169.254.169.254/latest"));
        assert_eq!(check_input(&spec, &input).unwrap_err(), bad("invalid_value", &img.name));
        input.insert(img.name.clone(), json!("/outputs/../config/keys.env"));
        assert_eq!(check_input(&spec, &input).unwrap_err(), bad("invalid_value", &img.name));
        input.insert(img.name.clone(), json!("/outputs/abc123.png"));
        let (out, locals) = check_input(&spec, &input).expect("accepted");
        assert_eq!(locals, vec!["abc123.png"]);
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn input_checks_follow_the_schema() {
        let spec = sample_spec("fal-ai/flux/schnell");
        let mut input = Map::new();
        input.insert("prompt".into(), json!("a lamp"));
        assert!(check_input(&spec, &input).is_ok());
        let mut extra = input.clone();
        extra.insert("webhook".into(), json!("x"));
        assert_eq!(check_input(&spec, &extra).unwrap_err(), bad("unknown_field", "webhook"));
        let mut hidden = input.clone();
        hidden.insert("sync_mode".into(), json!(true));
        assert_eq!(check_input(&spec, &hidden).unwrap_err(), bad("unknown_field", "sync_mode"));
        let mut size = input.clone();
        size.insert("image_size".into(), json!("square_hd"));
        assert!(check_input(&spec, &size).is_ok());
        size.insert("image_size".into(), json!({"width": 512, "height": 512}));
        assert!(check_input(&spec, &size).is_ok());
        size.insert("image_size".into(), json!("huge"));
        assert_eq!(check_input(&spec, &size).unwrap_err(), bad("invalid_value", "image_size"));
        let steps = spec.fields.iter().find(|f| f.name == "num_inference_steps").unwrap();
        let mut over = input.clone();
        over.insert("num_inference_steps".into(), json!(steps.max.unwrap() + 1.0));
        assert_eq!(check_input(&spec, &over).unwrap_err(), bad("invalid_value", "num_inference_steps"));
        let mut frac = input.clone();
        frac.insert("num_inference_steps".into(), json!(2.5));
        assert_eq!(check_input(&spec, &frac).unwrap_err(), bad("invalid_value", "num_inference_steps"));
        let mut nulls = input.clone();
        nulls.insert("seed".into(), Value::Null);
        assert_eq!(check_input(&spec, &nulls).unwrap().0.len(), 1, "null means use the default");
        let mut wrong = Map::new();
        wrong.insert("prompt".into(), json!(5));
        assert_eq!(check_input(&spec, &wrong).unwrap_err(), bad("invalid_value", "prompt"));
    }

    #[test]
    fn result_files_are_found_and_kept_only_when_media() {
        let v = json!({"images": [{"url": "https://v3.fal.media/a.png", "content_type": "image/png"},
                                   {"url": "https://v3.fal.media/b", "content_type": "image/jpeg"}],
                       "video": {"url": "https://v3.fal.media/c.mp4"},
                       "model_mesh": {"url": "https://v3.fal.media/d.glb"},
                       "meta": {"url": "https://v3.fal.media/e.html", "content_type": "text/html"},
                       "timings": {"inference": 1.2}});
        let mut files = Vec::new();
        result_files(&v, 0, &mut files);
        assert_eq!(files.len(), 5);
        let kept: Vec<_> = files.iter().filter_map(|(u, c)| kind_and_ext(u, c.as_deref())).collect();
        assert_eq!(kept, vec![("image", "png".to_string()), ("image", "jpg".to_string()), ("model", "glb".to_string()), ("video", "mp4".to_string())]);
        assert!(kind_and_ext("https://x.test/a.svg", Some("image/svg+xml")).is_none(), "never an svg from the studio's origin");
    }

    #[test]
    fn fal_validation_errors_read_as_one_line() {
        let body = r#"{"detail":[{"loc":["body","image_url"],"msg":"Image too small","type":"value_error"}]}"#;
        assert_eq!(fal_detail(body), "image_url: Image too small");
        assert_eq!(fal_detail(r#"{"detail":"Not enough credits"}"#), "Not enough credits");
    }

    #[test]
    fn restored_jobs_find_their_adapter() {
        assert!(adapter_for_job("fal:fal-ai/flux/schnell").is_some());
        assert!(adapter_for_job("fal/flux-schnell").is_none());
        assert!(adapter_for_job("fal:../x").is_none());
    }

    #[test]
    fn queue_urls_stay_on_fal() {
        assert!(queue_url("https://queue.fal.run/fal-ai/flux/requests/1/status").is_ok());
        assert!(queue_url("https://evil.test/fal-ai/flux/requests/1").is_err());
        assert!(queue_url("http://queue.fal.run/x").is_err());
    }

    // ------------------------------------------------------------ routes (test mode: the sample, no network)

    use axum::body::{to_bytes, Body};
    use axum::http::Request;
    use tower::ServiceExt;

    fn test_state(name: &str, fake: bool) -> AppState {
        let mut st = AppState::for_tests(name);
        st.playground = Arc::new(Cache::new(true));
        if fake {
            st.fake = Some(Arc::new(crate::providers::Fake::new(Duration::from_millis(10), "")));
        }
        st
    }

    fn get_req(uri: &str) -> Request<Body> {
        Request::builder().uri(uri).header("host", "127.0.0.1:4170").body(Body::empty()).unwrap()
    }

    fn post_req(uri: &str, body: &Value) -> Request<Body> {
        Request::builder()
            .method("POST")
            .uri(uri)
            .header("host", "127.0.0.1:4170")
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    }

    async fn call(app: &axum::Router, req: Request<Body>) -> (StatusCode, Value) {
        let resp = app.clone().oneshot(req).await.expect("response");
        let status = resp.status();
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.expect("body");
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    #[tokio::test]
    async fn catalog_lists_searches_and_filters() {
        let app = crate::api::router(test_state("pg_catalog", false));
        let (s, v) = call(&app, get_req("/api/fal/models")).await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(v["models"].as_array().unwrap().len(), 7);
        assert!(v["categories"].as_array().unwrap().len() >= 10);
        let (_, v) = call(&app, get_req("/api/fal/models?category=image-to-3d")).await;
        assert_eq!(v["models"][0]["id"], "fal-ai/trellis-2");
        let (_, v) = call(&app, get_req("/api/fal/models?q=kling")).await;
        assert_eq!(v["models"].as_array().unwrap().len(), 2);
        let (s, _) = call(&app, get_req("/api/fal/models?category=training")).await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
        let (s, _) = call(&app, get_req("/api/fal/models?cursor=a%20b")).await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn model_form_and_price() {
        let app = crate::api::router(test_state("pg_model", false));
        let (s, v) = call(&app, get_req("/api/fal/model?id=fal-ai/flux/schnell")).await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(v["model"]["fields"][0]["name"], "prompt");
        assert_eq!(v["model"]["fields"][0]["kind"], "longtext");
        assert_eq!(v["model"]["runnable"], true);
        let (s, _) = call(&app, get_req("/api/fal/model?id=fal-ai/../keys")).await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
        let (s, _) = call(&app, get_req("/api/fal/model?id=fal-ai/nothing-here")).await;
        assert_eq!(s, StatusCode::NOT_FOUND);
        let (s, v) = call(&app, get_req("/api/fal/price?id=fal-ai/flux/schnell")).await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(v["price"]["unit"], "megapixels");
        assert_eq!(v["price"]["test"], true);
    }

    #[tokio::test]
    async fn runs_go_through_the_job_queue() {
        let st = test_state("pg_run", true);
        let app = crate::api::router(st.clone());
        let body = json!({"id": "fal-ai/flux/schnell", "input": {"prompt": "a lamp on a desk", "num_images": 1, "seed": null}});
        let (s, v) = call(&app, post_req("/api/fal/run", &body)).await;
        assert_eq!(s, StatusCode::ACCEPTED, "{v}");
        assert_eq!(v["job"]["model"], "fal:fal-ai/flux/schnell");
        assert_eq!(v["job"]["mode"], "text-to-image");
        assert_eq!(v["job"]["request"]["playground"]["input"]["prompt"], "a lamp on a desk");
        assert!(v["job"]["request"]["playground"]["input"].get("seed").is_none(), "nulls are dropped");
        let id = v["job"]["id"].as_str().unwrap().to_string();
        let mut done = false;
        for _ in 0..200 {
            let job = st.jobs.get(&id).expect("job");
            if job.status == "done" {
                assert_eq!(job.outputs.len(), 1);
                done = true;
                break;
            }
            assert_ne!(job.status, "failed", "{:?}", job.error);
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(done, "the fake run finished");
        let fake = st.fake.as_ref().unwrap();
        let seen = fake.seen.lock().unwrap();
        assert_eq!(seen.last().unwrap().endpoint, "fal-ai/flux/schnell");
    }

    #[tokio::test]
    async fn bad_runs_are_refused_with_the_field() {
        let app = crate::api::router(test_state("pg_bad", true));
        let (s, v) = call(&app, post_req("/api/fal/run", &json!({"id": "fal-ai/flux/schnell", "input": {"prompt": "x", "steps": 3}}))).await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
        assert_eq!(v, json!({"error": "unknown_field", "field": "steps"}));
        let (s, v) = call(&app, post_req("/api/fal/run", &json!({"id": "fal-ai/flux/schnell", "input": {}}))).await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
        assert_eq!(v["field"], "prompt");
        let (s, v) = call(&app, post_req("/api/fal/run", &json!({"id": "fal-ai/trellis-2", "input": {"image_url": "/outputs/missing.png"}}))).await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
        assert_eq!(v["error"], "file_missing");
    }

    #[tokio::test]
    async fn real_runs_need_the_fal_key() {
        let app = crate::api::router(test_state("pg_key", false));
        let (s, v) = call(&app, post_req("/api/fal/run", &json!({"id": "fal-ai/flux/schnell", "input": {"prompt": "x"}}))).await;
        assert_eq!(s, StatusCode::PRECONDITION_FAILED);
        assert_eq!(v["error"], "missing_key");
    }

    #[tokio::test]
    async fn local_files_over_the_limit_are_refused_before_the_job() {
        let st = test_state("pg_big", true);
        crate::fsutil::ensure_private_dir(&st.ctx.outputs_dir).unwrap();
        let f = std::fs::File::create(st.ctx.outputs_dir.join("big.png")).unwrap();
        f.set_len(MAX_INLINE_FILE + 1).unwrap();
        let app = crate::api::router(st);
        let (s, v) = call(&app, post_req("/api/fal/run", &json!({"id": "fal-ai/trellis-2", "input": {"image_url": "/outputs/big.png"}}))).await;
        assert_eq!(s, StatusCode::PAYLOAD_TOO_LARGE);
        assert_eq!(v["error"], "file_too_large");
    }
}
