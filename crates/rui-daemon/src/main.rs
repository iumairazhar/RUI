//! RUI daemon entry point: HTTP API, MCP server, scheduler and chat loop.

use rui_daemon::{api, apps, chat, cli_version, db::Db, deploy, live, mcp, owner_auth, roster, scheduler, seat, skills, state};
use std::sync::Arc;
use std::time::Duration;

fn env_num(name: &str, default: u64) -> u64 {
    std::env::var(name).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

#[tokio::main]
async fn main() {
    let port = env_num("RUI_PORT", 7878) as u16;
    let tick = Duration::from_secs(env_num("RUI_TICK_SECS", 5));
    if let Err(e) = std::fs::create_dir_all(state::data_dir()) {
        eprintln!("create data dir: {e}");
        std::process::exit(1);
    }
    cli_version::init();
    let db = match Db::open(&state::db_path()) {
        Ok(db) => Arc::new(db),
        Err(e) => {
            eprintln!("{e:#}");
            std::process::exit(1);
        }
    };
    match scheduler::recover_on_start(&db) {
        Ok(n) if n > 0 => println!("recovered {n} task(s)"),
        Ok(_) => {}
        Err(e) => eprintln!("recovery failed: {e:#}"),
    }
    match deploy::recover(&db) {
        Ok(n) if n > 0 => println!("closed {n} deploy(s) left running"),
        Ok(_) => {}
        Err(e) => eprintln!("deploy recovery failed: {e:#}"),
    }
    let data_dir = state::data_dir();
    let cfg = scheduler::Config { port, tick, data_dir: data_dir.clone() };
    let (chat_tx, chat_rx) = tokio::sync::mpsc::unbounded_channel();
    let seat_cache = seat::SeatCache::default();
    let runs = scheduler::Runs::default();
    let client_locks = scheduler::ClientLocks::default();
    let live = live::Live::default();
    let apps = apps::Apps::default();
    let owner_token: Arc<str> = match owner_auth::issue(&data_dir) {
        Ok(t) => Arc::from(t),
        Err(e) => {
            eprintln!("cannot write owner token: {e:#}");
            std::process::exit(1);
        }
    };
    skills::seed(&data_dir);
    roster::seed(&data_dir);
    let state = api::ApiState { db: db.clone(), chat_tx, seat: seat_cache.clone(), runs: runs.clone(), live: live.clone(), data_dir: data_dir.clone(), apps: apps.clone(), owner_token };
    let app = api::router(state).merge(mcp::mcp_router(db.clone(), data_dir, apps.clone(), runs.clone()));
    let listener = match tokio::net::TcpListener::bind(("127.0.0.1", port)).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("cannot bind 127.0.0.1:{port}: {e}");
            std::process::exit(1);
        }
    };
    tokio::spawn(seat::refresh_loop(seat_cache));
    tokio::spawn(rui_daemon::seats::probe_loop(db.clone()));
    tokio::spawn(scheduler::run(db.clone(), cfg.clone(), runs, client_locks, live.clone(), apps.clone()));
    tokio::spawn(chat::run(db, chat_rx, cfg, live));
    println!("rui-daemon listening on 127.0.0.1:{port}");
    let served = axum::serve(listener, app).with_graceful_shutdown(shutdown_signal()).await;
    apps::stop_all(&apps);
    if let Err(e) = served {
        eprintln!("server error: {e}");
        std::process::exit(1);
    }
}

/// Resolves on Ctrl-C or SIGTERM.
async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c().await.ok();
    };
    let Ok(mut term) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) else {
        return ctrl_c.await;
    };
    tokio::select! {
        _ = ctrl_c => {}
        _ = term.recv() => {}
    }
}
