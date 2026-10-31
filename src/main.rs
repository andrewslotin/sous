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
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    tracing::info!(%addr, "listening");
    let clock = sous::calendar::Clock::system(tz);
    axum::serve(listener, sous::app_with_mcp(pool, clock, mcp))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}
