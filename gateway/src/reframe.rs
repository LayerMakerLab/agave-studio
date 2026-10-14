//! "Follow" reframing for Agave Edit: when a clip is cropped to a new shape (16:9 into 9:16, say),
//! the crop follows the subject instead of always taking the middle.
//!
//! Where the subject is comes from `native/vision/agave-vision` (Apple's Vision framework on this
//! Mac: faces first, then people, then the most eye-catching object), sampled 4 times a second and
//! cached in data/tracks/. The camera path is then smoothed like a calm operator would move: it holds
//! while the subject stays near the middle, eases toward it when it drifts, and jumps at a shot
//! change. Without the tool (Linux, or not built) the crop stays in the middle.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use tokio::process::Command;

use crate::fsutil;

const STEP_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const SAMPLES_PER_SECOND: &str = "4";
/// How far (in pan units, 0 = one edge, 1 = the other) the subject may drift before the camera moves.
const DEADZONE: f64 = 0.12;
/// Share of the remaining distance the camera covers per sample once it moves.
const EASE: f64 = 0.3;
/// Once moving, the camera keeps easing until it is this close to the subject.
const SETTLE: f64 = 0.02;
/// A jump this big between two samples is a shot change: the camera cuts instead of panning.
const CUT_JUMP: f64 = 0.3;
/// Keyframes are dropped while the path stays within this of a straight line.
/// Half a frame at 30 fps (and still after the previous frame at 60 fps).
const HALF_FRAME: f64 = 0.015;
const SIMPLIFY: f64 = 0.02;

/// One sample: time in the source (s), subject centre (0..1 from the top left), and what was found
/// (2 face, 1 person, 0 object, -1 nothing).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Spot {
    pub t: f64,
    pub cx: f64,
    pub cy: f64,
    pub kind: i32,
}

/// What the camera path is built from: subject samples and the source's shot changes (seconds).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Track {
    pub spots: Vec<Spot>,
    pub cuts: Vec<f64>,
}

/// Which way the crop can move for a source shape and an output shape (both width / height).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Axis {
    /// The source is wider than the output: the crop slides left and right.
    X,
    /// The source is taller: the crop slides up and down.
    Y,
    None,
}

pub fn axis(src_aspect: f64, out_aspect: f64) -> Axis {
    if src_aspect > out_aspect * 1.01 {
        Axis::X
    } else if src_aspect < out_aspect / 1.01 {
        Axis::Y
    } else {
        Axis::None
    }
}

/// The camera path as (source time, pan) keyframes, pan 0..1 across the room the crop has to move.
pub fn path(track: &Track, src_aspect: f64, out_aspect: f64) -> Vec<(f64, f64)> {
    let spots = &track.spots;
    let ax = axis(src_aspect, out_aspect);
    if spots.is_empty() || ax == Axis::None {
        return Vec::new();
    }
    // Part of the source the crop covers along the moving axis.
    let window = match ax {
        Axis::X => out_aspect / src_aspect,
        _ => src_aspect / out_aspect,
    };
    let room = (1.0 - window).max(1e-6);
    let mut last = 0.5;
    let target: Vec<(f64, f64)> = spots
        .iter()
        .map(|s| {
            // Nothing found: keep looking where we last looked.
            let c = if s.kind < 0 { last } else if ax == Axis::X { s.cx } else { s.cy };
            last = c;
            (s.t, ((c - window / 2.0) / room).clamp(0.0, 1.0))
        })
        .collect();
    // A 3-sample median removes single-frame detection jitter.
    let med: Vec<(f64, f64)> = (0..target.len())
        .map(|i| {
            let lo = i.saturating_sub(1);
            let hi = (i + 1).min(target.len() - 1);
            let mut w: Vec<f64> = target[lo..=hi].iter().map(|x| x.1).collect();
            w.sort_by(|a, b| a.total_cmp(b));
            (target[i].0, w[w.len() / 2])
        })
        .collect();
    let kinds: Vec<i32> = spots.iter().map(|s| s.kind).collect();
    let mut cam = med[0].1;
    let mut moving = false;
    let mut out = Vec::with_capacity(med.len());
    for (i, &(t, p)) in med.iter().enumerate() {
        let step = if i > 0 { (p - med[i - 1].1).abs() } else { 0.0 };
        // A shot change: a big jump, or a different kind of subject in a new place.
        // The source's own shot change between the two samples: re-aim at once instead of drifting over.
        let prev_t = if i > 0 { med[i - 1].0 } else { t };
        let shot = i > 0 && kinds[i] >= 0 && track.cuts.iter().any(|c| *c > prev_t && *c <= t) && (p - cam).abs() > SETTLE * 2.0;
        let cut = shot || i > 0 && (step > CUT_JUMP || (kinds[i] >= 0 && kinds[i - 1] >= 0 && kinds[i] != kinds[i - 1] && step > SETTLE * 2.0));
        if cut {
            // Cut exactly on the source's own shot change when one is close; else at this sample.
            // Half a frame early: frame times are rounded to the stream's time base, and a cut keyed
            // exactly on the new shot's first frame can land one frame late.
            let at = track.cuts.iter().copied().rfind(|c| *c > prev_t - 0.05 && *c <= t + 0.05).map_or(t, |c| c - HALF_FRAME);
            if let Some(&(_, prev)) = out.last() {
                out.push((at - 0.001, prev));
            }
            cam = p;
            moving = false;
            out.push((at, round3(cam)));
            if at < t - 0.001 {
                out.push((t, round3(cam)));
            }
            continue;
        } else {
            // Start easing once the subject leaves the dead zone; keep going until it is framed again.
            if (p - cam).abs() > DEADZONE {
                moving = true;
            }
            if moving {
                cam += (p - cam) * EASE;
                if (p - cam).abs() < SETTLE {
                    moving = false;
                }
            }
        }
        out.push((t, round3(cam)));
    }
    simplify(&out, SIMPLIFY)
}

fn round3(x: f64) -> f64 {
    (x * 1000.0).round() / 1000.0
}

/// Ramer-Douglas-Peucker on (t, p): keeps the keyframes needed to stay within `eps` of the path.
fn simplify(pts: &[(f64, f64)], eps: f64) -> Vec<(f64, f64)> {
    if pts.len() < 3 {
        return pts.to_vec();
    }
    let (a, b) = (pts[0], pts[pts.len() - 1]);
    let mut worst = (0, 0.0);
    for (i, &(t, p)) in pts.iter().enumerate().take(pts.len() - 1).skip(1) {
        let on_line = if b.0 > a.0 { a.1 + (b.1 - a.1) * (t - a.0) / (b.0 - a.0) } else { a.1 };
        let d = (p - on_line).abs();
        if d > worst.1 {
            worst = (i, d);
        }
    }
    if worst.1 <= eps {
        return vec![a, b];
    }
    let mut left = simplify(&pts[..=worst.0], eps);
    let right = simplify(&pts[worst.0..], eps);
    left.pop();
    left.extend(right);
    left
}

/// An ffmpeg expression for the pan at input time `t` (seconds from the clip's trimmed start),
/// built from keyframes in source time. Flat (no nesting), so long paths stay valid.
pub fn pan_expr(keys: &[(f64, f64)], from: f64) -> String {
    let k: Vec<(f64, f64)> = keys.iter().map(|&(t, p)| (t - from, p)).collect();
    match k.len() {
        0 => "0.5".into(),
        1 => format!("{:.4}", k[0].1),
        n => {
            let mut terms = vec![format!("lt(t,{:.3})*{:.4}", k[0].0, k[0].1)];
            for w in k.windows(2) {
                let ((t0, p0), (t1, p1)) = (w[0], w[1]);
                let span = (t1 - t0).max(1e-3);
                terms.push(format!("gte(t,{t0:.3})*lt(t,{t1:.3})*({p0:.4}+{:.5}*(t-{t0:.3}))", (p1 - p0) / span));
            }
            terms.push(format!("gte(t,{:.3})*{:.4}", k[n - 1].0, k[n - 1].1));
            terms.join("+")
        }
    }
}

/// Shot-change times from ffmpeg's showinfo log (frames picked by select='gt(scene,..)').
pub fn parse_cuts(log: &str) -> Vec<f64> {
    log.lines()
        .filter(|l| l.contains("showinfo"))
        .filter_map(|l| {
            let rest = &l[l.find("pts_time:")? + 9..];
            rest.split_whitespace().next()?.parse::<f64>().ok().filter(|t| t.is_finite())
        })
        .collect()
}

pub fn parse_track(json: &serde_json::Value) -> Track {
    let cuts = json.get("cuts").and_then(|c| c.as_array()).into_iter().flatten().filter_map(|v| v.as_f64()).collect();
    Track { spots: parse_spots(json), cuts }
}

pub fn parse_spots(json: &serde_json::Value) -> Vec<Spot> {
    json.get("points")
        .and_then(|p| p.as_array())
        .into_iter()
        .flatten()
        .filter_map(|p| {
            let a = p.as_array()?;
            let n = |i: usize| a.get(i).and_then(|v| v.as_f64()).filter(|v| v.is_finite());
            Some(Spot { t: n(0)?, cx: n(1)?.clamp(0.0, 1.0), cy: n(2)?.clamp(0.0, 1.0), kind: n(3)? as i32 })
        })
        .collect()
}

pub struct Tracker {
    data_dir: PathBuf,
}

impl Tracker {
    pub fn new(outputs_dir: &Path) -> Self {
        Tracker { data_dir: outputs_dir.parent().map(Path::to_path_buf).unwrap_or_else(|| outputs_dir.to_path_buf()) }
    }

    fn tool(&self) -> PathBuf {
        std::env::var_os("AGAVE_VISION_BIN")
            .map(PathBuf::from)
            .unwrap_or_else(|| self.data_dir.join("../native/vision/agave-vision"))
    }

    /// Where the subject is through /outputs/<name> and where its shots change; None when the
    /// tool is missing or fails (the caller then crops the middle).
    pub async fn track(&self, outputs_dir: &Path, name: &str) -> Option<Track> {
        let cache = self.data_dir.join("tracks").join(format!("{name}.json"));
        if let Ok(bytes) = tokio::fs::read(&cache).await {
            if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&bytes) {
                return Some(parse_track(&v));
            }
        }
        let tool = self.tool();
        if !tool.is_file() {
            return None;
        }
        let src = outputs_dir.join(name);
        let mut cmd = Command::new(tool);
        cmd.arg(&src).arg(SAMPLES_PER_SECOND);
        cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).kill_on_drop(true);
        let out = tokio::time::timeout(STEP_TIMEOUT, cmd.spawn().ok()?.wait_with_output()).await.ok()?.ok()?;
        if !out.status.success() {
            return None;
        }
        let mut v: serde_json::Value = serde_json::from_slice(&out.stdout).ok()?;
        // Shot changes, so the camera cuts with the picture instead of a few frames off.
        let mut ff = Command::new("ffmpeg");
        ff.args(["-hide_banner", "-nostats", "-v", "info", "-i"])
            .arg(&src)
            .args(["-an", "-vf", "scale=160:-2,select='gt(scene,0.3)',showinfo", "-f", "null", "-"]);
        ff.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped()).kill_on_drop(true);
        let cuts = match tokio::time::timeout(STEP_TIMEOUT, ff.spawn().ok()?.wait_with_output()).await {
            Ok(Ok(o)) if o.status.success() => parse_cuts(&String::from_utf8_lossy(&o.stderr)),
            _ => Vec::new(),
        };
        v["cuts"] = serde_json::json!(cuts);
        let _ = std::fs::create_dir_all(self.data_dir.join("tracks"));
        if let Ok(bytes) = serde_json::to_vec(&v) {
            let _ = fsutil::write_private(&cache, &bytes);
        }
        Some(parse_track(&v))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spot(t: f64, cx: f64, kind: i32) -> Spot {
        Spot { t, cx, cy: 0.5, kind }
    }

    #[test]
    fn the_axis_follows_the_shapes() {
        assert_eq!(axis(16.0 / 9.0, 9.0 / 16.0), Axis::X);
        assert_eq!(axis(9.0 / 16.0, 16.0 / 9.0), Axis::Y);
        assert_eq!(axis(1.0, 1.0), Axis::None);
        assert!(path(&Track { spots: vec![spot(0.0, 0.9, 2)], cuts: vec![] }, 1.0, 1.0).is_empty(), "same shape: nothing to follow");
    }

    #[test]
    fn a_still_subject_gives_a_still_camera_aimed_at_it() {
        // 16:9 into 9:16: the crop covers 0.316 of the width, so a face at 0.8 needs pan
        // (0.8 - 0.158) / 0.684 = 0.94.
        let spots: Vec<Spot> = (0..20).map(|i| spot(i as f64 * 0.25, 0.8 + if i % 2 == 0 { 0.01 } else { -0.01 }, 2)).collect();
        let p = path(&Track { spots: spots.clone(), cuts: vec![] }, 16.0 / 9.0, 9.0 / 16.0);
        assert_eq!(p.len(), 2, "jitter is smoothed away: {p:?}");
        assert!((p[0].1 - 0.94).abs() < 0.03 && (p[1].1 - 0.94).abs() < 0.03, "{p:?}");
    }

    #[test]
    fn small_drift_is_ignored_and_a_shot_change_cuts() {
        let mut spots: Vec<Spot> = (0..8).map(|i| spot(i as f64 * 0.25, 0.5 + i as f64 * 0.005, 2)).collect();
        spots.extend((8..16).map(|i| spot(i as f64 * 0.25, 0.15, 0)));
        let p = path(&Track { spots: spots.clone(), cuts: vec![] }, 16.0 / 9.0, 9.0 / 16.0);
        let before = p.iter().filter(|k| k.0 < 1.9).map(|k| k.1).collect::<Vec<_>>();
        assert!(before.iter().all(|&x| (x - before[0]).abs() < 1e-9), "held still: {p:?}");
        let cut = p.iter().position(|k| k.0 >= 2.0).unwrap();
        assert!((p[cut - 1].0 - 1.999).abs() < 1e-6 && (p[cut].0 - 2.0).abs() < 1e-6, "a hard cut at the shot change: {p:?}");
        assert!(p[cut].1 < 0.05, "the new subject on the left is framed at once: {p:?}");
    }

    #[test]
    fn a_new_kind_of_subject_cuts_and_a_drifting_one_is_caught_up() {
        // Face at pan ~0.57, then (new shot) an object at pan ~0.69: a cut, not a slow creep.
        let mut spots: Vec<Spot> = (0..8).map(|i| spot(i as f64 * 0.25, 0.55, 2)).collect();
        spots.extend((8..16).map(|i| spot(i as f64 * 0.25, 0.63, 0)));
        let p = path(&Track { spots: spots.clone(), cuts: vec![] }, 16.0 / 9.0, 9.0 / 16.0);
        let at = |t: f64| p.iter().rev().find(|k| k.0 <= t).unwrap().1;
        assert!((at(1.9) - 0.573).abs() < 0.01 && (at(2.0) - 0.69).abs() < 0.01, "{p:?}");
        // One subject walking steadily right: once it leaves the dead zone the camera follows all the way.
        let walk: Vec<Spot> = (0..40).map(|i| spot(i as f64 * 0.25, 0.3 + i as f64 * 0.012, 1)).collect();
        let p = path(&Track { spots: walk.clone(), cuts: vec![] }, 16.0 / 9.0, 9.0 / 16.0);
        let target_end = ((0.3 + 39.0 * 0.012) - 0.158) / 0.684;
        assert!((p.last().unwrap().1 - target_end).abs() < 0.13, "{p:?}");
        assert!(p.last().unwrap().1 > p[0].1 + 0.3, "the camera moved with it: {p:?}");
    }

    #[test]
    fn the_camera_cuts_on_the_real_shot_change() {
        // Samples every 0.25 s; the shot really changes at 1.1 s, between the 1.0 and 1.25 samples.
        let mut spots: Vec<Spot> = (0..5).map(|i| spot(i as f64 * 0.25, 0.55, 2)).collect();
        spots.extend((5..10).map(|i| spot(i as f64 * 0.25, 0.63, 0)));
        let p = path(&Track { spots, cuts: vec![1.1] }, 16.0 / 9.0, 9.0 / 16.0);
        assert!(p.iter().any(|k| (k.0 - 1.084).abs() < 1e-6) && p.iter().any(|k| (k.0 - 1.085).abs() < 1e-6), "{p:?}");
        let at = |t: f64| p.iter().rev().find(|k| k.0 <= t).unwrap().1;
        assert!(at(1.05) < 0.6 && at(1.15) > 0.65, "{p:?}");
        // A shot change to a subject only a little way off still cuts rather than drifting across.
        let mut near: Vec<Spot> = (0..5).map(|i| spot(i as f64 * 0.25, 0.62, 0)).collect();
        near.extend((5..10).map(|i| spot(i as f64 * 0.25, 0.55, 0)));
        let p = path(&Track { spots: near.clone(), cuts: vec![1.1] }, 16.0 / 9.0, 9.0 / 16.0);
        let at = |t: f64| p.iter().rev().find(|k| k.0 <= t).unwrap().1;
        assert!(at(1.05) > 0.62 && at(1.15) < 0.6, "{p:?}");
        let log = "[Parsed_showinfo_2 @ 0x1] n:   0 pts:  15000 pts_time:5      duration:512\n[Parsed_showinfo_2 @ 0x1] n:   1 pts:  30000 pts_time:10.1   x\n";
        assert_eq!(parse_cuts(log), vec![5.0, 10.1]);
    }

    #[test]
    fn nothing_found_keeps_the_last_framing() {
        let spots = vec![spot(0.0, 0.8, 2), spot(0.25, 0.8, 2), spot(0.5, 0.0, -1), spot(0.75, 0.0, -1)];
        let p = path(&Track { spots: spots.clone(), cuts: vec![] }, 16.0 / 9.0, 9.0 / 16.0);
        assert!(p.iter().all(|k| (k.1 - p[0].1).abs() < 1e-9), "{p:?}");
    }

    #[test]
    fn pan_expressions_are_flat_and_start_at_the_trim() {
        assert_eq!(pan_expr(&[], 0.0), "0.5");
        assert_eq!(pan_expr(&[(1.0, 0.25)], 0.0), "0.2500");
        let e = pan_expr(&[(2.0, 0.0), (4.0, 1.0)], 2.0);
        assert_eq!(e, "lt(t,0.000)*0.0000+gte(t,0.000)*lt(t,2.000)*(0.0000+0.50000*(t-0.000))+gte(t,2.000)*1.0000");
        assert!(!e.contains(',') || e.matches('(').count() == e.matches(')').count());
    }

    /// Runs the real tool when it is built (macOS); skipped quietly otherwise.
    #[tokio::test]
    async fn the_vision_tool_finds_a_face_or_object() {
        let tool = Path::new(env!("CARGO_MANIFEST_DIR")).join("../native/vision/agave-vision");
        let clip = Path::new(env!("CARGO_MANIFEST_DIR")).join("../data/outputs/c484de211feeb08bdd51ebb9.mp4");
        if !tool.is_file() || !clip.is_file() {
            return;
        }
        let out = std::process::Command::new(&tool).arg(&clip).arg("2").output().unwrap();
        assert!(out.status.success());
        let spots = parse_spots(&serde_json::from_slice(&out.stdout).unwrap());
        assert!(spots.windows(2).all(|w| w[1].t > w[0].t), "samples run forwards");
        assert!(spots.len() >= 15, "{}", spots.len());
        assert!(spots.iter().filter(|s| s.kind == 2).count() >= 5, "the lantern man's face is found");
    }
}
