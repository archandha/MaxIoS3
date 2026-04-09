/// Main entry point for the MaxIOs3 server.
/// This file initializes the configuration, logging, storage, and metadata engines.
/// It sets up the Axum router with S3-compatible routes and starts the server.
/// It also spawns a background task for garbage collection of stale multipart uploads.

use std::sync::Arc;

use axum::{
    routing::{get, put, delete, post, head},
    Router,
    http::StatusCode,
    extract::{Path, Query, State},
};
use maxios3::{
    FileSystemStorage, SqliteMetadata, PostgresMetadata,
    BucketMetadataEngine, MultipartMetadataEngine,
    SignatureValidator, AppState, UserMetadataEngine,
    Config, handlers
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = Config::from_env();

    // Initialize structured logging with tracing
    tracing_subscriber::fmt()
        .with_env_filter(&config.log_level)
        .init();

    tracing::info!("Starting S3 server with config: {:?}", config);

    let storage = Arc::new(FileSystemStorage::new(&config.storage_root));

    // Select metadata backend based on the DATABASE_URL scheme.
    // postgres:// or postgresql:// → PostgreSQL; anything else → SQLite.
    let (bucket_meta, multipart_meta, user_meta): (
        Arc<dyn BucketMetadataEngine>,
        Arc<dyn MultipartMetadataEngine>,
        Arc<dyn UserMetadataEngine>,
    ) = if config.database_url.starts_with("postgres://") || config.database_url.starts_with("postgresql://") {
        tracing::info!("Using PostgreSQL metadata backend");
        let meta = Arc::new(PostgresMetadata::new(&config.database_url).await?);
        (meta.clone(), meta.clone(), meta)
    } else {
        tracing::info!("Using SQLite metadata backend");
        let meta = Arc::new(SqliteMetadata::new(&config.database_url).await?);
        (meta.clone(), meta.clone(), meta)
    };

    // Create a default user for development if none exists
    if user_meta.get_user("AKIAIOSFODNN7EXAMPLE").await?.is_none() {
        tracing::info!("Creating default development user...");
        user_meta.create_user(
            "AKIAIOSFODNN7EXAMPLE",
            "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
            "admin"
        ).await?;
    }

    let auth = SignatureValidator::new();

    let state = Arc::new(AppState {
        storage,
        bucket_meta,
        multipart_meta,
        user_meta,
        auth
    });

    // Spawn background Garbage Collection task
    let gc_state = state.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(3600)); // Run every hour
        loop {
            interval.tick().await;
            tracing::info!("Running Multipart Garbage Collection...");

            // Clean up uploads older than 24 hours (86400 seconds)
            match gc_state.multipart_meta.find_stale_multipart_uploads(86400).await {
                Ok(stale_ids) => {
                    for upload_id in stale_ids {
                        tracing::warn!(upload_id = %upload_id, "Cleaning up stale multipart upload");
                        let _ = gc_state.storage.cleanup_multipart(&upload_id).await;
                        let _ = gc_state.multipart_meta.complete_multipart(&upload_id).await;
                    }
                }
                Err(e) => tracing::error!("GC Task failed to find stale uploads: {:?}", e),
            }
        }
    });

    // Define our S3 routes
    let app = Router::new()
        .route("/minio/health/live", get(|| async { StatusCode::OK }))
        .route("/minio/health/ready", get(|| async { StatusCode::OK }))
        .route("/minio/v2/metrics/cluster", get(|| async { "maxios3_metrics 1\n" }))
        .route("/minio/v2/metrics/node", get(|| async { "maxios3_metrics 1\n" }))
        .route("/minio/v2/metrics/bucket", get(|| async { "maxios3_metrics 1\n" }))
        .route("/minio/v2/metrics/resource", get(|| async { "maxios3_metrics 1\n" }))
        .route("/minio/v1/admin/listusers",   get(handlers::admin::list_users))
        .route("/minio/v1/admin/adduser",     put(handlers::admin::add_user))
        .route("/minio/v1/admin/removeuser",  delete(handlers::admin::remove_user))
        .route("/minio/v1/admin/setuserinfo", put(handlers::admin::set_user_status))
        .route("/", get(handlers::bucket::list_buckets))
        .route("/", head(handlers::bucket::list_buckets))
        .route("/{bucket}", get(handlers::bucket::list_objects))
        .route("/{bucket}/", get(handlers::bucket::list_objects))
        .route("/{bucket}", head(handlers::bucket::list_objects))
        .route("/{bucket}/", head(handlers::bucket::list_objects))
        .route("/{bucket}", put(handlers::bucket::create_bucket))
        .route("/{bucket}/", put(handlers::bucket::create_bucket))
        .route("/{bucket}", post(handlers::bucket::bucket_post_handler))
        .route("/{bucket}/", post(handlers::bucket::bucket_post_handler))
        .route("/{bucket}", delete(handlers::bucket::delete_bucket))
        .route("/{bucket}/", delete(handlers::bucket::delete_bucket))
        .route("/{bucket}/{*key}", get(|method: axum::http::Method, uri: axum::http::Uri, path: Path<(String, String)>, query: Query<Vec<(String, String)>>, state: State<Arc<AppState>>, headers: axum::http::HeaderMap| async move {
            tracing::info!(method = %method, uri = %uri, query = ?query, "Route hit: /{{bucket}}/{{*key}}");
            handlers::object::get_object(method, axum::extract::OriginalUri(uri), path, query, state, headers).await
        }))
        .route("/{bucket}/{*key}", head(|method: axum::http::Method, uri: axum::http::Uri, path: Path<(String, String)>, query: Query<Vec<(String, String)>>, state: State<Arc<AppState>>, headers: axum::http::HeaderMap| async move {
            tracing::info!(method = %method, uri = %uri, query = ?query, "Route hit (HEAD): /{{bucket}}/{{*key}}");
            handlers::object::get_object(method, axum::extract::OriginalUri(uri), path, query, state, headers).await
        }))
        .route("/{bucket}/{*key}", put(|method: axum::http::Method, uri: axum::http::Uri, path: Path<(String, String)>, query: Query<Vec<(String, String)>>, state: State<Arc<AppState>>, headers: axum::http::HeaderMap, body: axum::body::Body| async move {
            tracing::info!(method = %method, uri = %uri, query = ?query, "Route hit (PUT): /{{bucket}}/{{*key}}");
            handlers::multipart::put_handler(method, axum::extract::OriginalUri(uri), path, query, state, headers, body).await
        }))
        .route("/{bucket}/{*key}", post(|method: axum::http::Method, uri: axum::http::Uri, path: Path<(String, String)>, query: Query<Vec<(String, String)>>, state: State<Arc<AppState>>, headers: axum::http::HeaderMap, body: axum::body::Body| async move {
            tracing::info!(method = %method, uri = %uri, query = ?query, "Route hit (POST): /{{bucket}}/{{*key}}");
            handlers::multipart::post_handler(method, axum::extract::OriginalUri(uri), path, query, state, headers, body).await
        }))
        .route("/{bucket}/{*key}", delete(|method: axum::http::Method, uri: axum::http::Uri, path: Path<(String, String)>, query: Query<Vec<(String, String)>>, state: State<Arc<AppState>>, headers: axum::http::HeaderMap| async move {
            tracing::info!(method = %method, uri = %uri, query = ?query, "Route hit (DELETE): /{{bucket}}/{{*key}}");
            handlers::object::delete_object(method, axum::extract::OriginalUri(uri), path, query, state, headers).await
        }))
        .layer(tower_http::trace::TraceLayer::new_for_http())
        .with_state(state);

    // Bind to all interfaces on port 3000
    let addr: std::net::SocketAddr = format!("{}:{}", config.host, config.port).parse()?;

    if let (Some(cert_path), Some(key_path)) = (&config.tls_cert, &config.tls_key) {
        tracing::info!("Server running on https://{} (TLS enabled)", addr);
        let tls_config = axum_server::tls_rustls::RustlsConfig::from_pem_file(cert_path, key_path).await?;
        axum_server::bind_rustls(addr, tls_config)
            .serve(app.into_make_service())
            .await?;
    } else {
        tracing::info!("Server running on http://{}", addr);
        axum_server::bind(addr)
            .serve(app.into_make_service())
            .await?;
    }

    Ok(())
}
