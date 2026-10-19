//! Built-in editing (Agave Edit): renders a timeline of finished clips and stills into one MP4 with
//! ffmpeg. Each clip can be trimmed, stills last a set time, every clip can carry a caption overlay
//! (a transparent PNG drawn by the browser at the output size), the frame can be set to a platform
//! shape (fit with bars or fill by cropping), and a replacement soundtrack is optional.
//! Runs as an ordinary job so it shows up in the gallery like any generation.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use async_trait::async_trait;
use tokio::process::Command;

use crate::media::{self, Ctx, Output};
use crate::reframe;
use crate::providers::{PResult, Provider, ProviderError, SubmitReq, Submitted};

pub const MAX_CLIPS: usize = 200;
/// Longest piece of one clip, and the most a still can last, in seconds.
pub const MAX_CLIP_SECONDS: f64 = 600.0;
pub const MAX_STILL_SECONDS: f64 = 60.0;
pub const DEFAULT_STILL_SECONDS: f64 = 3.0;
/// Shortest piece the editor will render.
pub const MIN_CLIP_SECONDS: f64 = 0.1;
const STEP_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// One piece of the timeline: a file under data/outputs (already checked) and the part of it to use.
#[derive(Clone, Debug, PartialEq)]
pub struct Clip {
    pub name: String,
    /// A picture shown for `to - from` seconds instead of a video.
    pub still: bool,
    /// Start inside the source, in seconds.
    pub from: f64,
    /// End inside the source; None means the end of the video (or the default still length).
    pub to: Option<f64>,
}

impl Clip {
    #[cfg(test)]
    pub fn whole(name: &str) -> Self {
        Clip { name: name.to_string(), still: false, from: 0.0, to: None }
    }
}

/// Output shapes offered by the editor, as (label, width, height).
pub const FRAMES: [(&str, u32, u32); 4] = [("16:9", 1920, 1080), ("9:16", 1080, 1920), ("1:1", 1080, 1080), ("4:5", 1080, 1350)];

pub fn frame_size(label: &str) -> Option<(u32, u32)> {
    FRAMES.iter().find(|f| f.0 == label).map(|f| (f.1, f.2))
}

#[derive(Clone, Debug)]
pub struct Plan {
    pub clips: Vec<Clip>,
    /// Caption PNG per clip (same index), or None.
    pub captions: Vec<Option<String>>,
    /// Replacement soundtrack file name, or None.
    pub audio: Option<String>,
    /// Keep the clips' own sound (ignored when `audio` is set).
    pub keep_clip_audio: bool,
    /// Output size; None keeps the first clip's shape (capped at 1920 on the long side).
    pub frame: Option<(u32, u32)>,
    /// Fill the frame by cropping instead of fitting with bars.
    pub fill: bool,
    /// Timed caption images over the finished edit (auto captions), sorted and not overlapping.
    pub overlays: Vec<Overlay>,
    /// With `fill`: the crop follows the subject (see reframe.rs) instead of taking the middle.
    pub follow: bool,
    /// Take steady background noise out of the clips' own sound.
    pub clean_voice: bool,
    /// Bring the finished sound to LOUDNESS_TARGET.
    pub loudness: bool,
}

/// Integrated loudness the export is brought to (LUFS), with a true-peak ceiling (dBTP).
pub const LOUDNESS_TARGET: f64 = -14.0;
const TRUE_PEAK: f64 = -1.5;
/// Voice cleanup: cut rumble below 80 Hz, then FFT denoising with a -35 dB noise floor. Measured on tone
/// over white hiss: hiss down 12-24 dB, tone within 1.6 dB. (anlmdn crashed about half the time on ffmpeg
/// 9.0.x, and afftdn's own noise tracking barely moved the hiss.)
const CLEAN_VOICE: &str = "highpass=f=80,afftdn=nf=-35:nr=24,";

/// A path quoted for an ffconcat list: single quotes, with any ' inside written as '\''.
fn concat_path(p: &Path) -> String {
    format!("'{}'", p.display().to_string().replace('\'', "'\\''"))
}

/// Measured values from loudnorm's first pass, for the exact second pass.
fn loudnorm_second_pass(log: &str) -> Option<String> {
    let (open, close) = (log.rfind('{')?, log.rfind('}')?);
    let json = log.get(open..=close)?;
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let get = |k: &str| v.get(k)?.as_str()?.trim().parse::<f64>().ok().filter(|x| x.is_finite());
    let (i, tp, lra, thresh, offset) =
        (get("input_i")?, get("input_tp")?, get("input_lra")?, get("input_thresh")?, get("target_offset")?);
    // Silence (or nearly) has nothing to bring up.
    if i < -70.0 {
        return None;
    }
    Some(format!(
        "loudnorm=I={LOUDNESS_TARGET}:TP={TRUE_PEAK}:LRA=11:measured_I={i}:measured_TP={tp}:measured_LRA={lra}:measured_thresh={thresh}:offset={offset}:linear=true,aresample=48000"
    ))
}

/// Like `run`, but returns what ffmpeg logged (filters such as loudnorm report there).
async fn run_log(cmd: &mut Command) -> PResult<String> {
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped()).kill_on_drop(true);
    let child = cmd.spawn().map_err(|e| ProviderError::Failed(format!("ffmpeg not available: {e}")))?;
    let out = tokio::time::timeout(STEP_TIMEOUT, child.wait_with_output())
        .await
        .map_err(|_| ProviderError::Failed("editing took too long".into()))?
        .map_err(|e| ProviderError::Failed(format!("ffmpeg failed: {e}")))?;
    if !out.status.success() {
        return Err(ProviderError::Failed("ffmpeg could not measure the sound".into()));
    }
    Ok(String::from_utf8_lossy(&out.stderr).to_string())
}

/// The scale-and-frame filter for one clip of size (sw, sh). `pan` is a reframe expression for
/// Follow, or None for a centred crop.
fn frame_filter(w: u32, h: u32, fill: bool, sw: u32, sh: u32, pan: Option<&str>) -> String {
    let tail = "setsar=1,fps=30,format=yuv420p";
    if !fill {
        return format!("scale={w}:{h}:force_original_aspect_ratio=decrease,pad={w}:{h}:(ow-iw)/2:(oh-ih)/2,{tail}");
    }
    let crop = match (pan, reframe::axis(f64::from(sw) / f64::from(sh.max(1)), f64::from(w) / f64::from(h))) {
        (Some(e), reframe::Axis::X) => format!("crop={w}:{h}:x='(iw-ow)*({e})':y='(ih-oh)/2'"),
        (Some(e), reframe::Axis::Y) => format!("crop={w}:{h}:x='(iw-ow)/2':y='(ih-oh)*({e})'"),
        _ => format!("crop={w}:{h}"),
    };
    format!("scale={w}:{h}:force_original_aspect_ratio=increase,{crop},{tail}")
}

/// A caption PNG (drawn by the browser at the output size) shown from `start` to `end` on the edit.
#[derive(Clone, Debug, PartialEq)]
pub struct Overlay {
    pub png: String,
    pub start: f64,
    pub end: f64,
}

pub const MAX_OVERLAYS: usize = 3000;

/// True when the overlays run forwards, start at or after 0 and do not overlap.
pub fn overlays_ok(list: &[Overlay]) -> bool {
    let mut at = 0.0;
    for o in list {
        if !o.start.is_finite() || !o.end.is_finite() || o.start < at - 1e-6 || o.end <= o.start || o.end > 3600.0 * 4.0 {
            return false;
        }
        at = o.end;
    }
    true
}

/// An ffconcat list that shows each caption for its time and a clear image in the gaps.
fn overlay_list(dir: &Path, blank: &Path, list: &[Overlay]) -> String {
    let mut out = String::from("ffconcat version 1.0\n");
    let mut at = 0.0;
    for o in list {
        if o.start > at + 0.001 {
            out.push_str(&format!("file {}\nduration {:.3}\n", concat_path(blank), o.start - at));
        }
        out.push_str(&format!("file {}\nduration {:.3}\n", concat_path(&dir.join(&o.png)), o.end - o.start));
        at = o.end;
    }
    // The last entry is listed twice so its duration is honoured.
    out.push_str(&format!("file {}\nduration 1.000\nfile {}\n", concat_path(blank), concat_path(blank)));
    out
}

pub struct Assemble(pub Plan);

const VIDEO_EXT: [&str; 3] = ["mp4", "mov", "webm"];
const AUDIO_EXT: [&str; 6] = ["mp3", "wav", "m4a", "aac", "mp4", "mov"];

fn ext(name: &str) -> String {
    name.rsplit('.').next().unwrap_or("").to_ascii_lowercase()
}

/// Checks an /outputs/<name> URL and returns the bare name if it exists and has an allowed extension.
pub fn local_file(dir: &Path, url: &str, allowed: &[&str]) -> Option<String> {
    let name = url.strip_prefix("/outputs/")?;
    (media::valid_output_name(name) && allowed.contains(&ext(name).as_str()) && dir.join(name).is_file())
        .then(|| name.to_string())
}

pub fn video_ok(dir: &Path, url: &str) -> Option<String> {
    local_file(dir, url, &VIDEO_EXT)
}

pub fn audio_ok(dir: &Path, url: &str) -> Option<String> {
    local_file(dir, url, &AUDIO_EXT)
}

pub fn caption_ok(dir: &Path, url: &str) -> Option<String> {
    local_file(dir, url, &["png"])
}

pub fn still_ok(dir: &Path, url: &str) -> Option<String> {
    local_file(dir, url, &["png", "jpg", "jpeg", "webp"])
}

/// Where a clip starts and how long it runs, from its trim and the source length (None for a still).
/// Err when the trim leaves nothing.
pub fn span(clip: &Clip, source_len: Option<f64>) -> Result<(f64, f64), &'static str> {
    let from = clip.from.max(0.0);
    let (end, cap) = match source_len {
        Some(len) => (clip.to.unwrap_or(len).min(len), MAX_CLIP_SECONDS),
        None => (clip.to.unwrap_or(from + DEFAULT_STILL_SECONDS), MAX_STILL_SECONDS),
    };
    let len = (end - from).min(cap);
    if !len.is_finite() || len < MIN_CLIP_SECONDS {
        return Err("a clip is trimmed to nothing");
    }
    Ok(if source_len.is_some() { (from, len) } else { (0.0, len) })
}

/// How far Dolly in moves on a model that takes an end picture: the end picture is the middle 65% of the start
/// picture, scaled back to its size (about a 1.5x push).
const PUSH_IN_CROP: f64 = 0.65;

/// The end picture for Dolly in: the start picture cropped in around its middle, as a JPEG data URI.
pub(crate) async fn push_in_frame(ctx: &Ctx, start: &str) -> PResult<String> {
    let (bytes, mime) = media::load_image(ctx, start).await?;
    let ext = match mime.as_str() {
        "image/png" => "png",
        "image/webp" => "webp",
        _ => "jpg",
    };
    let dir = std::env::temp_dir();
    let tag = media::random_hex(8);
    let (src, dst) = (dir.join(format!("agave-push-{tag}.{ext}")), dir.join(format!("agave-push-{tag}-end.jpg")));
    tokio::fs::write(&src, &bytes).await.map_err(|e| ProviderError::Failed(format!("could not stage the start picture: {e}")))?;
    let c = PUSH_IN_CROP;
    let vf = format!("crop=trunc(iw*{c}/2)*2:trunc(ih*{c}/2)*2,scale=trunc(iw/{c}/2)*2:trunc(ih/{c}/2)*2:flags=lanczos");
    let made = run(Command::new("ffmpeg").args(["-y", "-v", "error", "-i"]).arg(&src).args(["-vf", &vf, "-frames:v", "1", "-q:v", "2"]).arg(&dst)).await;
    let end = match made {
        Ok(_) => tokio::fs::read(&dst).await.map_err(|e| ProviderError::Failed(format!("could not read the end picture: {e}"))),
        Err(e) => Err(e),
    };
    let _ = tokio::fs::remove_file(&src).await;
    let _ = tokio::fs::remove_file(&dst).await;
    let end = end?;
    Ok(format!("data:image/jpeg;base64,{}", base64::Engine::encode(&base64::engine::general_purpose::STANDARD, end)))
}

pub(crate) async fn run(cmd: &mut Command) -> PResult<Vec<u8>> {
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    let child = cmd.spawn().map_err(|e| ProviderError::Failed(format!("ffmpeg not available: {e}")))?;
    let out = tokio::time::timeout(STEP_TIMEOUT, child.wait_with_output())
        .await
        .map_err(|_| ProviderError::Failed("editing took too long".into()))?
        .map_err(|e| ProviderError::Failed(format!("ffmpeg failed: {e}")))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let last = err.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("").chars().take(200).collect::<String>();
        return Err(ProviderError::Failed(format!("ffmpeg: {last}")));
    }
    Ok(out.stdout)
}

/// Width, height, duration in seconds, and whether the file has sound.
#[cfg(test)]
async fn probe(path: &Path) -> PResult<(u32, u32, f64, bool)> {
    let (w, h, dur, audio) = probe_any(path).await?;
    Ok((w, h, dur.ok_or_else(|| ProviderError::Failed("a clip has no length".into()))?, audio))
}

/// Picture size of a local file (for the reframe preview).
pub async fn video_size(path: &Path) -> Option<(u32, u32)> {
    probe_any(path).await.ok().map(|(w, h, _, _)| (w, h))
}

/// Like `probe`, but a picture (no length) is fine.
async fn probe_any(path: &Path) -> PResult<(u32, u32, Option<f64>, bool)> {
    let out = run(Command::new("ffprobe")
        .args(["-v", "error", "-show_entries", "stream=codec_type,width,height:format=duration", "-of", "json"])
        .arg(path))
    .await?;
    let v: serde_json::Value =
        serde_json::from_slice(&out).map_err(|_| ProviderError::Failed("could not read a clip".into()))?;
    let streams = v.get("streams").and_then(|s| s.as_array()).cloned().unwrap_or_default();
    let video = streams.iter().find(|s| s.get("codec_type").and_then(|t| t.as_str()) == Some("video"));
    let has_audio = streams.iter().any(|s| s.get("codec_type").and_then(|t| t.as_str()) == Some("audio"));
    let (w, h) = video
        .and_then(|s| Some((s.get("width")?.as_u64()? as u32, s.get("height")?.as_u64()? as u32)))
        .ok_or_else(|| ProviderError::Failed("a clip has no picture".into()))?;
    let dur = v
        .pointer("/format/duration")
        .and_then(|d| d.as_str())
        .and_then(|d| d.parse::<f64>().ok())
        .filter(|d| d.is_finite() && *d > 0.0);
    Ok((w, h, dur, has_audio))
}

/// Even size no larger than 1920 on the long side, keeping the first clip's shape.
fn target_size(w: u32, h: u32) -> (u32, u32) {
    let scale = (1920.0 / f64::from(w.max(h))).min(1.0);
    let even = |x: f64| ((x.round() as u32) / 2 * 2).max(2);
    (even(f64::from(w) * scale), even(f64::from(h) * scale))
}

impl Assemble {
    async fn build(&self, ctx: &Ctx, work: &Path) -> PResult<Output> {
        let dir = &ctx.outputs_dir;
        let p = &self.0;
        let (w, h) = match p.frame {
            Some(size) => size,
            None => {
                let (w0, h0, _, _) = probe_any(&dir.join(&p.clips[0].name)).await?;
                target_size(w0, h0)
            }
        };
        let tracker = reframe::Tracker::new(dir);
        let mut list = String::new();
        for (i, clip) in p.clips.iter().enumerate() {
            let src = dir.join(&clip.name);
            let (sw, sh, source_len, has_audio) = probe_any(&src).await?;
            let source_len = if clip.still { None } else {
                Some(source_len.ok_or_else(|| ProviderError::Failed("a clip has no length".into()))?)
            };
            let (from, dur) = span(clip, source_len).map_err(|e| ProviderError::Failed(e.into()))?;
            // Follow: aim the crop at the subject; without the vision tool it stays centred.
            let pan = if p.fill && p.follow && !clip.still {
                match tracker.track(dir, &clip.name).await {
                    Some(track) => {
                        let keys = reframe::path(&track, f64::from(sw) / f64::from(sh.max(1)), f64::from(w) / f64::from(h));
                        (!keys.is_empty()).then(|| reframe::pan_expr(&keys, from))
                    }
                    None => None,
                }
            } else {
                None
            };
            let fit = frame_filter(w, h, p.fill, sw, sh, pan.as_deref());
            let part = work.join(format!("part{i:02}.mp4"));
            let mut cmd = Command::new("ffmpeg");
            cmd.args(["-y", "-v", "error"]);
            if clip.still {
                cmd.args(["-loop", "1", "-framerate", "30", "-t", &format!("{dur:.3}"), "-i"]).arg(&src);
            } else {
                cmd.args(["-ss", &format!("{from:.3}"), "-t", &format!("{dur:.3}"), "-i"]).arg(&src);
            }
            let mut next = 1;
            let audio_in = if !clip.still && has_audio && p.keep_clip_audio && p.audio.is_none() {
                format!("[0:a]{}", if p.clean_voice { CLEAN_VOICE } else { "" })
            } else {
                cmd.args(["-f", "lavfi", "-t", &format!("{dur:.3}"), "-i", "anullsrc=r=48000:cl=stereo"]);
                next += 1;
                format!("[{}:a]", next - 1)
            };
            let video = match p.captions.get(i).cloned().flatten() {
                Some(png) => {
                    cmd.arg("-i").arg(dir.join(png));
                    // The caption is drawn at the output shape and goes on after the clip is framed.
                    format!("[0:v]{fit}[b];[{next}:v]scale={w}:{h},format=rgba[cap];[b][cap]overlay=0:0:format=auto,format=yuv420p[v]")
                }
                None => format!("[0:v]{fit}[v]"),
            };
            let graph = format!("{video};{audio_in}aresample=48000,aformat=channel_layouts=stereo[a]");
            cmd.args(["-filter_complex", &graph, "-map", "[v]", "-map", "[a]", "-t", &format!("{dur:.3}")])
                .args(["-c:v", "libx264", "-profile:v", "high", "-preset", "veryfast", "-crf", "20", "-c:a", "aac", "-b:a", "192k"])
                .arg(&part);
            run(&mut cmd).await?;
            list.push_str(&format!("file {}\n", concat_path(&part)));
        }
        let list_file = work.join("list.txt");
        tokio::fs::write(&list_file, list).await.map_err(|e| ProviderError::Failed(format!("file error: {e}")))?;
        let joined = work.join("joined.mp4");
        run(Command::new("ffmpeg")
            .args(["-y", "-v", "error", "-f", "concat", "-safe", "0", "-i"])
            .arg(&list_file)
            .args(["-c", "copy", "-movflags", "+faststart"])
            .arg(&joined))
        .await?;

        let joined = if p.overlays.is_empty() {
            joined
        } else {
            let blank = work.join("blank.png");
            run(Command::new("ffmpeg")
                .args(["-y", "-v", "error", "-f", "lavfi", "-i", &format!("color=c=black@0.0:s={w}x{h},format=rgba"), "-frames:v", "1"])
                .arg(&blank))
            .await?;
            let list = work.join("captions.txt");
            tokio::fs::write(&list, overlay_list(dir, &blank, &p.overlays))
                .await
                .map_err(|e| ProviderError::Failed(format!("file error: {e}")))?;
            let captioned = work.join("captioned.mp4");
            let graph = format!("[1:v]scale={w}:{h},format=rgba[c];[0:v][c]overlay=0:0:eof_action=pass:format=auto,format=yuv420p[v]");
            run(Command::new("ffmpeg")
                .args(["-y", "-v", "error", "-i"])
                .arg(&joined)
                .args(["-f", "concat", "-safe", "0", "-i"])
                .arg(&list)
                .args(["-filter_complex", &graph, "-map", "[v]", "-map", "0:a?"])
                .args(["-c:v", "libx264", "-profile:v", "high", "-preset", "veryfast", "-crf", "20", "-c:a", "copy"])
                .arg(&captioned))
            .await?;
            captioned
        };

        let name = format!("{}.mp4", media::random_hex(12));
        let dest = dir.join(&name);
        let mixed = work.join("mixed.mp4");
        let has_sound = p.audio.is_some() || p.keep_clip_audio;
        match &p.audio {
            Some(track) => {
                run(Command::new("ffmpeg")
                    .args(["-y", "-v", "error", "-i"])
                    .arg(&joined)
                    .arg("-i")
                    .arg(dir.join(track))
                    // A short song is padded with silence so the picture is never cut; a long one stops with it.
                    .args(["-filter_complex", "[1:a]apad[a]", "-map", "0:v", "-map", "[a]", "-shortest"])
                    .args(["-c:v", "copy", "-c:a", "aac", "-b:a", "192k"])
                    .arg(&mixed))
                .await?;
            }
            None if !p.keep_clip_audio => {
                run(Command::new("ffmpeg").args(["-y", "-v", "error", "-i"]).arg(&joined).args(["-c", "copy", "-an"]).arg(&mixed))
                    .await?;
            }
            None => {
                tokio::fs::rename(&joined, &mixed).await.map_err(|e| ProviderError::Failed(format!("file error: {e}")))?;
            }
        }
        // Loudness: measure the finished sound, then apply the exact correction (two-pass loudnorm).
        let second = if p.loudness && has_sound {
            let log = run_log(Command::new("ffmpeg")
                .args(["-hide_banner", "-nostats", "-v", "info", "-i"])
                .arg(&mixed)
                .args(["-vn", "-af", &format!("loudnorm=I={LOUDNESS_TARGET}:TP={TRUE_PEAK}:LRA=11:print_format=json"), "-f", "null", "-"]))
            .await?;
            loudnorm_second_pass(&log)
        } else {
            None
        };
        match second {
            Some(af) => {
                run(Command::new("ffmpeg")
                    .args(["-y", "-v", "error", "-i"])
                    .arg(&mixed)
                    .args(["-af", &af, "-c:v", "copy", "-c:a", "aac", "-b:a", "192k", "-movflags", "+faststart"])
                    .arg(&dest))
                .await?;
            }
            None => {
                run(Command::new("ffmpeg").args(["-y", "-v", "error", "-i"]).arg(&mixed).args(["-c", "copy", "-movflags", "+faststart"]).arg(&dest))
                    .await?;
            }
        }
        Ok(Output { url: format!("/outputs/{name}"), kind: "video".into() })
    }
}

#[async_trait]
impl Provider for Assemble {
    async fn submit(&self, ctx: &Ctx, _key: &str, _req: &SubmitReq) -> PResult<Submitted> {
        if self.0.clips.is_empty() {
            return Err(ProviderError::Failed("no clips".into()));
        }
        let work: PathBuf = ctx.outputs_dir.join(format!(".edit-{}", media::random_hex(6)));
        tokio::fs::create_dir_all(&work).await.map_err(|e| ProviderError::Failed(format!("file error: {e}")))?;
        let result = self.build(ctx, &work).await;
        let _ = tokio::fs::remove_dir_all(&work).await;
        result.map(|o| Submitted::Done(vec![o]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_size_is_even_and_capped() {
        assert_eq!(target_size(1280, 720), (1280, 720));
        assert_eq!(target_size(3840, 2160), (1920, 1080));
        assert_eq!(target_size(1080, 1920), (1080, 1920));
        assert_eq!(target_size(721, 405), (720, 404));
    }

    #[test]
    fn only_existing_local_files_with_the_right_type_pass() {
        let dir = crate::fsutil::test_dir("assemble_files");
        std::fs::write(dir.join("a.mp4"), b"x").unwrap();
        std::fs::write(dir.join("c.png"), b"x").unwrap();
        assert_eq!(video_ok(&dir, "/outputs/a.mp4").as_deref(), Some("a.mp4"));
        assert_eq!(video_ok(&dir, "/outputs/missing.mp4"), None);
        assert_eq!(video_ok(&dir, "/outputs/c.png"), None);
        assert_eq!(video_ok(&dir, "/outputs/../a.mp4"), None);
        assert_eq!(video_ok(&dir, "https://example.com/a.mp4"), None);
        assert_eq!(caption_ok(&dir, "/outputs/c.png").as_deref(), Some("c.png"));
        assert_eq!(audio_ok(&dir, "/outputs/a.mp4").as_deref(), Some("a.mp4"));
    }

    #[test]
    fn spans_follow_the_trim_and_refuse_empty_pieces() {
        let c = |from: f64, to: Option<f64>| Clip { name: "a.mp4".into(), still: false, from, to };
        assert_eq!(span(&c(0.0, None), Some(5.0)), Ok((0.0, 5.0)));
        assert_eq!(span(&c(1.5, Some(4.0)), Some(5.0)), Ok((1.5, 2.5)));
        assert_eq!(span(&c(1.0, Some(9.0)), Some(5.0)), Ok((1.0, 4.0)), "the end is capped at the source length");
        assert!(span(&c(4.0, Some(4.05)), Some(5.0)).is_err());
        assert!(span(&c(6.0, None), Some(5.0)).is_err(), "a start past the end leaves nothing");
        let still = |to: Option<f64>| Clip { name: "a.png".into(), still: true, from: 0.0, to };
        assert_eq!(span(&still(None), None), Ok((0.0, DEFAULT_STILL_SECONDS)));
        assert_eq!(span(&still(Some(2.0)), None), Ok((0.0, 2.0)));
        assert_eq!(span(&still(Some(500.0)), None), Ok((0.0, MAX_STILL_SECONDS)));
        assert_eq!(frame_size("9:16"), Some((1080, 1920)));
        assert_eq!(frame_size("2:1"), None);
    }

    #[test]
    fn overlays_must_run_forwards_without_overlap() {
        let o = |start: f64, end: f64| Overlay { png: "c.png".into(), start, end };
        assert!(overlays_ok(&[o(0.0, 1.0), o(1.0, 1.5), o(2.0, 3.0)]));
        assert!(!overlays_ok(&[o(0.0, 1.0), o(0.9, 1.5)]), "overlap");
        assert!(!overlays_ok(&[o(1.0, 1.0)]), "empty");
        assert!(!overlays_ok(&[o(-1.0, 1.0)]), "before the start");
        assert!(!overlays_ok(&[o(0.0, f64::NAN)]));
        let list = overlay_list(Path::new("/d"), Path::new("/w/blank.png"), &[o(0.5, 1.0), o(1.0, 2.0)]);
        assert_eq!(list, "ffconcat version 1.0\nfile '/w/blank.png'\nduration 0.500\nfile '/d/c.png'\nduration 0.500\nfile '/d/c.png'\nduration 1.000\nfile '/w/blank.png'\nduration 1.000\nfile '/w/blank.png'\n");
    }

    #[test]
    fn concat_paths_survive_apostrophes() {
        assert_eq!(concat_path(Path::new("/srv/o'brien/a.mp4")), "'/srv/o'\\''brien/a.mp4'");
    }

    /// Needs ffmpeg on PATH; skipped quietly when missing. A red caption shows only in its window.
    #[tokio::test]
    async fn dolly_in_end_picture_is_the_start_cropped_in() {
        if std::process::Command::new("ffmpeg").arg("-version").output().is_err() {
            return;
        }
        let dir = crate::fsutil::test_dir("assemble_push_in");
        // A 400x200 picture: blue, with a red 100x50 block in the middle. Cropped in, the red block fills more.
        let ok = std::process::Command::new("ffmpeg")
            .args(["-y", "-v", "error", "-f", "lavfi", "-i", "color=c=blue:s=400x200", "-f", "lavfi", "-i", "color=c=red:s=100x50",
                "-filter_complex", "[0:v][1:v]overlay=150:75", "-frames:v", "1"])
            .arg(dir.join("start.png"))
            .status()
            .unwrap();
        assert!(ok.success());
        let ctx = Ctx { http: reqwest::Client::new(), outputs_dir: dir.clone() };
        let uri = push_in_frame(&ctx, "/outputs/start.png").await.unwrap();
        let b64 = uri.strip_prefix("data:image/jpeg;base64,").expect("a JPEG data URI");
        let bytes = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, b64).unwrap();
        std::fs::write(dir.join("end.jpg"), &bytes).unwrap();
        let (w, h, _, _) = probe(&dir.join("end.jpg")).await.unwrap();
        assert_eq!((w, h), (400, 200), "the end picture keeps the start picture's size");
        // Columns 70 and 330 were blue at the start; after the push they sit inside the red block.
        let px = |x: u32| {
            let out = std::process::Command::new("ffmpeg")
                .args(["-v", "error", "-i"])
                .arg(dir.join("end.jpg"))
                .args(["-vf", &format!("crop=1:1:{x}:100"), "-f", "rawvideo", "-pix_fmt", "rgb24", "-"])
                .output()
                .unwrap();
            (out.stdout[0], out.stdout[2])
        };
        assert!(px(200).0 > 200, "the middle stays red");
        assert!(px(70).1 > 200 && px(70).0 < 60, "the edge is still blue: a push, not a fill");
        assert!(px(140).0 > 200, "the red block grew past its old edge at x=150");
        let missing = push_in_frame(&ctx, "/outputs/nope.png").await;
        assert!(missing.is_err(), "no start picture, no job");
    }

    #[tokio::test]
    async fn timed_captions_show_only_in_their_window() {
        if std::process::Command::new("ffmpeg").arg("-version").output().is_err() {
            return;
        }
        let dir = crate::fsutil::test_dir("assemble_overlays");
        let gen = |args: &[&str]| {
            let ok = std::process::Command::new("ffmpeg").args(["-y", "-v", "error"]).args(args).status().unwrap();
            assert!(ok.success());
        };
        gen(&["-f", "lavfi", "-i", "color=c=black:s=320x240:d=3", "-f", "lavfi", "-i", "sine=d=3", "-shortest", dir.join("a.mp4").to_str().unwrap()]);
        // A caption image: clear except a red square in the middle.
        gen(&["-f", "lavfi", "-i", "color=c=black@0.0:s=320x240,format=rgba", "-f", "lavfi", "-i", "color=c=red:s=40x40,format=rgba",
            "-filter_complex", "[0:v][1:v]overlay=140:100:format=rgb", "-frames:v", "1", dir.join("red.png").to_str().unwrap()]);
        let ctx = Ctx { http: reqwest::Client::new(), outputs_dir: dir.clone() };
        let plan = Plan {
            clips: vec![Clip::whole("a.mp4")],
            captions: vec![None],
            audio: None,
            keep_clip_audio: true,
            frame: None,
            fill: false,
            overlays: vec![Overlay { png: "red.png".into(), start: 1.0, end: 2.0 }],
            follow: false,
            clean_voice: false,
            loudness: false,
        };
        let req = SubmitReq {
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
            push_in: false,
            end_image_url: None,
        };
        let Ok(Submitted::Done(outs)) = Assemble(plan).submit(&ctx, "", &req).await else { panic!("render failed") };
        let out = dir.join(outs[0].url.trim_start_matches("/outputs/"));
        let red_at = |t: &str| {
            let o = std::process::Command::new("ffmpeg")
                .args(["-v", "error", "-ss", t, "-i", out.to_str().unwrap(), "-frames:v", "1", "-vf", "crop=10:10:155:115,scale=1:1", "-f", "rawvideo", "-pix_fmt", "rgb24", "-"])
                .output()
                .unwrap();
            assert_eq!(o.stdout.len(), 3, "one pixel at {t}");
            o.stdout[0] > 150 && o.stdout[1] < 80
        };
        assert!(!red_at("0.5"), "no caption before its start");
        assert!(red_at("1.5"), "the caption shows in its window");
        assert!(!red_at("2.5"), "no caption after its end");
        let (_, _, dur, audio) = probe(&out).await.unwrap();
        assert!((2.8..3.3).contains(&dur) && audio);
    }

    fn plan_of(clips: Vec<Clip>) -> Plan {
        Plan {
            clips,
            captions: Vec::new(),
            audio: None,
            keep_clip_audio: true,
            frame: None,
            fill: false,
            overlays: Vec::new(),
            follow: false,
            clean_voice: false,
            loudness: false,
        }
    }

    fn no_req() -> SubmitReq {
        SubmitReq {
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
            push_in: false,
            end_image_url: None,
        }
    }

    async fn render(dir: &Path, plan: Plan) -> PathBuf {
        let ctx = Ctx { http: reqwest::Client::new(), outputs_dir: dir.to_path_buf() };
        let Ok(Submitted::Done(outs)) = Assemble(plan).submit(&ctx, "", &no_req()).await else { panic!("render failed") };
        dir.join(outs[0].url.trim_start_matches("/outputs/"))
    }

    fn ff(args: &[&str]) {
        assert!(std::process::Command::new("ffmpeg").args(["-y", "-v", "error"]).args(args).status().unwrap().success());
    }

    /// ffmpeg's measured integrated loudness (LUFS) and the RMS level (dB) of seconds a..b.
    fn loudness_of(path: &Path) -> f64 {
        let o = std::process::Command::new("ffmpeg")
            .args(["-hide_banner", "-nostats", "-i", path.to_str().unwrap(), "-af", "ebur128", "-f", "null", "-"])
            .output()
            .unwrap();
        let log = String::from_utf8_lossy(&o.stderr);
        let line = log.lines().rev().find(|l| l.trim_start().starts_with("I:")).unwrap();
        line.split_whitespace().nth(1).unwrap().parse().unwrap()
    }

    fn rms_db(path: &Path, a: f64, b: f64) -> f64 {
        let o = std::process::Command::new("ffmpeg")
            .args(["-hide_banner", "-nostats", "-ss", &a.to_string(), "-t", &(b - a).to_string(), "-i", path.to_str().unwrap(), "-af", "astats=measure_overall=RMS_level:measure_perchannel=none", "-f", "null", "-"])
            .output()
            .unwrap();
        let log = String::from_utf8_lossy(&o.stderr);
        let line = log.lines().find(|l| l.contains("RMS level dB")).unwrap();
        line.rsplit(':').next().unwrap().trim().parse().unwrap()
    }

    #[test]
    fn the_second_loudness_pass_uses_the_measurement() {
        let log = "[Parsed_loudnorm_0 @ 0x1]\n{\n\t\"input_i\" : \"-29.53\",\n\t\"input_tp\" : \"-20.10\",\n\t\"input_lra\" : \"0.00\",\n\t\"input_thresh\" : \"-39.53\",\n\t\"target_offset\" : \"0.23\"\n}\n";
        let af = loudnorm_second_pass(log).unwrap();
        assert!(af.contains("measured_I=-29.53") && af.contains("offset=0.23") && af.contains("linear=true"), "{af}");
        assert_eq!(loudnorm_second_pass(&log.replace("-29.53", "-inf")), None, "silence is left alone");
        assert_eq!(loudnorm_second_pass("no json"), None);
    }

    /// Needs ffmpeg; skipped quietly when missing. A quiet clip comes out at about -14 LUFS.
    #[tokio::test]
    async fn loudness_brings_a_quiet_clip_to_the_target() {
        if std::process::Command::new("ffmpeg").arg("-version").output().is_err() {
            return;
        }
        let dir = crate::fsutil::test_dir("assemble_loudness");
        ff(&["-f", "lavfi", "-i", "color=c=gray:s=320x240:d=4", "-f", "lavfi", "-i", "sine=f=440:d=4,volume=0.03", "-shortest", dir.join("q.mp4").to_str().unwrap()]);
        let before = loudness_of(&dir.join("q.mp4"));
        let mut plan = plan_of(vec![Clip::whole("q.mp4")]);
        plan.loudness = true;
        let out = render(&dir, plan).await;
        let after = loudness_of(&out);
        assert!(before < -25.0, "the source is quiet: {before}");
        assert!((after - LOUDNESS_TARGET).abs() < 1.0, "brought to {LOUDNESS_TARGET}: got {after}");
    }

    /// Needs ffmpeg; skipped quietly when missing. Steady hiss in a pause drops; the tone stays.
    #[tokio::test]
    async fn clean_voice_lowers_steady_noise() {
        if std::process::Command::new("ffmpeg").arg("-version").output().is_err() {
            return;
        }
        let dir = crate::fsutil::test_dir("assemble_clean");
        // 2 s of tone over hiss, then 2 s of hiss alone.
        ff(&["-f", "lavfi", "-i", "color=c=gray:s=320x240:d=4", "-f", "lavfi", "-i", "anoisesrc=c=white:a=0.02:d=4",
            "-f", "lavfi", "-i", "sine=f=300:d=2,volume=0.4,apad=whole_dur=4",
            "-filter_complex", "[1:a][2:a]amix=inputs=2:normalize=0[a]", "-map", "0:v", "-map", "[a]", "-shortest", dir.join("n.mp4").to_str().unwrap()]);
        let plain = render(&dir, plan_of(vec![Clip::whole("n.mp4")])).await;
        let mut plan = plan_of(vec![Clip::whole("n.mp4")]);
        plan.clean_voice = true;
        let clean = render(&dir, plan).await;
        let (hiss_before, hiss_after) = (rms_db(&plain, 2.8, 3.8), rms_db(&clean, 2.8, 3.8));
        let (tone_before, tone_after) = (rms_db(&plain, 0.5, 1.5), rms_db(&clean, 0.5, 1.5));
        assert!(hiss_after < hiss_before - 6.0, "hiss {hiss_before} dB -> {hiss_after} dB");
        assert!((tone_after - tone_before).abs() < 3.0, "the tone keeps its level: {tone_before} -> {tone_after}");
    }

    /// Needs ffmpeg; skipped quietly when missing. With a cached track that puts the subject on the
    /// right, Follow frames the right side of a dark-left / bright-right picture; a plain fill shows the middle.
    #[tokio::test]
    async fn follow_aims_the_crop_at_the_subject() {
        if std::process::Command::new("ffmpeg").arg("-version").output().is_err() {
            return;
        }
        let root = crate::fsutil::test_dir("assemble_follow");
        let dir = root.join("outputs");
        std::fs::create_dir_all(root.join("tracks")).unwrap();
        std::fs::create_dir_all(&dir).unwrap();
        ff(&["-f", "lavfi", "-i", "color=c=black:s=640x360:d=2", "-f", "lavfi", "-i", "color=c=white:s=640x360:d=2",
            "-filter_complex", "[0:v]crop=320:360:0:0[l];[1:v]crop=320:360:0:0[r];[l][r]hstack[v]", "-map", "[v]", dir.join("half.mp4").to_str().unwrap()]);
        let pts: Vec<serde_json::Value> = (0..8).map(|i| serde_json::json!([i as f64 * 0.25, 0.9, 0.5, 2])).collect();
        std::fs::write(root.join("tracks/half.mp4.json"), serde_json::json!({"points": pts}).to_string()).unwrap();
        let brightness = |p: &Path| {
            let o = std::process::Command::new("ffmpeg")
                .args(["-v", "error", "-ss", "1", "-i", p.to_str().unwrap(), "-frames:v", "1", "-vf", "scale=1:1", "-f", "rawvideo", "-pix_fmt", "gray", "-"])
                .output()
                .unwrap();
            o.stdout[0]
        };
        let mut plan = plan_of(vec![Clip::whole("half.mp4")]);
        plan.frame = frame_size("9:16");
        plan.fill = true;
        let middle = brightness(&render(&dir, plan.clone()).await);
        plan.follow = true;
        let followed = brightness(&render(&dir, plan).await);
        assert!((60..200).contains(&middle), "a centred crop straddles the edge: {middle}");
        assert!(followed > 235, "the followed crop is all on the bright side: {followed}");
    }

    /// Needs ffmpeg on PATH; skipped quietly when missing.
    #[tokio::test]
    async fn renders_a_trimmed_clip_and_a_still_into_a_vertical_frame() {
        if std::process::Command::new("ffmpeg").arg("-version").output().is_err() {
            return;
        }
        let dir = crate::fsutil::test_dir("assemble_timeline");
        let gen = |args: &[&str]| {
            let ok = std::process::Command::new("ffmpeg").args(["-y", "-v", "error"]).args(args).status().unwrap();
            assert!(ok.success());
        };
        gen(&["-f", "lavfi", "-i", "testsrc=s=640x360:d=4", "-f", "lavfi", "-i", "sine=d=4", "-shortest", dir.join("a.mp4").to_str().unwrap()]);
        gen(&["-f", "lavfi", "-i", "testsrc=s=400x400", "-frames:v", "1", dir.join("s.png").to_str().unwrap()]);
        gen(&["-f", "lavfi", "-i", "color=c=black@0.0:s=1080x1920,format=rgba", "-frames:v", "1", dir.join("cap.png").to_str().unwrap()]);
        let ctx = Ctx { http: reqwest::Client::new(), outputs_dir: dir.clone() };
        let plan = Plan {
            clips: vec![
                Clip { name: "a.mp4".into(), still: false, from: 1.0, to: Some(2.5) },
                Clip { name: "s.png".into(), still: true, from: 0.0, to: Some(1.0) },
            ],
            captions: vec![None, Some("cap.png".into())],
            audio: None,
            keep_clip_audio: true,
            frame: frame_size("9:16"),
            fill: true,
            overlays: Vec::new(),
            follow: false,
            clean_voice: false,
            loudness: false,
        };
        let req = SubmitReq {
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
            push_in: false,
            end_image_url: None,
        };
        let Ok(Submitted::Done(outs)) = Assemble(plan).submit(&ctx, "", &req).await else { panic!("render failed") };
        let out = dir.join(outs[0].url.trim_start_matches("/outputs/"));
        let (w, h, dur, audio) = probe(&out).await.unwrap();
        assert_eq!((w, h), (1080, 1920));
        assert!((2.3..2.8).contains(&dur), "1.5 s of video plus a 1 s still, got {dur}");
        assert!(audio);
    }

    /// Needs ffmpeg on PATH; skipped quietly when missing.
    #[tokio::test]
    async fn joins_two_clips_with_a_caption_and_a_new_soundtrack() {
        if std::process::Command::new("ffmpeg").arg("-version").output().is_err() {
            return;
        }
        let dir = crate::fsutil::test_dir("assemble_run");
        let gen = |args: &[&str]| {
            let ok = std::process::Command::new("ffmpeg").args(["-y", "-v", "error"]).args(args).status().unwrap();
            assert!(ok.success());
        };
        gen(&["-f", "lavfi", "-i", "testsrc=s=640x360:d=1", "-f", "lavfi", "-i", "sine=d=1", "-shortest", dir.join("a.mp4").to_str().unwrap()]);
        gen(&["-f", "lavfi", "-i", "testsrc=s=360x640:d=1", dir.join("b.mp4").to_str().unwrap()]);
        gen(&["-f", "lavfi", "-i", "color=c=black@0.0:s=640x360,format=rgba", "-frames:v", "1", dir.join("cap.png").to_str().unwrap()]);
        gen(&["-f", "lavfi", "-i", "sine=f=220:d=3", dir.join("song.mp3").to_str().unwrap()]);
        let ctx = Ctx { http: reqwest::Client::new(), outputs_dir: dir.clone() };
        let plan = Plan {
            clips: vec![Clip::whole("a.mp4"), Clip::whole("b.mp4")],
            captions: vec![Some("cap.png".into()), None],
            audio: Some("song.mp3".into()),
            keep_clip_audio: false,
            frame: None,
            fill: false,
            overlays: Vec::new(),
            follow: false,
            clean_voice: false,
            loudness: false,
        };
        let req = SubmitReq {
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
            push_in: false,
            end_image_url: None,
        };
        let Ok(Submitted::Done(outs)) = Assemble(plan).submit(&ctx, "", &req).await else { panic!("assemble failed") };
        let out = dir.join(outs[0].url.trim_start_matches("/outputs/"));
        let (w, h, dur, audio) = probe(&out).await.unwrap();
        assert_eq!((w, h), (640, 360));
        assert!((1.8..2.4).contains(&dur), "two 1 s clips, got {dur}");
        assert!(audio);
        assert!(std::fs::read_dir(&dir).unwrap().all(|e| !e.unwrap().file_name().to_string_lossy().starts_with(".edit-")));
    }
}
