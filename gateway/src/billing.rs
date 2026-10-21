//! What fal actually charged, read with an optional read-only key (FAL_BILLING_KEY, fal's BILLING preset).
//!
//! Every fal job keeps fal's request id (`Cost::request_id`). Once fal's billing reports a request, its charge goes on
//! the job and a fresh line in data/spend.jsonl, next to the estimate the page showed before the run. fal reports a
//! request some minutes after it ends, so finished jobs from the last week (failed ones for a day: fal rarely bills
//! them) are looked up again until they are found: a light check every ten minutes while any are waiting, and
//! whenever the Spending panel asks. Without the key nothing here runs. Test mode never asks fal.

use std::collections::HashMap;
use std::time::Duration;

use axum::extract::State;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::{json, Value};

use crate::jobs::{now_unix, Charge, Job};
use crate::playground::{fal_get, Fetch};
use crate::state::AppState;

pub const KEY: &str = "FAL_BILLING_KEY";
/// fal looks back at most 90 days per request; a job not billed within a week is not looked up again.
const LOOK_BACK: u64 = 7 * 86_400;
const FAILED_LOOK_BACK: u64 = 86_400;
/// fal accepts at most 50 request ids per lookup; at most this many jobs are asked about at once.
const BATCH: usize = 50;
const MAX_ASKED: usize = 500;
/// Pages of billing rows read for one lookup; past this nothing from it is recorded, rather than a part of a charge.
const MAX_PAGES: usize = 10;
const EVERY: Duration = Duration::from_secs(600);

/// `YYYY-MM-DD` (UTC) for a unix time, as fal's date filters take it.
pub fn date(unix: u64) -> String {
    // Days since 1970-01-01 to a civil date (Howard Hinnant's algorithm).
    let z = (unix / 86_400) as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

/// The charges in fal's billing rows: request id and cost in nano dollars, rows for the same request added up. A
/// row whose id isn't fal's shape or whose cost is missing or negative is left out.
pub fn charges(rows: &[Value]) -> Vec<Charge> {
    let mut out: Vec<Charge> = Vec::new();
    let mut at: HashMap<String, usize> = HashMap::new();
    for r in rows {
        let Some(request_id) = r.get("request_id").and_then(Value::as_str).and_then(crate::jobs::fal_request_id) else {
            continue;
        };
        let nano = match r.get("cost_estimate_nano_usd").and_then(Value::as_f64) {
            Some(n) if n.is_finite() && n >= 0.0 => n.round() as u64,
            _ => match r.get("cost_total").and_then(Value::as_f64) {
                Some(usd) if usd.is_finite() && usd >= 0.0 => (usd * 1e9).round() as u64,
                _ => continue,
            },
        };
        match at.get(&request_id) {
            Some(&k) => out[k].nano = out[k].nano.saturating_add(nano),
            None => {
                at.insert(request_id.clone(), out.len());
                out.push(Charge { request_id, nano });
            }
        }
    }
    out
}

/// Every billing row fal has for these request ids, all pages. Err when fal couldn't be asked, refused the key, or
/// had more than `MAX_PAGES` pages.
async fn billed(st: &AppState, key: &str, ids: &[String], start: &str) -> Result<Vec<Value>, &'static str> {
    let mut rows = Vec::new();
    let mut cursor: Option<String> = None;
    for _ in 0..MAX_PAGES {
        let mut query = vec![("request_id", ids.join(",")), ("start", start.to_string()), ("limit", "200".to_string())];
        if let Some(c) = &cursor {
            query.push(("cursor", c.clone()));
        }
        let page = match fal_get(&st.ctx, Some(key), "/models/billing-events", &query).await {
            Fetch::Ok(page) => page,
            Fetch::Busy => return Err("fal_busy"),
            Fetch::Refused => return Err("key_refused"),
            Fetch::NotFound | Fetch::Failed => return Err("fal_billing_unavailable"),
        };
        rows.extend(page.get("billing_events").and_then(Value::as_array).into_iter().flatten().cloned());
        if page.get("has_more").and_then(Value::as_bool) != Some(true) {
            return Ok(rows);
        }
        cursor = page.get("next_cursor").and_then(Value::as_str).filter(|c| !c.is_empty() && c.len() <= 512).map(str::to_string);
        if cursor.is_none() {
            break;
        }
    }
    Err("fal_billing_unavailable")
}

/// Looks up the charges of finished fal jobs not yet charged (newest first, 50 per lookup, at most 500) and records
/// the ones fal has billed. Returns how many jobs got their charge, and the first error if fal couldn't be asked: a
/// batch that fails doesn't stop the ones after it, except when fal is busy or refuses the key.
pub async fn reconcile(st: &AppState) -> (usize, Option<&'static str>) {
    if st.fake.is_some() {
        return (0, None);
    }
    let Some(key) = st.keys.get(KEY) else { return (0, None) };
    let now = now_unix();
    let pending = st.jobs.uncharged(now.saturating_sub(LOOK_BACK), now.saturating_sub(FAILED_LOOK_BACK), MAX_ASKED);
    let mut found = 0;
    let mut first_error = None;
    for batch in pending.chunks(BATCH) {
        let Some(oldest) = batch.iter().map(|(_, _, at)| *at).min() else { continue };
        let ids: Vec<String> = batch.iter().map(|(_, rid, _)| rid.clone()).collect();
        match billed(st, &key, &ids, &date(oldest.saturating_sub(86_400))).await {
            Ok(rows) => found += st.jobs.record_charges(&charges(&rows)),
            Err(e @ ("fal_busy" | "key_refused")) => return (found, Some(e)),
            Err(e) => {
                first_error.get_or_insert(e);
            }
        }
    }
    (found, first_error)
}

/// While the key is set and jobs wait for their charge, looks them up every ten minutes.
pub async fn run_reconciler(st: AppState) {
    // Each kind of failure is logged once, not every ten minutes.
    let mut last: Option<&'static str> = None;
    loop {
        tokio::time::sleep(EVERY).await;
        let now = now_unix();
        let waiting = st.keys.is_set(KEY)
            && !st.jobs.uncharged(now.saturating_sub(LOOK_BACK), now.saturating_sub(FAILED_LOOK_BACK), 1).is_empty();
        if waiting {
            match reconcile(&st).await {
                (_, Some(e)) if last != Some(e) => {
                    eprintln!("billing: {e}");
                    last = Some(e);
                }
                (_, Some(_)) => {}
                (_, None) => last = None,
            }
        }
    }
}

/// The last seven days of paid runs by model: runs, the page's estimates and what fal charged (a failed run only
/// when fal charged for it). `billable`: runs fal can bill (fal jobs with fal's request id); `uncharged`: those fal
/// hasn't billed yet. A Playground run carries its fal model's `name`.
pub fn week(jobs: &[Job], now: u64) -> Vec<Value> {
    struct Row { model: String, name: Option<String>, runs: u64, estimate: f64, unestimated: u64, charged_nano: u64, billable: u64, uncharged: u64 }
    let mut rows: Vec<Row> = Vec::new();
    let mut at: HashMap<String, usize> = HashMap::new();
    for j in jobs {
        let (Some(c), Some(end)) = (j.cost.as_ref(), j.finished_at) else { continue };
        // A failed run counts once fal has billed it for something.
        let billed_failure = j.status == "failed" && c.charged_nano.is_some_and(|n| n > 0);
        if c.test || end.saturating_add(LOOK_BACK) < now || !(j.status == "done" || billed_failure) {
            continue;
        }
        let i = *at.entry(j.model.clone()).or_insert_with(|| {
            let name = j.model.starts_with("fal:").then(|| j.request.as_ref()?.get("name")?.as_str().map(|n| n.chars().take(120).collect()))
                .flatten();
            rows.push(Row { model: j.model.clone(), name, runs: 0, estimate: 0.0, unestimated: 0, charged_nano: 0, billable: 0, uncharged: 0 });
            rows.len() - 1
        });
        let r = &mut rows[i];
        r.runs += 1;
        match c.estimate_usd {
            Some(e) if e.is_finite() && e >= 0.0 => r.estimate += e,
            _ => r.unestimated += 1,
        }
        if c.request_id.is_some() {
            r.billable += 1;
            match c.charged_nano {
                Some(n) => r.charged_nano = r.charged_nano.saturating_add(n),
                None => r.uncharged += 1,
            }
        }
    }
    rows.sort_by(|a, b| b.charged_nano.cmp(&a.charged_nano).then(b.runs.cmp(&a.runs)).then(a.model.cmp(&b.model)));
    rows.into_iter()
        .map(|r| {
            json!({"model": r.model, "name": r.name, "runs": r.runs, "estimate_usd": r.estimate, "unestimated": r.unestimated,
                   "charged_usd": r.charged_nano as f64 / 1e9, "billable": r.billable, "uncharged": r.uncharged})
        })
        .collect()
}

/// fal's own total for the last 30 days, by endpoint (everything on the account, not only the studio's runs).
pub fn month(page: &Value) -> (f64, Vec<Value>) {
    let mut by: Vec<(String, f64)> = Vec::new();
    let mut at: HashMap<String, usize> = HashMap::new();
    for bucket in page.get("time_series").and_then(Value::as_array).into_iter().flatten() {
        for r in bucket.get("results").and_then(Value::as_array).into_iter().flatten() {
            let (Some(id), Some(cost)) = (r.get("endpoint_id").and_then(Value::as_str), r.get("cost_total").and_then(Value::as_f64)) else {
                continue;
            };
            if !cost.is_finite() || cost < 0.0 || id.len() > 200 {
                continue;
            }
            match at.get(id) {
                Some(&k) => by[k].1 += cost,
                None => {
                    at.insert(id.to_string(), by.len());
                    by.push((id.to_string(), cost));
                }
            }
        }
    }
    by.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    let total = by.iter().map(|(_, c)| c).sum();
    (total, by.into_iter().take(20).map(|(e, c)| json!({"endpoint": e, "usd": c})).collect())
}

/// GET /api/spending: the week from the studio's own jobs (no call to fal).
pub async fn spending(State(st): State<AppState>) -> Response {
    let now = now_unix();
    Json(json!({"billing_key": st.keys.is_set(KEY), "test": st.fake.is_some(), "week": week(&st.jobs.list(), now)})).into_response()
}

/// POST /api/spending/refresh: looks up waiting charges now, then reads fal's own 30-day total. Needs the page's
/// launch token like every change, so no other program on the computer can make the studio call fal.
pub async fn refresh(State(st): State<AppState>) -> Response {
    if st.fake.is_some() || !st.keys.is_set(KEY) {
        return Json(json!({"ok": true, "found": 0, "fal": Value::Null})).into_response();
    }
    let (found, error) = reconcile(&st).await;
    let key = st.keys.get(KEY).unwrap_or_default();
    let query = [("start", date(now_unix().saturating_sub(30 * 86_400))), ("timeframe", "day".to_string())];
    let fal = match fal_get(&st.ctx, Some(&key), "/models/usage", &query).await {
        Fetch::Ok(page) => {
            let (total, by) = month(&page);
            json!({"total_usd": total, "by_endpoint": by})
        }
        _ => Value::Null,
    };
    Json(json!({"ok": error.is_none(), "found": found, "error": error, "fal": fal})).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_are_utc_days() {
        assert_eq!(date(0), "1970-01-01");
        assert_eq!(date(1_792_510_232), "2026-10-20");
        assert_eq!(date(951_782_400), "2000-02-29");
    }

    #[test]
    fn charges_take_fal_exact_figure_and_skip_bad_rows() {
        let page = json!({"billing_events": [
            {"request_id": "abc-123", "cost_total": 0.0035, "cost_estimate_nano_usd": 3_500_000},
            {"request_id": "def-456", "cost_total": 0.56},
            {"request_id": "../etc", "cost_total": 1.0},
            {"request_id": "ghi-789", "cost_total": -1.0},
            {"request_id": "jkl-012"},
            {"request_id": "abc-123", "cost_total": 0.001, "cost_estimate_nano_usd": 1_000_000}
        ]});
        assert_eq!(charges(page["billing_events"].as_array().unwrap()), vec![
            Charge { request_id: "abc-123".into(), nano: 4_500_000 },
            Charge { request_id: "def-456".into(), nano: 560_000_000 },
        ]);
        assert!(charges(&[]).is_empty());
    }

    #[test]
    fn the_week_adds_up_by_model() {
        let now = 1_792_510_232;
        let run = |model: &str, status: &str, est: Option<f64>, charged: Option<u64>, ago: u64, test: bool| Job {
            id: format!("j_{model}_{ago}"),
            model: model.into(),
            status: status.into(),
            finished_at: Some(now - ago),
            mode: "t2i".into(),
            prompt: String::new(),
            progress: None,
            outputs: Vec::new(),
            error: None,
            created_at: now - ago,
            request: model.starts_with("fal:").then(|| json!({"name": "FLUX.1 [schnell]"})),
            cost: Some(crate::jobs::Cost {
                estimate_usd: est,
                charged_nano: charged,
                test,
                request_id: (!model.starts_with("replicate/")).then(|| "r".to_string()),
                ..Default::default()
            }),
        };
        let jobs = vec![
            run("fal/flux-schnell", "done", Some(0.003), Some(3_000_000), 60, false),
            run("fal/flux-schnell", "done", None, None, 120, false),
            run("fal/kling", "done", Some(0.56), Some(560_000_000), 3_600, false),
            run("fal/kling", "failed", Some(0.56), None, 60, false),
            run("fal/kling", "failed", Some(0.56), Some(10_000_000), 60, false),
            run("fal/old", "done", Some(1.0), None, LOOK_BACK + 1, false),
            run("fal/fake", "done", Some(1.0), None, 60, true),
            run("replicate/x", "done", Some(0.01), None, 60, false),
            run("fal:fal-ai/flux/schnell", "done", Some(0.003), None, 60, false),
        ];
        let week = week(&jobs, now);
        assert_eq!(week.len(), 4, "old and test runs are left out: {week:?}");
        assert_eq!(week[0]["model"], "fal/kling");
        assert_eq!(week[0]["runs"], 2, "a failed run counts only when fal charged for it");
        assert!((week[0]["charged_usd"].as_f64().unwrap() - 0.57).abs() < 1e-9);
        assert_eq!((week[1]["runs"].as_u64(), week[1]["unestimated"].as_u64(), week[1]["uncharged"].as_u64()), (Some(2), Some(1), Some(1)));
        let other = week.iter().find(|w| w["model"] == "replicate/x").unwrap();
        assert_eq!((other["billable"].as_u64(), other["uncharged"].as_u64()), (Some(0), Some(0)), "fal can't bill another provider's run");
        let playground = week.iter().find(|w| w["model"] == "fal:fal-ai/flux/schnell").unwrap();
        assert_eq!(playground["name"], "FLUX.1 [schnell]");
        assert_eq!(week[0]["name"], Value::Null);
    }

    #[test]
    fn fal_totals_add_up_by_endpoint() {
        let page = json!({"time_series": [
            {"bucket": "2026-10-19", "results": [{"endpoint_id": "fal-ai/flux/schnell", "cost_total": 0.006}, {"endpoint_id": "fal-ai/kling", "cost_total": 0.84}]},
            {"bucket": "2026-10-20", "results": [{"endpoint_id": "fal-ai/flux/schnell", "cost_total": 0.012}, {"endpoint_id": "x", "cost_total": -3.0}]}
        ]});
        let (total, by) = month(&page);
        assert!((total - 0.858).abs() < 1e-9);
        assert_eq!(by[0]["endpoint"], "fal-ai/kling");
        assert!((by[1]["usd"].as_f64().unwrap() - 0.018).abs() < 1e-9);
    }
}
