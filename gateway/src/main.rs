//! Agave Studio gateway. Serves the web app and the JSON API on 127.0.0.1:4170 (test mode: AGAVE_TEST_PORT).

mod api;
mod assemble;
mod billing;
mod boards;
mod fsutil;
mod jobs;
mod keys;
mod media;
mod models;
mod moves;
mod pipelines;
mod placeholder;
mod playground;
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
    let mut state = match state::AppState::load(web_dir_from_args()) {
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

    // Test mode can run beside a real studio: AGAVE_TEST_PORT moves it to another local port. A real studio ignores it.
    let test_port = std::env::var("AGAVE_TEST_PORT").ok();
    let owned;
    let addr = match (test_port, state.fake.is_some()) {
        (Some(p), true) => match p.parse::<u16>() {
            Ok(port) if port >= 1024 => {
                state.use_port(port);
                owned = format!("127.0.0.1:{port}");
                owned.as_str()
            }
            _ => {
                eprintln!("AGAVE_TEST_PORT must be a port number from 1024 up");
                std::process::exit(1);
            }
        },
        (Some(_), false) => {
            eprintln!("AGAVE_TEST_PORT is ignored outside test mode (AGAVE_FAKE_PROVIDERS=1)");
            ADDR
        }
        (None, _) => ADDR,
    };
    let listener = match tokio::net::TcpListener::bind(addr).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("cannot bind {addr}: {e}");
            if e.kind() == std::io::ErrorKind::AddrInUse {
                eprintln!("Another program is using {addr}, often Agave Studio already running (the Mac app or another terminal). Open http://{addr}/app/, or quit the other copy and start this one again.");
            }
            std::process::exit(1);
        }
    };
    // This copy owns the port now: pick up jobs from the last run, start polling, and clear leftovers.
    state.jobs.restore(&state.registry);
    tokio::spawn(jobs::run_poller(state.clone()));
    tokio::spawn(pipelines::run_ticker(state.clone()));
    tokio::spawn(billing::run_reconciler(state.clone()));
    fsutil::remove_leftovers(&state.ctx.outputs_dir);
    if let Some(data) = state.paths.jobs_file.parent() {
        fsutil::remove_leftovers(data);
    }
    fsutil::remove_leftovers(&state.paths.boards_dir);
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
