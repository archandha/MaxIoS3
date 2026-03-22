/// Shared utilities and module definitions for S3 request handlers.
/// This file provides common functions for validating authentication and bucket policies.
/// It acts as a central point for coordinating access control before specific handlers are executed.

use std::collections::HashMap;
use axum::http::StatusCode;
use crate::{AppState, SigV4Params, S3Error, S3ErrorCode, S3User};

pub mod bucket;
pub mod object;
pub mod multipart;

pub async fn validate_auth(
    state: &AppState,
    method: &str,
    uri: &str,
    query: &[(String, String)],
    headers: &axum::http::HeaderMap,
    payload_hash: &str,
) -> Result<(SigV4Params, String), S3Error> {
    let query_map: HashMap<String, String> = query.iter().cloned().collect();

    let params = if let Some(auth_header) = headers.get("authorization").and_then(|h| h.to_str().ok()) {
        SigV4Params::parse(auth_header)
            .map_err(|e| S3Error::new(S3ErrorCode::AccessDenied, format!("Invalid Authorization header: {}", e), StatusCode::FORBIDDEN))?
    } else if query_map.contains_key("X-Amz-Algorithm") {
        SigV4Params::parse_query(&query_map)
            .map_err(|e| S3Error::new(S3ErrorCode::AccessDenied, format!("Invalid Pre-signed URL: {}", e), StatusCode::FORBIDDEN))?
    } else {
        return Err(S3Error::new(S3ErrorCode::AccessDenied, "Missing authentication", StatusCode::FORBIDDEN));
    };

    // Look up user secret key
    let user: S3User = state.user_meta.get_user(&params.access_key).await?
        .ok_or_else(|| S3Error::new(S3ErrorCode::AccessDenied, "Invalid Access Key", StatusCode::FORBIDDEN))?;

    if !user.is_active {
        return Err(S3Error::new(S3ErrorCode::AccessDenied, "User account is inactive", StatusCode::FORBIDDEN));
    }

    let mut header_map = HashMap::new();
    for name in headers.keys() {
        let hn = name.as_str().to_lowercase();
        let values: Vec<_> = headers.get_all(name).iter()
            .map(|v| v.to_str().unwrap_or("").trim())
            .collect();
        header_map.insert(hn, values.join(","));
    }

    let auth_payload_hash = if let Some(h) = headers.get("x-amz-content-sha256") {
        h.to_str().unwrap_or(payload_hash)
    } else if let Some(h) = query_map.get("X-Amz-Content-Sha256") {
        h.as_str()
    } else if params.is_presigned {
        "UNSIGNED-PAYLOAD"
    } else {
        payload_hash
    };

    tracing::debug!(method = %method, uri = %uri, payload_hash = %auth_payload_hash, "Validating Auth");

    let is_valid = state.auth.validate(method, uri, query, &header_map, &params, auth_payload_hash, &user.secret_key)
        .map_err(|e| S3Error::internal(e))?;

    if !is_valid {
        return Err(S3Error::new(S3ErrorCode::SignatureDoesNotMatch, "Signature mismatch or expired", StatusCode::FORBIDDEN));
    }

    Ok((params, auth_payload_hash.to_string()))
}

pub async fn validate_policy(
    state: &AppState,
    params: &SigV4Params,
    method: &str,
    bucket: &str,
    key: Option<&str>,
) -> Result<(), S3Error> {
    let policy = state.bucket_meta.get_bucket_policy(bucket).await?;
    if let Some(policy) = policy {
        let principal_arn = format!("arn:aws:iam:::user/{}", params.access_key);
        let action = match (method, key) {
            ("GET", None) | ("HEAD", None) => "s3:ListBucket",
            ("GET", Some(_)) | ("HEAD", Some(_)) => "s3:GetObject",
            ("PUT", None) => "s3:CreateBucket",
            ("PUT", Some(_)) => "s3:PutObject",
            ("DELETE", None) => "s3:DeleteBucket",
            ("DELETE", Some(_)) => "s3:DeleteObject",
            ("POST", Some(_)) => "s3:PutObject",
            _ => "s3:Unknown",
        };

        let resource_arn = match key {
            Some(k) => format!("arn:aws:s3:::{}/{}", bucket, k),
            None => format!("arn:aws:s3:::{}", bucket),
        };

        let result = crate::auth::policy::PolicyEvaluator::evaluate(
            &policy,
            &principal_arn,
            action,
            &resource_arn,
        );

        if result == crate::auth::policy::PolicyResult::Deny {
            return Err(S3Error::new(S3ErrorCode::AccessDenied, "Access denied by bucket policy", StatusCode::FORBIDDEN));
        }

        // In S3, if a policy exists, it must explicitly Allow the action if it's not the owner.
        // For simplicity, we'll just enforce Deny for now if explicitly denied.
    }
    Ok(())
}
