//! KiwiManga entry point: config -> logging -> DB+migrate -> state -> commands ->
//! workers -> long-polling dispatcher -> graceful shutdown.

use kiwimanga::config::Config;
use kiwimanga::db;
use kiwimanga::error::Result;
use kiwimanga::http::HttpClient;
use kiwimanga::i18n::I18n;
use kiwimanga::queue;
use kiwimanga::state::AppState;
use kiwimanga::worker;
use std::time::Duration;
use teloxide::prelude::*;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    let cfg = Config::from_env()?;
    init_logging(&cfg.log_level);
    tracing::info!("kiwimanga starting (workers={})", cfg.workers);

    std::fs::create_dir_all(&cfg.data_dir)?;
    std::fs::create_dir_all(format!("{}/tmp", cfg.data_dir))?;
    std::fs::create_dir_all(format!("{}/cache", cfg.data_dir))?;

    let pool = db::connect(&cfg.database_url).await?;
    let resumed = queue::reset_running_to_pending(&pool).await?;
    if resumed > 0 {
        tracing::warn!("startup recovery: {resumed} job(s) running -> pending");
    }

    let i18n = I18n::load()?;
    let http = HttpClient::new(
        Duration::from_secs(cfg.http_timeout_secs),
        cfg.http_max_attempts,
    )?;
    let (shutdown_tx, _) = tokio::sync::watch::channel(false);
    let state = AppState::new(cfg.clone(), pool.clone(), i18n, http, shutdown_tx.clone());

    let bot = Bot::new(cfg.bot_token.clone());
    if let Err(e) = kiwimanga::bot::register_commands(&bot, &state.i18n).await {
        tracing::warn!("set_my_commands failed (bot still works): {e}");
    }

    let handles = worker::spawn_workers(state.clone(), cfg.workers);

    // Long-polling dispatcher (no ports, no webhook).
    let handler = teloxide::dptree::entry()
        .branch(Update::filter_message().endpoint(kiwimanga::bot::handlers::on_message))
        .branch(Update::filter_callback_query().endpoint(kiwimanga::bot::handlers::on_callback));
    let mut dispatcher = teloxide::dispatching::Dispatcher::builder(bot, handler)
        .dependencies(teloxide::dptree::deps![state.clone()])
        .enable_ctrlc_handler()
        .build();

    tokio::select! {
        _ = dispatcher.dispatch() => {
            tracing::info!("dispatcher stopped (ctrl-c)");
        }
        _ = wait_terminate() => {
            tracing::info!("terminate signal received");
        }
    }

    // Graceful shutdown: workers finish the current chapter, then requeue.
    let _ = shutdown_tx.send(true);
    let drained = tokio::time::timeout(Duration::from_secs(180), async {
        for h in handles {
            let _ = h.await;
        }
    })
    .await;
    if drained.is_err() {
        tracing::warn!("workers did not drain in time; running jobs will resume on boot");
    }
    let stuck = queue::reset_running_to_pending(&pool).await?;
    if stuck > 0 {
        tracing::warn!("shutdown recovery: {stuck} job(s) running -> pending");
    }
    pool.close().await;
    tracing::info!("kiwimanga stopped");
    Ok(())
}

fn init_logging(level: &str) {
    if std::env::var("RUST_LOG").is_err() {
        std::env::set_var("RUST_LOG", format!("kiwimanga={level},teloxide=warn"));
    }
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .with_target(false)
        .init();
}

/// First of SIGTERM (Unix) / Ctrl-C. Dispatcher itself stops on Ctrl-C via
/// teloxide's ctrlc_handler; this branch exists so SIGTERM (Railway/Docker
/// stop) also triggers the graceful path instead of a hard kill.
async fn wait_terminate() {
    #[cfg(unix)]
    {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut sig) => {
                tokio::select! {
                    _ = sig.recv() => {}
                    _ = tokio::signal::ctrl_c() => {}
                }
                return;
            }
            Err(e) => tracing::debug!("cannot watch SIGTERM: {e}"),
        }
    }
    let _ = tokio::signal::ctrl_c().await;
}
