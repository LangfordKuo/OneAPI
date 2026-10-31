mod admin;
mod app;
mod backup;
mod client_ip;
mod config;
mod db;
mod error;
mod events;
mod gateway;
mod maintenance;
mod metrics;
mod models;
mod protocol;
mod recovery;
mod security;
mod state;
mod telemetry;
mod update;
mod upstream;
mod web;

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;
use clap::{Parser, Subcommand};
use config::Config;
use state::AppState;
use zeroize::Zeroizing;

#[derive(Debug, Parser)]
#[command(name = "oneapi", version, about = "Self-hosted AI API gateway")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    /// Data directory (overrides ONEAPI_DATA_DIR)
    #[arg(long, global = true)]
    data_dir: Option<PathBuf>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Start the HTTP server
    Serve,
    /// Reset the administrator password
    ResetPassword {
        /// New password; read from stdin when omitted
        #[arg(long)]
        password: Option<String>,
    },
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let data_dir = cli.data_dir.clone();
    let command = cli.command.unwrap_or(Command::Serve);

    // `env::set_var` mutates process-global state and is not thread-safe, so
    // the .env files must be loaded before the runtime spawns worker threads.
    let data_dir = config::load_env_before_startup(data_dir)?;

    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("failed to build the async runtime")?
        .block_on(async move {
            match command {
                Command::Serve => serve(data_dir).await,
                Command::ResetPassword { password } => reset_password(data_dir, password).await,
            }
        })
}

async fn serve(data_dir: PathBuf) -> anyhow::Result<()> {
    let config = Config::from_env(data_dir)?;
    telemetry::init(&config.log_level)?;

    let db = db::connect(&config).await?;
    let crypto = Arc::new(security::Crypto::load_or_create(&config.data_dir)?);
    admin::bootstrap(&db).await?;
    // Precompute the constant-cost dummy hash before any login can reach it.
    let _ = security::dummy_password_hash();

    let events = Arc::new(events::EventBus::default());
    let usage_writer = Arc::new(gateway::usage::UsageWriter::start(
        db.clone(),
        events.clone(),
    ));
    let state = AppState {
        config: Arc::new(config.clone()),
        db,
        crypto,
        live: Arc::new(metrics::LiveMetrics::default()),
        events,
        updates: std::sync::Arc::new(crate::state::UpdateState::default()),
        login_limiter: std::sync::Arc::new(crate::admin::LoginLimiter::default()),
        auth_failure_limiter: std::sync::Arc::new(crate::state::AuthFailureLimiter::default()),
        clients: Arc::new(upstream::ClientCache::default()),
        cache: Arc::new(state::RuntimeCache::default()),
        probes: Arc::new(state::ChannelProbes::default()),
        usage_writer: Some(usage_writer),
        started_at: chrono::Utc::now(),
        backup_lock: Arc::new(tokio::sync::Mutex::new(())),
        last_backup_at: Arc::new(std::sync::Mutex::new(None)),
        response_store_lock: Arc::new(tokio::sync::Mutex::new(())),
        response_store_rejected: Arc::new(std::sync::atomic::AtomicU64::new(0)),
    };

    maintenance::spawn(state.clone());
    recovery::spawn(state.clone());

    let updates = state.updates.clone();
    let shutdown_updates = updates.clone();
    let usage_writer_shutdown = state.usage_writer.clone();
    let app = app::router(state);
    let listener = tokio::net::TcpListener::bind(config.addr)
        .await
        .with_context(|| format!("failed to bind {}", config.addr))?;

    tracing::info!(
        addr = %config.addr,
        version = env!("CARGO_PKG_VERSION"),
        data_dir = %config.data_dir.display(),
        "OneAPI listening"
    );

    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(async move {
        tokio::select! {
            _ = shutdown_signal() => {},
            _ = shutdown_updates.shutdown.notified() => {},
        }
    })
    .await
    .context("HTTP server failed")?;

    // Flush queued usage rows before a restart replaces the process.
    if let Some(writer) = usage_writer_shutdown {
        writer.shutdown().await;
    }

    if updates
        .restart_requested
        .load(std::sync::atomic::Ordering::SeqCst)
    {
        let executable = updates
            .restart_exe
            .lock()
            .ok()
            .and_then(|slot| slot.clone());
        restart_process(executable)?;
    }

    Ok(())
}

#[cfg(unix)]
fn restart_process(exe: Option<std::path::PathBuf>) -> anyhow::Result<()> {
    use std::os::unix::process::CommandExt;

    let exe = match exe {
        Some(path) => path,
        None => std::env::current_exe().context("failed to locate current executable")?,
    };
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let exe_display = exe.display().to_string();
    tracing::info!(executable = %exe_display, "restarting OneAPI after update");
    let error = std::process::Command::new(exe).args(args).exec();
    Err(anyhow::anyhow!(
        "failed to restart OneAPI from {exe_display}: {error}"
    ))
}

#[cfg(not(unix))]
fn restart_process(exe: Option<std::path::PathBuf>) -> anyhow::Result<()> {
    let exe = match exe {
        Some(path) => path,
        None => std::env::current_exe().context("failed to locate current executable")?,
    };
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let exe_display = exe.display().to_string();
    tracing::info!(executable = %exe_display, "restarting OneAPI after update");
    std::process::Command::new(exe)
        .args(args)
        .spawn()
        .with_context(|| format!("failed to restart OneAPI from {exe_display}"))?;
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        if let Err(error) = tokio::signal::ctrl_c().await {
            tracing::error!(error = %error, "failed to listen for Ctrl+C");
        }
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                let _ = signal.recv().await;
            }
            Err(error) => {
                // Keep waiting on Ctrl+C instead of shutting down eagerly.
                tracing::error!(error = %error, "failed to install SIGTERM handler");
                std::future::pending::<()>().await;
            }
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }

    tracing::info!("shutdown signal received");
}

async fn reset_password(data_dir: PathBuf, password: Option<String>) -> anyhow::Result<()> {
    let config = Config::from_env(data_dir)?;
    let db = db::connect(&config).await?;

    let admin = models::admin::get(&db).await?.ok_or_else(|| {
        anyhow::anyhow!("no admin account found; start the server and complete setup first")
    })?;

    let password = match password {
        Some(password) => Zeroizing::new(password),
        None => {
            use std::io::Write;
            eprint!("New password: ");
            std::io::stderr().flush().ok();
            let mut line = Zeroizing::new(String::new());
            std::io::stdin().read_line(&mut line)?;
            // Trim in place so the buffer stays zeroized instead of leaving an
            // extra plaintext copy behind.
            let trimmed_len = line.trim_end_matches(['\r', '\n']).len();
            line.truncate(trimmed_len);
            line
        }
    };

    if let Some(violation) = security::password_violation(password.as_str()) {
        anyhow::bail!("password {violation}");
    }

    let password_hash = security::hash_password(password.as_str())?;
    models::admin::update_password(&db, admin.id, &password_hash).await?;
    models::session::delete_all_for_admin(&db, admin.id).await?;

    println!(
        "Password updated for admin `{}`; all sessions were signed out.",
        admin.username
    );

    Ok(())
}
