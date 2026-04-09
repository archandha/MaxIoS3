// Handlers for the minimal user management admin API.
// All endpoints are protected by SigV4 authentication and require the caller
// to be a user whose `user_id` is "admin".
//
// Endpoints:
//   GET    /minio/v1/admin/listusers                         — list all users
//   PUT    /minio/v1/admin/adduser?accessKey=X               — create / replace a user
//   DELETE /minio/v1/admin/removeuser?accessKey=X            — delete a user
//   PUT    /minio/v1/admin/setuserinfo?accessKey=X&status=.. — enable or disable a user

use std::sync::Arc;
use std::collections::HashMap;
use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::IntoResponse,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use crate::{AppState, S3Error, S3ErrorCode};
use super::validate_auth;

// ── Response type ─────────────────────────────────────────────────────────────

#[derive(Serialize)]
struct UserInfo {
    #[serde(rename = "secretKey")]
    secret_key: String,
    status: &'static str,
    #[serde(rename = "userId")]
    user_id: String,
    #[serde(rename = "createdAt")]
    created_at: DateTime<Utc>,
}

// ── Request body for add_user ─────────────────────────────────────────────────

#[derive(Deserialize)]
struct AddUserRequest {
    #[serde(rename = "secretKey")]
    secret_key: String,
    #[serde(rename = "userId")]
    user_id: String,
}

// ── Admin auth guard ──────────────────────────────────────────────────────────

/// Validates that the request is signed by a user with user_id == "admin".
async fn require_admin(
    state: &AppState,
    method: &str,
    uri: &str,
    query: &[(String, String)],
    headers: &axum::http::HeaderMap,
) -> Result<(), S3Error> {
    let (params, _) = validate_auth(state, method, uri, query, headers, "UNSIGNED-PAYLOAD").await?;
    let user = state.user_meta.get_user(&params.access_key).await
        .map_err(|e| S3Error::internal(e))?
        .ok_or_else(|| S3Error::new(S3ErrorCode::AccessDenied, "User not found", StatusCode::FORBIDDEN))?;
    if user.user_id != "admin" {
        return Err(S3Error::new(S3ErrorCode::AccessDenied, "Admin access required", StatusCode::FORBIDDEN));
    }
    Ok(())
}

// ── Handlers ──────────────────────────────────────────────────────────────────

/// GET /minio/v1/admin/listusers
pub async fn list_users(
    method: axum::http::Method,
    axum::extract::OriginalUri(uri): axum::extract::OriginalUri,
    Query(all_query): Query<Vec<(String, String)>>,
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
) -> Result<impl IntoResponse, S3Error> {
    require_admin(&state, method.as_str(), uri.path(), &all_query, &headers).await?;

    let users = state.user_meta.list_users().await.map_err(|e| S3Error::internal(e))?;
    let map: HashMap<String, UserInfo> = users.into_iter().map(|u| {
        let status = if u.is_active { "enabled" } else { "disabled" };
        let info = UserInfo {
            secret_key: u.secret_key,
            status,
            user_id: u.user_id,
            created_at: u.created_at,
        };
        (u.access_key, info)
    }).collect();

    Ok(axum::Json(map))
}

/// PUT /minio/v1/admin/adduser?accessKey=X
/// Body: {"secretKey": "...", "userId": "..."}
pub async fn add_user(
    method: axum::http::Method,
    axum::extract::OriginalUri(uri): axum::extract::OriginalUri,
    Query(all_query): Query<Vec<(String, String)>>,
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    body: axum::body::Body,
) -> Result<impl IntoResponse, S3Error> {
    require_admin(&state, method.as_str(), uri.path(), &all_query, &headers).await?;

    let access_key = all_query.iter()
        .find(|(k, _)| k == "accessKey")
        .map(|(_, v)| v.clone())
        .ok_or_else(|| S3Error::new(S3ErrorCode::InvalidArgument, "Missing accessKey query parameter", StatusCode::BAD_REQUEST))?;

    let body_bytes = axum::body::to_bytes(body, usize::MAX).await
        .map_err(|e| S3Error::internal(e))?;
    let req: AddUserRequest = serde_json::from_slice(&body_bytes)
        .map_err(|e| S3Error::new(S3ErrorCode::InvalidArgument, format!("Invalid JSON body: {}", e), StatusCode::BAD_REQUEST))?;

    state.user_meta.create_user(&access_key, &req.secret_key, &req.user_id).await
        .map_err(|e| S3Error::internal(e))?;

    Ok(StatusCode::OK)
}

/// DELETE /minio/v1/admin/removeuser?accessKey=X
pub async fn remove_user(
    method: axum::http::Method,
    axum::extract::OriginalUri(uri): axum::extract::OriginalUri,
    Query(all_query): Query<Vec<(String, String)>>,
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
) -> Result<impl IntoResponse, S3Error> {
    require_admin(&state, method.as_str(), uri.path(), &all_query, &headers).await?;

    let access_key = all_query.iter()
        .find(|(k, _)| k == "accessKey")
        .map(|(_, v)| v.clone())
        .ok_or_else(|| S3Error::new(S3ErrorCode::InvalidArgument, "Missing accessKey query parameter", StatusCode::BAD_REQUEST))?;

    state.user_meta.delete_user(&access_key).await
        .map_err(|e| S3Error::internal(e))?;

    Ok(StatusCode::NO_CONTENT)
}

/// PUT /minio/v1/admin/setuserinfo?accessKey=X&status=enabled|disabled
pub async fn set_user_status(
    method: axum::http::Method,
    axum::extract::OriginalUri(uri): axum::extract::OriginalUri,
    Query(all_query): Query<Vec<(String, String)>>,
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
) -> Result<impl IntoResponse, S3Error> {
    require_admin(&state, method.as_str(), uri.path(), &all_query, &headers).await?;

    let access_key = all_query.iter()
        .find(|(k, _)| k == "accessKey")
        .map(|(_, v)| v.clone())
        .ok_or_else(|| S3Error::new(S3ErrorCode::InvalidArgument, "Missing accessKey query parameter", StatusCode::BAD_REQUEST))?;

    let status = all_query.iter()
        .find(|(k, _)| k == "status")
        .map(|(_, v)| v.clone())
        .ok_or_else(|| S3Error::new(S3ErrorCode::InvalidArgument, "Missing status query parameter", StatusCode::BAD_REQUEST))?;

    let is_active = match status.as_str() {
        "enabled"  => true,
        "disabled" => false,
        other => return Err(S3Error::new(
            S3ErrorCode::InvalidArgument,
            format!("Invalid status '{}': must be 'enabled' or 'disabled'", other),
            StatusCode::BAD_REQUEST,
        )),
    };

    state.user_meta.set_user_active(&access_key, is_active).await
        .map_err(|e| S3Error::internal(e))?;

    Ok(StatusCode::OK)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    // ── AddUserRequest deserialization ────────────────────────────────────────

    #[test]
    fn test_add_user_request_deserializes_camel_case() {
        let json = r#"{"secretKey":"s3cr3t","userId":"alice"}"#;
        let req: AddUserRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.secret_key, "s3cr3t");
        assert_eq!(req.user_id, "alice");
    }

    #[test]
    fn test_add_user_request_missing_secret_key_fails() {
        let json = r#"{"userId":"alice"}"#;
        assert!(serde_json::from_str::<AddUserRequest>(json).is_err());
    }

    #[test]
    fn test_add_user_request_missing_user_id_fails() {
        let json = r#"{"secretKey":"s3cr3t"}"#;
        assert!(serde_json::from_str::<AddUserRequest>(json).is_err());
    }

    // ── UserInfo serialization ────────────────────────────────────────────────

    #[test]
    fn test_user_info_serializes_field_names() {
        let info = UserInfo {
            secret_key: "s3cr3t".into(),
            status: "enabled",
            user_id: "alice".into(),
            created_at: Utc::now(),
        };
        let json = serde_json::to_value(&info).unwrap();
        assert!(json.get("secretKey").is_some(), "expected secretKey");
        assert!(json.get("userId").is_some(),    "expected userId");
        assert!(json.get("createdAt").is_some(), "expected createdAt");
        assert!(json.get("secret_key").is_none(), "unexpected snake_case field");
    }

    #[test]
    fn test_user_info_status_enabled() {
        let info = UserInfo { secret_key: "x".into(), status: "enabled", user_id: "u".into(), created_at: Utc::now() };
        let json = serde_json::to_value(&info).unwrap();
        assert_eq!(json["status"], "enabled");
    }

    #[test]
    fn test_user_info_status_disabled() {
        let info = UserInfo { secret_key: "x".into(), status: "disabled", user_id: "u".into(), created_at: Utc::now() };
        let json = serde_json::to_value(&info).unwrap();
        assert_eq!(json["status"], "disabled");
    }
}
