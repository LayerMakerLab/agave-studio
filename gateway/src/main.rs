//! Agave Studio gateway. Serves the web app and the JSON API on 127.0.0.1:4170.

mod api;
mod assemble;
mod fsutil;
mod jobs;
mod keys;
mod media;
mod models;
mod pipelines;
mod placeholder;
mod promptcraft;
mod providers;
mod reframe;
mod setup;
mod speech;
mod state;
mod studio;

use std::net::SocketAddr;
use std::path::PathBuf;

/// Local only: the API has no sign-in, so it must never listen on a network address.
const ADDR: &str = "127.0.0.1:4170";

/// `--web <dir>` overrides the default <repo>/web.
fn web_dir_from_args() -> Option<PathBuf> {
    let args: Vec<String> = std::env::args().collect();
    args.iter()
        .position(|a| a == "--web")
        .and_then(|i| args.get(i + 1))
        .map(PathBuf::from)
}

#[tokio::main]
async fn main() {
    let state = match state::AppState::load(web_dir_from_args()) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("startup failed: {e}");
            std::process::exit(1);
        }
    };
    // The data folder (job history, outputs, work files) is owner-only on disk. While the gateway runs, any
    // account on this computer can still reach it on 127.0.0.1 (see SECURITY.md).
    let data_dir = state.ctx.outputs_dir.parent().map(|d| d.to_path_buf());
    for dir in data_dir.iter().chain(std::iter::once(&state.ctx.outputs_dir)) {
        if let Err(e) = fsutil::ensure_private_dir(dir) {
            eprintln!("cannot create {}: {e}", dir.display());
            std::process::exit(1);
        }
    }

    let addr = ADDR;
    let listener = match tokio::net::TcpListener::bind(addr).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("cannot bind {addr}: {e}");
            std::process::exit(1);
        }
    };
    // This copy owns the port now: pick up jobs from the last run, start polling, and clear leftovers.
    state.jobs.restore(&state.registry);
    tokio::spawn(jobs::run_poller(state.clone()));
    tokio::spawn(pipelines::run_ticker(state.clone()));
    fsutil::remove_leftovers(&state.ctx.outputs_dir);
    if let Some(data) = state.paths.jobs_file.parent() {
        fsutil::remove_leftovers(data);
    }
    println!("Agave Studio gateway on http://{addr} (web: {})", state.paths.web.display());
    if state.fake.is_some() {
        println!("TEST MODE (AGAVE_FAKE_PROVIDERS=1): fake providers, no keys, no network; data in {}", state.ctx.outputs_dir.display());
    }

    let app = api::router(state);
    if let Err(e) = axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>()).await {
        eprintln!("server error: {e}");
        std::process::exit(1);
    }
}
