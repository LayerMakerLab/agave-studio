//! Output storage (data/outputs, served at /outputs) and image input loading.

use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::time::Duration;

use base64::Engine as _;
use rand::RngCore;
use reqwest::header::CONTENT_TYPE;
use serde::Serialize;

use crate::providers::{PResult, ProviderError};

/// Shared HTTP client and the outputs directory.
pub struct Ctx {
    pub http: reqwest::Client,
    pub outputs_dir: PathBuf,
}

/// A finished file. `url` is always local (/outputs/...) because provider URLs expire.
#[derive(Serialize, serde::Deserialize, Clone, Debug)]
pub struct Output {
    pub url: String,
    pub kind: String,
}

pub fn random_hex(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    rand::thread_rng().fill_bytes(&mut buf);
    buf.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn net(e: reqwest::Error) -> ProviderError {
    ProviderError::Failed(format!("network error: {}", e.without_url()))
}

fn io(e: std::io::Error) -> ProviderError {
    ProviderError::Failed(format!("file error: {e}"))
}

/// Saves bytes under data/outputs with a random name and returns the local URL.
pub async fn save(ctx: &Ctx, bytes: &[u8], ext: &str, kind: &str) -> PResult<Output> {
    let name = format!("{}.{}", random_hex(12), ext);
    crate::fsutil::ensure_private_dir(&ctx.outputs_dir).map_err(io)?;
    tokio::fs::write(ctx.outputs_dir.join(&name), bytes).await.map_err(io)?;
    Ok(Output { url: format!("/outputs/{name}"), kind: kind.to_string() })
}

/// Media types a provider output may be saved as. Anything else (html, svg, xml...) falls back to the default,
/// so a provider response can never become a page served from the studio's own origin.
const MEDIA_EXTS: &[&str] = &[
    "png", "jpg", "jpeg", "webp", "gif", "avif", "heic", "mp4", "m4v", "webm", "mov", "mp3", "wav", "m4a", "aac", "ogg",
    "opus", "flac", "glb", "usdz", "stl", "3mf", "obj", "fbx",
];

/// File extension from a URL path when it is a known media type, else the default.
pub fn ext_from_url(url: &str, default: &str) -> String {
    let path = url.split(['?', '#']).next().unwrap_or("");
    path.rsplit('/')
        .next()
        .and_then(|seg| seg.rsplit_once('.'))
        .map(|(_, e)| e.to_ascii_lowercase())
        .filter(|e| MEDIA_EXTS.contains(&e.as_str()))
        .unwrap_or_else(|| default.to_string())
}

/// Largest provider JSON body read into memory (inline base64 images fit well under this).
pub const MAX_JSON_BYTES: usize = 64 << 20;
/// Largest output file downloaded from a provider.
pub const MAX_DOWNLOAD_BYTES: usize = 512 << 20;
/// Largest user-supplied input image fetched by the gateway.
const MAX_INPUT_IMAGE_BYTES: usize = 20 << 20;
const MAX_REDIRECTS: usize = 5;

/// Reads a response body, failing once it passes `limit` bytes.
pub async fn read_capped(mut resp: reqwest::Response, limit: usize) -> PResult<Vec<u8>> {
    if resp.content_length().is_some_and(|n| n > limit as u64) {
        return Err(ProviderError::Failed("response too large".into()));
    }
    let mut buf = Vec::new();
    while let Some(chunk) = resp.chunk().await.map_err(net)? {
        if buf.len() + chunk.len() > limit {
            return Err(ProviderError::Failed("response too large".into()));
        }
        buf.extend_from_slice(&chunk);
    }
    Ok(buf)
}

/// Parses an https URL with a host and no credentials. Anything else is None.
pub fn parse_https_url(s: &str) -> Option<reqwest::Url> {
    let u = reqwest::Url::parse(s).ok()?;
    let ok = u.scheme() == "https" && u.host().is_some() && u.username().is_empty() && u.password().is_none();
    ok.then_some(u)
}

/// A file name the gateway itself writes under data/outputs.
pub fn valid_output_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() < 200
        && !name.starts_with('.')
        && !name.contains("..")
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_')
}

fn same_origin(a: &reqwest::Url, b: &reqwest::Url) -> bool {
    a.scheme() == b.scheme() && a.host_str() == b.host_str() && a.port_or_known_default() == b.port_or_known_default()
}

/// GET an https URL and return its bytes and content type. Extra headers (provider keys) are sent
/// only to the original origin: redirects are followed by hand so a key never reaches another host.
/// `ctx.http` must not follow redirects itself.
pub async fn fetch(http: &reqwest::Client, url: &str, headers: &[(&str, &str)]) -> PResult<(Vec<u8>, Option<String>)> {
    let first = parse_https_url(url).ok_or_else(|| ProviderError::Failed("download URL must be https".into()))?;
    let mut current = first.clone();
    for _ in 0..=MAX_REDIRECTS {
        let mut req = http.get(current.clone());
        if same_origin(&current, &first) {
            for (k, v) in headers {
                req = req.header(*k, *v);
            }
        }
        if !host_is_public(&current) {
            return Err(ProviderError::Failed("download host is not public".into()));
        }
        let resp = req.send().await.map_err(net)?;
        let status = resp.status();
        if status.is_redirection() {
            let next = resp
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|v| v.to_str().ok())
                .and_then(|loc| current.join(loc).ok())
                .and_then(|u| parse_https_url(u.as_str()))
                .ok_or_else(|| ProviderError::Failed("download redirect refused".into()))?;
            current = next;
            continue;
        }
        if !status.is_success() {
            return Err(ProviderError::Failed(format!("download HTTP {}", status.as_u16())));
        }
        let ct = resp
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let bytes = read_capped(resp, MAX_DOWNLOAD_BYTES).await?;
        return Ok((bytes, ct));
    }
    Err(ProviderError::Failed("too many download redirects".into()))
}

/// Refuses provider download URLs that name the local network outright: a private or loopback IP, or a
/// local-only name. Names are not resolved here, so fake-IP proxies, NAT64 and proxy-only networks still work.
fn host_is_public(url: &reqwest::Url) -> bool {
    let host = url.host_str().unwrap_or("").to_ascii_lowercase();
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    if let Ok(ip) = bare.parse::<IpAddr>() {
        return is_public_ip(ip);
    }
    let bare = bare.trim_end_matches('.');
    !(bare == "localhost" || bare.ends_with(".localhost") || bare.ends_with(".local") || bare.ends_with(".internal") || !bare.contains('.'))
}

/// True for addresses on the public internet. Loopback, private, link-local, CGNAT, multicast,
/// documentation and reserved ranges are refused so a user URL cannot reach the local network.
pub fn is_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            !(v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || v4.is_multicast()
                || v4.is_documentation()
                || o[0] == 0
                || (o[0] == 100 && (o[1] & 0xc0) == 64)
                || (o[0] == 198 && (o[1] & 0xfe) == 18)
                || (o[0] == 192 && o[1] == 0 && o[2] == 0)
                || o[0] >= 240)
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_public_ip(IpAddr::V4(v4));
            }
            let seg = v6.segments();
            // IPv4-compatible (::a.b.c.d) carries an IPv4 address too.
            if seg[..6].iter().all(|&s| s == 0) && !v6.is_loopback() && !v6.is_unspecified() {
                let o = v6.octets();
                return is_public_ip(IpAddr::V4(std::net::Ipv4Addr::new(o[12], o[13], o[14], o[15])));
            }
            !(v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                || (seg[0] & 0xfe00) == 0xfc00
                || (seg[0] & 0xffc0) == 0xfe80
                || (seg[0] == 0x2001 && seg[1] == 0x0db8)
                || (seg[0] == 0x64 && seg[1] == 0xff9b)
                || seg[0] == 0x2002
                || (seg[0] == 0x2001 && seg[1] == 0)
                || (seg[0] & 0xffc0) == 0xfec0)
        }
    }
}

/// Fetches a user-supplied https image. The host must resolve only to public addresses, the
/// connection is pinned to the checked address (no DNS rebinding), and redirects are refused.
async fn fetch_user_image(src: &str) -> PResult<(Vec<u8>, Option<String>)> {
    let url = parse_https_url(src).ok_or_else(|| ProviderError::Failed("image URL must be https".into()))?;
    let port = url.port_or_known_default().unwrap_or(443);
    let mut builder = reqwest::Client::builder()
        .user_agent("agave-studio/0.1")
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(60));
    let host = url.host_str().unwrap_or("");
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    if let Ok(ip) = bare.parse::<IpAddr>() {
        if !is_public_ip(ip) {
            return Err(ProviderError::Failed("image URL host is not public".into()));
        }
    } else {
        let addrs: Vec<SocketAddr> = tokio::net::lookup_host((bare, port))
            .await
            .map_err(|_| ProviderError::Failed("image URL host did not resolve".into()))?
            .collect();
        if addrs.is_empty() || !addrs.iter().all(|a| is_public_ip(a.ip())) {
            return Err(ProviderError::Failed("image URL host is not public".into()));
        }
        builder = builder.resolve(bare, addrs[0]);
    }
    let client = builder.build().map_err(|_| ProviderError::Failed("http client error".into()))?;
    let resp = client.get(url).send().await.map_err(net)?;
    let status = resp.status();
    if !status.is_success() {
        return Err(ProviderError::Failed(format!("image download HTTP {}", status.as_u16())));
    }
    let ct = resp
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let bytes = read_capped(resp, MAX_INPUT_IMAGE_BYTES).await?;
    Ok((bytes, ct))
}

fn mime_from_ext(path: &str) -> &'static str {
    match path.rsplit('.').next().map(|e| e.to_ascii_lowercase()).as_deref() {
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("webp") => "image/webp",
        _ => "image/png",
    }
}

/// Loads an image for a provider that needs bytes. Accepts data: URIs, /outputs/<file>, and https URLs.
pub async fn load_image(ctx: &Ctx, src: &str) -> PResult<(Vec<u8>, String)> {
    if let Some(rest) = src.strip_prefix("data:") {
        let (meta, data) = rest
            .split_once(',')
            .ok_or_else(|| ProviderError::Failed("bad data URI".into()))?;
        if !meta.ends_with(";base64") {
            return Err(ProviderError::Failed("data URI must be base64".into()));
        }
        let mime = meta.split(';').next().unwrap_or("image/png").to_string();
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(data)
            .map_err(|_| ProviderError::Failed("bad base64 in data URI".into()))?;
        return Ok((bytes, mime));
    }
    if let Some(name) = src.strip_prefix("/outputs/") {
        if !valid_output_name(name) {
            return Err(ProviderError::Failed("bad local image path".into()));
        }
        let bytes = tokio::fs::read(ctx.outputs_dir.join(name))
            .await
            .map_err(|_| ProviderError::Failed("local image not found".into()))?;
        return Ok((bytes, mime_from_ext(name).to_string()));
    }
    let (bytes, ct) = fetch_user_image(src).await?;
    let mime = ct
        .filter(|c| c.starts_with("image/"))
        .unwrap_or_else(|| mime_from_ext(src).to_string());
    Ok((bytes, mime))
}

/// Same as load_image, but returns a data: URI string for providers that take inline images.
pub async fn as_data_uri(ctx: &Ctx, src: &str) -> PResult<String> {
    if src.starts_with("data:") {
        return Ok(src.to_string());
    }
    let (bytes, mime) = load_image(ctx, src).await?;
    Ok(format!(
        "data:{};base64,{}",
        mime,
        base64::engine::general_purpose::STANDARD.encode(bytes)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_downloads_refuse_local_hosts_without_resolving_names() {
        let ok = |u: &str| host_is_public(&reqwest::Url::parse(u).unwrap());
        assert!(ok("https://v3.fal.media/files/a.mp4"));
        assert!(ok("https://cdn.example.com/x.png"));
        for u in ["https://127.0.0.1/a", "https://192.168.1.4/a", "https://[::1]/a", "https://localhost/a",
                  "https://printer.local/a", "https://intranet/a", "https://metadata.google.internal/a"] {
            assert!(!ok(u), "{u}");
        }
    }

    #[test]
    fn ext_from_url_handles_query_and_default() {
        assert_eq!(ext_from_url("https://x.test/a/b.MP4?sig=1", "bin"), "mp4");
        assert_eq!(ext_from_url("https://x.test/a/b", "png"), "png");
        assert_eq!(ext_from_url("https://x.test/a/b.averylongext", "png"), "png");
        assert_eq!(ext_from_url("https://x.test/a/page.html", "png"), "png");
        assert_eq!(ext_from_url("https://x.test/a/logo.SVG", "png"), "png");
    }

    #[test]
    fn only_https_urls_parse() {
        assert!(parse_https_url("https://cdn.example.com/a.png").is_some());
        assert!(parse_https_url("http://cdn.example.com/a.png").is_none());
        assert!(parse_https_url("file:///etc/passwd").is_none());
        assert!(parse_https_url("https://user:pw@example.com/a.png").is_none());
        assert!(parse_https_url("https://").is_none());
    }

    #[test]
    fn private_and_local_addresses_are_not_public() {
        for ip in ["127.0.0.1", "10.0.0.1", "172.16.0.1", "192.168.1.1", "169.254.169.254", "100.64.0.1",
                   "0.0.0.0", "::1", "fd00::1", "fe80::1", "::ffff:127.0.0.1", "224.0.0.1", "::7f00:1", "2002:7f00:1::1",
                   "2001:0:4136:e378::1", "fec0::1", "192.0.0.8"] {
            assert!(!is_public_ip(ip.parse().unwrap()), "{ip}");
        }
        assert!(is_public_ip("1.1.1.1".parse().unwrap()));
        assert!(is_public_ip("2606:4700::1111".parse().unwrap()));
    }

    #[tokio::test]
    async fn user_image_fetch_refuses_local_hosts() {
        for src in ["https://127.0.0.1/a.png", "https://[::1]/a.png", "https://169.254.169.254/latest", "https://localhost/a.png"] {
            assert!(fetch_user_image(src).await.is_err(), "{src}");
        }
    }

    #[test]
    fn output_names_are_strict() {
        assert!(valid_output_name("0a1b2c.png"));
        assert!(!valid_output_name("../config/keys.env"));
        assert!(!valid_output_name(".hidden"));
        assert!(!valid_output_name("a/b.png"));
        assert!(!valid_output_name(""));
    }
}
