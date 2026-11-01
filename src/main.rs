use std::path::PathBuf;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let db_path =
        PathBuf::from(std::env::var("SOUS_DATABASE").unwrap_or_else(|_| "sous.db".into()));
    let addr = std::env::var("SOUS_ADDR").unwrap_or_else(|_| "127.0.0.1:3000".into());

    match std::env::args().nth(1).as_deref() {
        None => {}
        Some("migrate") => {
            sous::db::migrate(&db_path).await?;
            tracing::info!(db = %db_path.display(), "migrations applied");
            return Ok(());
        }
        Some(other) => {
            eprintln!("unknown command: {other}\nusage: sous [migrate]");
            std::process::exit(2);
        }
    }

    let tz = sous::calendar::parse_timezone(std::env::var("APP_TIMEZONE").ok().as_deref())?;
    sous::db::migrate(&db_path).await?;
    let pool = sous::db::open_read_pool(&db_path).await?;
    let mcp = sous::McpConfig {
        write_pool: sous::db::open_write_pool(&db_path).await?,
        allowed_hosts: sous::mcp::parse_list(
            std::env::var("SOUS_MCP_ALLOWED_HOSTS").ok().as_deref(),
        ),
        allowed_origins: sous::mcp::parse_list(
            std::env::var("SOUS_MCP_ALLOWED_ORIGINS").ok().as_deref(),
        ),
    };
    let shutdown = Shutdown::new()?;
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    tracing::info!(%addr, "listening");
    let clock = sous::calendar::Clock::system(tz);
    axum::serve(listener, sous::app_with_mcp(pool, clock, mcp))
        .with_graceful_shutdown(shutdown.wait())
        .await?;
    tracing::info!("stopped");
    Ok(())
}

/// Signal handlers, registered before binding so no signal after `listening` is missed.
#[cfg(unix)]
struct Shutdown {
    sigint: tokio::signal::unix::Signal,
    sigterm: tokio::signal::unix::Signal,
}

#[cfg(unix)]
impl Shutdown {
    fn new() -> std::io::Result<Self> {
        use tokio::signal::unix::{SignalKind, signal};
        Ok(Self {
            sigint: signal(SignalKind::interrupt())?,
            sigterm: signal(SignalKind::terminate())?,
        })
    }

    async fn wait(mut self) {
        let name = tokio::select! {
            _ = self.sigint.recv() => "SIGINT",
            _ = self.sigterm.recv() => "SIGTERM",
        };
        tracing::info!(signal = name, "shutting down");
    }
}

#[cfg(not(unix))]
struct Shutdown;

#[cfg(not(unix))]
impl Shutdown {
    fn new() -> std::io::Result<Self> {
        Ok(Self)
    }

    async fn wait(self) {
        let _ = tokio::signal::ctrl_c().await;
        tracing::info!(signal = "ctrl_c", "shutting down");
    }
}
