/// Request handlers for S3 object-level operations.
/// This file contains Axum handlers for retrieving (GET) and deleting (DELETE) individual objects.
/// It ensures that requests are authenticated and authorized via bucket policies
/// before interacting with the underlying storage and metadata engines.

use std::sync::Arc;
use std::str::FromStr;
use std::collections::HashMap;
use axum::{
    extract::{Path, State, Query},
    http::{StatusCode, HeaderMap, HeaderName, HeaderValue},
    response::IntoResponse,
};
use crate::{AppState, S3Error, S3ErrorCode};
use super::validate_auth;

/// Handler for GET Object requests
pub async fn get_object(
    method: axum::http::Method,
    axum::extract::OriginalUri(uri): axum::extract::OriginalUri,
    Path((bucket, key)): Path<(String, String)>,
    Query(all_query): Query<Vec<(String, String)>>,
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
) -> Result<impl IntoResponse, S3Error> {
    tracing::info!(bucket = %bucket, key = %key, query = ?all_query, "GET Object");

    if all_query.iter().any(|(k, _)| k == "uploadId") {
        return crate::handlers::multipart::list_parts_handler(method, axum::extract::OriginalUri(uri), Path((bucket, key)), Query(all_query), State(state), headers).await.map(|r| r.into_response());
    }

    // 1. Authentication
    let (params, _) = validate_auth(
        &state,
        method.as_str(),
        uri.path(),
        &all_query,
        &headers,
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855", // Empty body hash for GET
    ).await?;

    super::validate_policy(&state, &params, method.as_str(), &bucket, Some(&key)).await?;

    if !state.storage.exists(&bucket, &key).await? {
        return Err(S3Error::new(
            S3ErrorCode::NoSuchKey,
            "The specified key does not exist.",
            StatusCode::NOT_FOUND
        ));
    }

    let body: axum::body::Body = state.storage.get(&bucket, &key).await
        .map_err(|e| S3Error::internal(e))?;

    let meta = state.bucket_meta.get_object(&bucket, &key).await?
        .ok_or_else(|| S3Error::new(S3ErrorCode::NoSuchKey, "Object metadata not found", StatusCode::NOT_FOUND))?;

    tracing::info!(bucket = %bucket, key = %key, size = meta.size, checksum = %meta.checksum, "GET Object metadata");

    let etag = meta.checksum.trim_matches('"').to_string();

    let quoted_etag = format!("\"{}\"", etag);

    tracing::info!(bucket = %bucket, key = %key, etag = %quoted_etag, "GET Object ETag");

    let mut response_headers = HeaderMap::new();
    response_headers.insert(axum::http::header::ETAG, quoted_etag.parse().unwrap());
    response_headers.insert(axum::http::header::CONTENT_TYPE, meta.content_type.parse().unwrap_or_else(|_| "application/octet-stream".parse().unwrap()));
    response_headers.insert(axum::http::header::CONTENT_LENGTH, meta.size.to_string().parse().unwrap());
    if let Some(sc) = &meta.storage_class {
        if sc != "STANDARD" {
            if let Ok(v) = sc.parse() {
                response_headers.insert(
                    axum::http::header::HeaderName::from_static("x-amz-storage-class"),
                    v,
                );
            }
        }
    }

    if let Some(metadata_json) = meta.metadata {
        if let Ok(metadata) = serde_json::from_str::<HashMap<String, String>>(&metadata_json) {
            for (key, value) in metadata {
                if let Ok(header_name) = HeaderName::from_str(&key) {
                    if let Ok(header_value) = HeaderValue::from_str(&value) {
                        response_headers.insert(header_name, header_value);
                    }
                }
            }
        }
    }

    Ok((
        StatusCode::OK,
        response_headers,
        body,
    ).into_response())
}

/// Handler for DELETE Object requests
pub async fn delete_object(
    method: axum::http::Method,
    axum::extract::OriginalUri(uri): axum::extract::OriginalUri,
    Path((bucket, key)): Path<(String, String)>,
    Query(all_query): Query<Vec<(String, String)>>,
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
) -> Result<impl IntoResponse, S3Error> {
    tracing::info!(bucket = %bucket, key = %key, "DELETE Object");

    let (params, _) = validate_auth(
        &state,
        method.as_str(),
        uri.path(),
        &all_query,
        &headers,
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
    ).await?;

    super::validate_policy(&state, &params, method.as_str(), &bucket, Some(&key)).await?;

    state.storage.delete(&bucket, &key).await?;
    state.bucket_meta.delete_object(&bucket, &key).await?;

    Ok(StatusCode::NO_CONTENT)
}
