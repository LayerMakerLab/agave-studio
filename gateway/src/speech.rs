//! Speech tools for Agave Edit, all local and free: pause finding with ffmpeg's silencedetect, and
//! word-timed transcripts on this Mac: NVIDIA Parakeet v3 through native/speech/agave-speech when its
//! model is installed (fast, on the Neural Engine), else whisper.cpp (`whisper-cli` plus a ggml model
//! in data/models). Both get the audio cut at pauses so no word lands in a pause.
//!
//! Whisper's word times drift across long pauses, so the audio is first cut at the pauses and each
//! stretch of speech is transcribed on its own; a word can then never land inside a pause.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::OnceLock;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::process::Command;
use tokio::sync::Semaphore;

use crate::fsutil;

const STEP_TIMEOUT: Duration = Duration::from_secs(10 * 60);
/// Loudness below this counts as a pause when cutting audio for whisper.
const SPLIT_DB: f64 = -35.0;
const SPLIT_MIN_PAUSE: f64 = 0.3;
/// Speech kept on each side of a cut, so word edges are not clipped.
const SPLIT_PAD: f64 = 0.15;
const MIN_SPEECH: f64 = 0.2;
/// Longest source the editor will transcribe, in seconds.
pub const MAX_TRANSCRIBE_SECONDS: f64 = 1800.0;
const DEFAULT_MODEL: &str = "models/ggml-small.bin";

#[derive(Debug, PartialEq)]
pub enum SpeechError {
    /// whisper-cli or the model file is missing.
    Unavailable,
    TooLong,
    Failed(String),
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Word {
    pub t0: f64,
    pub t1: f64,
    pub text: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Transcript {
    pub language: String,
    pub words: Vec<Word>,
}

/// One transcription at a time; whisper already uses every core.
fn gate() -> &'static Semaphore {
    static GATE: OnceLock<Semaphore> = OnceLock::new();
    GATE.get_or_init(|| Semaphore::new(1))
}

/// Runs a tool and returns (stdout, stderr). Err with the last error line when it fails.
async fn run(cmd: &mut Command) -> Result<(Vec<u8>, String), SpeechError> {
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    let child = cmd.spawn().map_err(|_| SpeechError::Unavailable)?;
    let out = tokio::time::timeout(STEP_TIMEOUT, child.wait_with_output())
        .await
        .map_err(|_| SpeechError::Failed("took too long".into()))?
        .map_err(|e| SpeechError::Failed(e.to_string()))?;
    let err = String::from_utf8_lossy(&out.stderr).to_string();
    if !out.status.success() {
        let last = err.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("").chars().take(200).collect();
        return Err(SpeechError::Failed(last));
    }
    Ok((out.stdout, err))
}

/// Length in seconds and whether the file has a sound track.
pub async fn probe_audio(path: &Path) -> Result<(f64, bool), SpeechError> {
    let (out, _) = run(Command::new("ffprobe")
        .args(["-v", "error", "-show_entries", "stream=codec_type:format=duration", "-of", "json"])
        .arg(path))
    .await?;
    let v: serde_json::Value = serde_json::from_slice(&out).map_err(|_| SpeechError::Failed("could not read the file".into()))?;
    let has_audio = v
        .get("streams")
        .and_then(|s| s.as_array())
        .is_some_and(|s| s.iter().any(|x| x.get("codec_type").and_then(|t| t.as_str()) == Some("audio")));
    let len = v
        .pointer("/format/duration")
        .and_then(|d| d.as_str())
        .and_then(|d| d.parse::<f64>().ok())
        .filter(|d| d.is_finite() && *d > 0.0)
        .ok_or_else(|| SpeechError::Failed("the file has no length".into()))?;
    Ok((len, has_audio))
}

/// Pulls (start, end) pairs out of silencedetect's log. A pause still open at the end runs to `len`.
pub fn parse_silences(log: &str, len: f64) -> Vec<(f64, f64)> {
    let num = |line: &str, key: &str| -> Option<f64> {
        let rest = &line[line.find(key)? + key.len()..];
        rest.trim_start().split(|c: char| c.is_whitespace() || c == '|').next()?.parse().ok()
    };
    let mut out = Vec::new();
    let mut open: Option<f64> = None;
    for line in log.lines() {
        if let Some(s) = num(line, "silence_start:") {
            open = Some(s.max(0.0));
        } else if let Some(e) = num(line, "silence_end:") {
            if let Some(s) = open.take() {
                out.push((s, e.min(len)));
            }
        }
    }
    if let Some(s) = open {
        if s < len {
            out.push((s, len));
        }
    }
    out
}

/// Pauses at least `min_len` long and quieter than `db` (dBFS).
pub async fn silences(path: &Path, db: f64, min_len: f64, len: f64) -> Result<Vec<(f64, f64)>, SpeechError> {
    let filter = format!("silencedetect=noise={db}dB:d={min_len}");
    let (_, log) = run(Command::new("ffmpeg")
        .args(["-hide_banner", "-nostats", "-v", "info", "-i"])
        .arg(path)
        .args(["-vn", "-af", &filter, "-f", "null", "-"]))
    .await?;
    Ok(parse_silences(&log, len))
}

/// The stretches between pauses, padded and merged, as (start, end).
pub fn speech_regions(pauses: &[(f64, f64)], len: f64) -> Vec<(f64, f64)> {
    let mut regions: Vec<(f64, f64)> = Vec::new();
    let mut at = 0.0;
    for &(s, e) in pauses {
        if s > at {
            regions.push((at, s));
        }
        at = at.max(e);
    }
    if at < len {
        regions.push((at, len));
    }
    let mut merged: Vec<(f64, f64)> = Vec::new();
    for (s, e) in regions {
        let (s, e) = ((s - SPLIT_PAD).max(0.0), (e + SPLIT_PAD).min(len));
        match merged.last_mut() {
            Some(last) if s <= last.1 => last.1 = last.1.max(e),
            _ => merged.push((s, e)),
        }
    }
    merged.retain(|(s, e)| e - s >= MIN_SPEECH);
    merged
}

/// Words from one whisper JSON file (run with one word per segment), moved by `offset` and kept
/// inside [offset, end]. Non-speech marks like [BLANK_AUDIO] or (music) are dropped.
pub fn words_from_whisper(json: &serde_json::Value, offset: f64, end: f64) -> Vec<Word> {
    let mut out = Vec::new();
    // Sound labels like "(upbeat music)" or "[BLANK_AUDIO]" can span several one-word segments.
    let mut in_label = false;
    for seg in json.get("transcription").and_then(|t| t.as_array()).into_iter().flatten() {
        let text = seg.get("text").and_then(|t| t.as_str()).unwrap_or("").trim().to_string();
        if text.is_empty() {
            continue;
        }
        if text.starts_with(['(', '[', '*', '♪']) {
            in_label = true;
        }
        if in_label {
            in_label = !text.ends_with([')', ']', '*', '♪']);
            continue;
        }
        let ms = |k: &str| seg.pointer(&format!("/offsets/{k}")).and_then(|v| v.as_f64());
        let (Some(a), Some(b)) = (ms("from"), ms("to")) else { continue };
        let t0 = (offset + a / 1000.0).min(end);
        let t1 = (offset + b / 1000.0).clamp(t0, end);
        out.push(Word { t0: round3(t0), t1: round3(t1), text: text.chars().take(64).collect() });
    }
    out
}

/// Words from one agave-speech JSON line for the stretch (s, e) of a `len`-second file. The stretch
/// is padded by SPLIT_PAD on each side; Parakeet's word ends run on to the next word, so words are
/// kept inside the unpadded speech, which is where the pauses really start and end.
pub fn words_from_parakeet(line: &serde_json::Value, s: f64, e: f64, len: f64) -> Vec<Word> {
    let lo = if s > 0.0 { s + SPLIT_PAD } else { 0.0 };
    let hi = if e < len { e - SPLIT_PAD } else { len };
    let (lo, hi) = if hi > lo { (lo, hi) } else { (s, e) };
    let mut out = Vec::new();
    let mut in_label = false;
    for w in line.get("words").and_then(|w| w.as_array()).into_iter().flatten() {
        let (Some(a), Some(b), Some(text)) = (w.get(0).and_then(|v| v.as_f64()), w.get(1).and_then(|v| v.as_f64()), w.get(2).and_then(|v| v.as_str())) else {
            continue;
        };
        let text = text.trim();
        if text.is_empty() {
            continue;
        }
        if text.starts_with(['(', '[', '*', '♪']) {
            in_label = true;
        }
        if in_label {
            in_label = !text.ends_with([')', ']', '*', '♪']);
            continue;
        }
        let t0 = (s + a).clamp(lo, hi);
        let t1 = (s + b).clamp(t0, hi);
        out.push(Word { t0: round3(t0), t1: round3(t1), text: text.chars().take(64).collect() });
    }
    out
}

/// Which engine makes transcripts.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Engine {
    Parakeet,
    Whisper,
}

fn round3(x: f64) -> f64 {
    (x * 1000.0).round() / 1000.0
}

pub struct Speech {
    data_dir: PathBuf,
}

impl Speech {
    pub fn new(outputs_dir: &Path) -> Self {
        Speech { data_dir: outputs_dir.parent().map(Path::to_path_buf).unwrap_or_else(|| outputs_dir.to_path_buf()) }
    }

    fn model(&self) -> PathBuf {
        std::env::var_os("AGAVE_WHISPER_MODEL").map(PathBuf::from).unwrap_or_else(|| self.data_dir.join(DEFAULT_MODEL))
    }

    /// native/speech/agave-speech next to the data folder, or AGAVE_SPEECH_BIN.
    pub fn speech_tool(&self) -> PathBuf {
        std::env::var_os("AGAVE_SPEECH_BIN").map(PathBuf::from).unwrap_or_else(|| self.data_dir.join("../native/speech/agave-speech"))
    }

    fn parakeet_dir(&self) -> PathBuf {
        crate::setup::model("parakeet-v3").map(|m| m.dest(&self.data_dir)).unwrap_or_default()
    }

    /// Parakeet when its tool and model are installed, else Whisper when its model is, else nothing.
    pub fn engine(&self) -> Option<Engine> {
        let parakeet = crate::setup::model("parakeet-v3").is_some_and(|m| m.installed(&self.data_dir));
        if parakeet && self.speech_tool().is_file() {
            Some(Engine::Parakeet)
        } else if self.model().is_file() {
            Some(Engine::Whisper)
        } else {
            None
        }
    }

    /// Loads Parakeet once on a second of silence, which compiles it for this Mac.
    pub fn warm_parakeet(&self) -> bool {
        let wav = self.data_dir.join(format!(".warm-{}.wav", crate::media::random_hex(4)));
        let made = std::process::Command::new("ffmpeg")
            .args(["-y", "-v", "error", "-f", "lavfi", "-i", "anullsrc=r=16000:cl=mono", "-t", "1"])
            .arg(&wav)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        let ok = made && std::process::Command::new(self.speech_tool()).arg(self.parakeet_dir()).arg(&wav).output().map(|o| o.status.success()).unwrap_or(false);
        let _ = std::fs::remove_file(&wav);
        ok
    }

    fn cache_path(&self, name: &str) -> PathBuf {
        self.data_dir.join("transcripts").join(format!("{name}.json"))
    }

    /// The transcript of /outputs/<name>, made once and then read from data/transcripts.
    pub async fn transcript(&self, outputs_dir: &Path, name: &str) -> Result<Transcript, SpeechError> {
        let cache = self.cache_path(name);
        if let Ok(bytes) = tokio::fs::read(&cache).await {
            if let Ok(t) = serde_json::from_slice::<Transcript>(&bytes) {
                return Ok(t);
            }
        }
        let Some(engine) = self.engine() else {
            return Err(SpeechError::Unavailable);
        };
        let _turn = gate().acquire().await.map_err(|_| SpeechError::Failed("busy".into()))?;
        let src = outputs_dir.join(name);
        let (len, has_audio) = probe_audio(&src).await?;
        if len > MAX_TRANSCRIBE_SECONDS {
            return Err(SpeechError::TooLong);
        }
        let t = if has_audio { self.whisper(&src, len, engine).await? } else { Transcript { language: String::new(), words: Vec::new() } };
        if let Ok(bytes) = serde_json::to_vec(&t) {
            let _ = std::fs::create_dir_all(self.data_dir.join("transcripts"));
            let _ = fsutil::write_private(&cache, &bytes);
        }
        Ok(t)
    }

    async fn whisper(&self, src: &Path, len: f64, engine: Engine) -> Result<Transcript, SpeechError> {
        let work = self.data_dir.join(format!(".speech-{}", crate::media::random_hex(6)));
        tokio::fs::create_dir_all(&work).await.map_err(|e| SpeechError::Failed(e.to_string()))?;
        let result = self.whisper_in(src, len, &work, engine).await;
        let _ = tokio::fs::remove_dir_all(&work).await;
        result
    }

    async fn whisper_in(&self, src: &Path, len: f64, work: &Path, engine: Engine) -> Result<Transcript, SpeechError> {
        let wav = work.join("all.wav");
        run(Command::new("ffmpeg").args(["-y", "-v", "error", "-i"]).arg(src).args(["-vn", "-ac", "1", "-ar", "16000", "-c:a", "pcm_s16le"]).arg(&wav))
            .await?;
        let pauses = silences(&wav, SPLIT_DB, SPLIT_MIN_PAUSE, len).await?;
        let regions = speech_regions(&pauses, len);
        if regions.is_empty() {
            return Ok(Transcript { language: String::new(), words: Vec::new() });
        }
        let mut cmd = match engine {
            Engine::Parakeet => {
                let mut c = Command::new(self.speech_tool());
                c.arg(self.parakeet_dir());
                c
            }
            Engine::Whisper => {
                let mut c = Command::new("whisper-cli");
                c.arg("-m").arg(self.model()).args(["-l", "auto", "-ml", "1", "-sow", "-oj", "-np", "-t", "4"]);
                c
            }
        };
        for (i, (s, e)) in regions.iter().enumerate() {
            let part = work.join(format!("r{i:04}.wav"));
            run(Command::new("ffmpeg")
                .args(["-y", "-v", "error", "-ss", &format!("{s:.3}"), "-to", &format!("{e:.3}"), "-i"])
                .arg(&wav)
                .args(["-c", "copy"])
                .arg(&part))
            .await?;
            if engine == Engine::Whisper {
                cmd.arg("-f");
            }
            cmd.arg(&part);
        }
        let (stdout, _) = run(&mut cmd).await?;
        if engine == Engine::Parakeet {
            // One JSON line per part, matched to its region by the "file" it names (any other output is ignored).
            let mut words = Vec::new();
            for line in String::from_utf8_lossy(&stdout).lines() {
                let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else { continue };
                let Some(file) = v.get("file").and_then(|f| f.as_str()) else { continue };
                let region = (0..regions.len()).find(|i| work.join(format!("r{i:04}.wav")) == Path::new(file));
                if let Some((s, e)) = region.map(|i| regions[i]) {
                    words.extend(words_from_parakeet(&v, s, e, len));
                }
            }
            words.sort_by(|a, b| a.t0.total_cmp(&b.t0));
            return Ok(Transcript { language: String::new(), words });
        }
        let mut words = Vec::new();
        let mut language = String::new();
        for (i, (s, e)) in regions.iter().enumerate() {
            let Ok(bytes) = tokio::fs::read(work.join(format!("r{i:04}.wav.json"))).await else { continue };
            let Ok(v) = serde_json::from_slice::<serde_json::Value>(&bytes) else { continue };
            if language.is_empty() {
                language = v.pointer("/result/language").and_then(|l| l.as_str()).unwrap_or("").to_string();
            }
            words.extend(words_from_whisper(&v, *s, *e));
        }
        Ok(Transcript { language, words })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn silencedetect_logs_become_pauses() {
        let log = "[silencedetect @ 0x1] silence_start: 2.525125\n\
                   [silencedetect @ 0x1] silence_end: 4.340816 | silence_duration: 1.8\n\
                   size=N/A time=00:00:11.05\n\
                   [silencedetect @ 0x1] silence_start: 9.9\n";
        assert_eq!(parse_silences(log, 11.0), vec![(2.525125, 4.340816), (9.9, 11.0)]);
        assert_eq!(parse_silences("[silencedetect @ 0x1] silence_start: -0.01\n[silencedetect @ 0x1] silence_end: 1.5 | x\n", 5.0), vec![(0.0, 1.5)]);
        assert!(parse_silences("nothing here", 3.0).is_empty());
    }

    #[test]
    fn speech_sits_between_pauses_with_padding() {
        let r = speech_regions(&[(2.5, 4.3), (7.4, 9.9)], 11.0);
        assert_eq!(r.len(), 3);
        assert!((r[0].0 - 0.0).abs() < 1e-9 && (r[0].1 - 2.65).abs() < 1e-9);
        assert!((r[1].0 - 4.15).abs() < 1e-9 && (r[1].1 - 7.55).abs() < 1e-9);
        assert!((r[2].0 - 9.75).abs() < 1e-9 && (r[2].1 - 11.0).abs() < 1e-9);
        // A short pause merges once padded; an all-silent file has no speech.
        assert_eq!(speech_regions(&[(1.0, 1.2)], 3.0), vec![(0.0, 3.0)]);
        assert!(speech_regions(&[(0.0, 3.0)], 3.0).is_empty());
    }

    #[test]
    fn parakeet_words_stay_inside_the_speech_not_the_padding() {
        // A stretch cut from 4.0 to 7.6 s of a 12 s file: speech really runs 4.15 to 7.45 s.
        let line = json!({"file": "r1.wav", "duration": 3.6, "words": [[0.0, 0.4, "Today"], [0.4, 0.6, "I"], [2.9, 3.6, "printer."], [3.0, 3.1, "(music)"]]});
        let w = words_from_parakeet(&line, 4.0, 7.6, 12.0);
        let t: Vec<(f64, f64, &str)> = w.iter().map(|w| (w.t0, w.t1, w.text.as_str())).collect();
        assert_eq!(t, vec![(4.15, 4.4, "Today"), (4.4, 4.6, "I"), (6.9, 7.45, "printer.")]);
        // At the very start and end of the file there is no padding to take off.
        let w = words_from_parakeet(&json!({"words": [[0.0, 0.5, "Hello"]]}), 0.0, 2.0, 2.0);
        assert_eq!((w[0].t0, w[0].t1), (0.0, 0.5));
    }

    #[test]
    fn whisper_words_are_moved_clamped_and_cleaned() {
        let v = json!({"transcription": [
            {"offsets": {"from": 0, "to": 50}, "text": ""},
            {"offsets": {"from": 50, "to": 400}, "text": " Hello"},
            {"offsets": {"from": 400, "to": 900}, "text": " [BLANK_AUDIO]"},
            {"offsets": {"from": 900, "to": 9000}, "text": " world."},
            {"offsets": {"from": 950, "to": 990}, "text": " (music)"},
            {"offsets": {"from": 1000, "to": 1100}, "text": " (upbeat"},
            {"offsets": {"from": 1100, "to": 1200}, "text": " music)"},
            {"offsets": {"from": 1200, "to": 1300}, "text": " ♪"}
        ]});
        let w = words_from_whisper(&v, 4.0, 6.0);
        assert_eq!(w, vec![
            Word { t0: 4.05, t1: 4.4, text: "Hello".into() },
            Word { t0: 4.9, t1: 6.0, text: "world.".into() },
        ]);
    }

    /// Needs whisper-cli, ffmpeg and data/models/ggml-small.bin; skipped quietly when missing.
    #[tokio::test]
    async fn transcribes_speech_with_pauses_and_keeps_words_out_of_them() {
        let model = Path::new(env!("CARGO_MANIFEST_DIR")).join("../data/models/ggml-small.bin");
        let have = |c: &str| std::process::Command::new(c).arg("-h").output().is_ok();
        if !model.is_file() || !have("whisper-cli") || !have("ffmpeg") || !have("say") {
            return;
        }
        let dir = crate::fsutil::test_dir("speech_run");
        let outputs = dir.join("outputs");
        std::fs::create_dir_all(&outputs).unwrap();
        std::fs::create_dir_all(dir.join("models")).unwrap();
        std::os::unix::fs::symlink(&model, dir.join("models/ggml-small.bin")).unwrap();
        let sh = |args: &[&str]| assert!(std::process::Command::new(args[0]).args(&args[1..]).status().unwrap().success());
        let a = dir.join("a.aiff");
        let b = dir.join("b.aiff");
        sh(&["say", "-o", a.to_str().unwrap(), "Welcome back to the channel."]);
        sh(&["say", "-o", b.to_str().unwrap(), "Let's get started."]);
        let clip = outputs.join("talk.mp4");
        sh(&["ffmpeg", "-y", "-v", "error", "-i", a.to_str().unwrap(), "-f", "lavfi", "-t", "2", "-i", "anullsrc=r=22050:cl=mono",
            "-i", b.to_str().unwrap(), "-filter_complex", "[0:a]aresample=22050[x];[2:a]aresample=22050[z];[x][1:a][z]concat=n=3:v=0:a=1[a]",
            "-map", "[a]", "-c:a", "aac", clip.to_str().unwrap()]);
        let speech = Speech::new(&outputs);
        let (len, has_audio) = probe_audio(&clip).await.unwrap();
        assert!(has_audio);
        let pauses = silences(&clip, -35.0, 0.5, len).await.unwrap();
        assert_eq!(pauses.len(), 1, "{pauses:?}");
        let (ps, pe) = pauses[0];
        let t = speech.transcript(&outputs, "talk.mp4").await.unwrap();
        let text = t.words.iter().map(|w| w.text.to_lowercase()).collect::<Vec<_>>().join(" ");
        assert!(text.contains("welcome") && text.contains("started"), "{text}");
        for w in &t.words {
            let mid = (w.t0 + w.t1) / 2.0;
            assert!(!(mid > ps + 0.2 && mid < pe - 0.2), "{w:?} sits inside the pause {ps}-{pe}");
        }
        assert!(dir.join("transcripts/talk.mp4.json").is_file(), "the transcript is cached");
        assert!(std::fs::read_dir(&dir).unwrap().all(|e| !e.unwrap().file_name().to_string_lossy().starts_with(".speech-")));
    }
}
