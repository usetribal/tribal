use std::env;

use lineage_mcp::server;
use std::path::PathBuf;

use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .with_target(false)
        .init();

    let repo = env::var("LINEAGE_REPO")
        .map(PathBuf::from)
        .unwrap_or_else(|_| env::current_dir().expect("cwd"));

    if let Err(e) = server::run_stdio(&repo).await {
        eprintln!("lineage-mcp error: {e}");
        std::process::exit(1);
    }
}
