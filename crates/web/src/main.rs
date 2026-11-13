//! Entry point. One binary, two modes:
//!
//! - In AWS Lambda (`AWS_LAMBDA_RUNTIME_API` is set) it hands the router to `lambda_http`.
//! - Otherwise it serves HTTP on 127.0.0.1:3000.

use std::process::ExitCode;
use std::sync::Arc;

use choosey_store::{DynamoStore, MemoryStore, Store};
use choosey_web::app::{AppState, router};
use choosey_web::config::{Config, StoreKind};
use tracing_subscriber::EnvFilter;

const LOCAL_ADDR: &str = "127.0.0.1:3000";

#[tokio::main]
async fn main() -> ExitCode {
    let in_lambda = std::env::var_os("AWS_LAMBDA_RUNTIME_API").is_some();
    init_tracing(in_lambda);

    let config = match Config::from_env(in_lambda) {
        Ok(c) => c,
        Err(e) => {
            // Fail closed: e.g. never start in Lambda without the origin secret.
            tracing::error!("invalid configuration: {e}");
            return ExitCode::FAILURE;
        }
    };
    tracing::info!(?config, in_lambda, "starting {}", choosey_core::APP_NAME);

    let store: Arc<dyn Store> = match &config.store {
        StoreKind::Memory => Arc::new(MemoryStore::new()),
        // Region and credentials come from the standard AWS environment. Nothing is called at
        // startup: the Lambda role only allows GetItem, PutItem and DeleteItem.
        StoreKind::Dynamo { table_name } => {
            Arc::new(DynamoStore::from_env(table_name.clone()).await)
        }
    };

    let app = router(AppState {
        store,
        config: Arc::new(config),
    });

    let result = if in_lambda {
        lambda_http::run(app).await.map_err(|e| e.to_string())
    } else {
        serve_locally(app).await.map_err(|e| e.to_string())
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            tracing::error!("server error: {e}");
            ExitCode::FAILURE
        }
    }
}

async fn serve_locally(app: axum::Router) -> std::io::Result<()> {
    let listener = tokio::net::TcpListener::bind(LOCAL_ADDR).await?;
    tracing::info!("listening on http://{LOCAL_ADDR}");
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await
}

/// Logs go to stdout (CloudWatch in Lambda). `RUST_LOG` overrides the default filter.
///
/// The default keeps the Lambda runtime and HTTP libraries at `warn`: at debug level they can
/// log request details, including paths that contain edit secrets.
fn init_tracing(in_lambda: bool) {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        EnvFilter::new("info,lambda_runtime=warn,lambda_http=warn,hyper=warn,aws_config=warn")
    });
    let fmt = tracing_subscriber::fmt().with_env_filter(filter);
    if in_lambda {
        // CloudWatch adds its own timestamps and doesn't render colours.
        fmt.with_ansi(false).without_time().init();
    } else {
        fmt.init();
    }
}
