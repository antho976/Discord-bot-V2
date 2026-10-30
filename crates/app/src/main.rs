//! Starts the bot: database, features, dashboard, gateway and the minute timer, all in one
//! process. `bot help` lists the maintenance commands.

use std::sync::Arc;

use anyhow::Result;
use bot_core::{Config, Ctx, Db, Feature};

fn features() -> Vec<Arc<dyn Feature>> {
    vec![]
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,twilight_gateway=warn,twilight_http_ratelimiting=warn".into()),
        )
        .with_target(false)
        .init();
    rustls::crypto::ring::default_provider()
        .install_default()
        .map_err(|_| anyhow::anyhow!("couldn't install the TLS provider"))?;

    // Two worker threads are plenty for two servers and keep memory low.
    let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build()?;
    rt.block_on(run())
}

async fn run() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        None | Some("run") => serve().await,
        Some("preview") => preview(args.get(1).and_then(|p| p.parse().ok()).unwrap_or(3099)).await,
        Some("help") | Some("--help") | Some("-h") => {
            println!("{HELP}");
            Ok(())
        }
        Some(other) => anyhow::bail!("unknown command {other:?}. Try `bot help`."),
    }
}

const HELP: &str = "bot                 run the bot and dashboard
bot preview [port]  dashboard with sample servers and no Discord, for local design work
bot help            show this";

async fn serve() -> Result<()> {
    let cfg = Config::from_env()?;
    let db = Db::open(&cfg.database_path)?;
    let ctx = Ctx::new(cfg, db)?;
    ctx.install_features(features())?;
    for f in ctx.features() {
        if let Err(e) = f.start(&ctx).await {
            tracing::warn!(feature = f.meta().key, error = %e, "feature didn't start cleanly");
        }
    }

    let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
    let web = tokio::spawn(bot_core::web::serve(ctx.clone(), stop_rx.clone()));
    let timer = tokio::spawn(bot_core::scheduler::run(ctx.clone(), stop_rx.clone()));
    let gateway = tokio::spawn(bot_core::discord::run(ctx.clone(), stop_rx));

    shutdown_signal().await;
    tracing::info!("shutting down");
    let _ = stop_tx.send(true);
    let _ = gateway.await;
    let _ = timer.await;
    let _ = web.await;
    Ok(())
}

async fn preview(port: u16) -> Result<()> {
    let cfg = Config::preview(port);
    let db = Db::open(&cfg.database_path)?;
    let ctx = Ctx::new(cfg, db)?;
    ctx.install_features(features())?;
    bot_core::preview::seed(&ctx)?;
    println!("Preview: http://localhost:{port}/dev/login");
    let (_stop_tx, stop_rx) = tokio::sync::watch::channel(false);
    bot_core::web::serve(ctx, stop_rx).await
}

async fn shutdown_signal() {
    let ctrl_c = tokio::signal::ctrl_c();
    #[cfg(unix)]
    {
        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).expect("signal handler");
        tokio::select! {
            _ = ctrl_c => {}
            _ = term.recv() => {}
        }
    }
    #[cfg(not(unix))]
    let _ = ctrl_c.await;
}
