//! SSR dashboard for the IoT sensing platform (topcoat + htmx + ECharts).

mod config;
mod db;
mod echarts;
mod range;
mod util;
mod web;

use tokio::net::TcpListener;

use crate::config::Config;
use crate::db::Db;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "dashboard=info,topcoat=info".into()),
        )
        .init();

    let config = Config::from_env();
    tracing::info!(
        listen_addr = %config.listen_addr,
        clickhouse_url = %config.clickhouse_url,
        clickhouse_database = %config.clickhouse_database,
        "starting dashboard"
    );

    let db = Db::new(&config);
    let router = web::router(db);

    let listener = TcpListener::bind(&config.listen_addr)
        .await
        .unwrap_or_else(|error| panic!("failed to bind {}: {error}", config.listen_addr));

    if let Err(error) = topcoat::serve(listener, router).await {
        tracing::error!(%error, "server exited with error");
        std::process::exit(1);
    }
}
