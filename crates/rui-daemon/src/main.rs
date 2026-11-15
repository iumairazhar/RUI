//! RUI daemon entry point: HTTP API, MCP server, scheduler and chat loop.

use rui_daemon::{api, chat, db::Db, mcp, scheduler, state};
use std::sync::Arc;
use std::time::Duration;

fn env_num(name: &str, default: u64) -> u64 {
    std::env::var(name).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

#[tokio::main]
async fn main() {
    let port = env_num("RUI_PORT", 7878) as u16;
    let tick = Duration::from_secs(env_num("RUI_TICK_SECS", 5));
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
    let cfg = scheduler::Config { port, tick, data_dir: state::data_dir() };
    let (chat_tx, chat_rx) = tokio::sync::mpsc::unbounded_channel();
    let app = api::router(api::ApiState { db: db.clone(), chat_tx }).merge(mcp::mcp_router(db.clone()));
    let listener = match tokio::net::TcpListener::bind(("127.0.0.1", port)).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("cannot bind 127.0.0.1:{port}: {e}");
            std::process::exit(1);
        }
    };
    tokio::spawn(scheduler::run(db.clone(), cfg.clone()));
    tokio::spawn(chat::run(db, chat_rx, cfg));
    println!("rui-daemon listening on 127.0.0.1:{port}");
    if let Err(e) = axum::serve(listener, app).await {
        eprintln!("server error: {e}");
        std::process::exit(1);
    }
}
