/// Shared utilities and module definitions for S3 request handlers.
/// This file provides common functions for validating authentication and bucket policies.
/// It acts as a central point for coordinating access control before specific handlers are executed.

use std::collections::HashMap;
use axum::http::StatusCode;
use crate::{AppState, SigV4Params, S3Error, S3ErrorCode, S3User};

/// URL percent-encode a string (spaces become %20, other special chars become %XX).
/// Matches the encoding used by minio-go SigV2 canonical resource construction
/// (url.Values.Encode() with '+' replaced by '%20').
fn query_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' |
            b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

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

    // SigV2 presigned URL: AWSAccessKeyId + Signature + Expires
    if query_map.contains_key("AWSAccessKeyId") {
        return validate_auth_sigv2(state, method, uri, &query_map).await;
    }

    // SigV2 header auth: Authorization: AWS access_key:signature
    if let Some(auth_header) = headers.get("authorization").and_then(|h| h.to_str().ok()) {
        if auth_header.starts_with("AWS ") && !auth_header.starts_with("AWS4") {
            return validate_auth_sigv2_header(state, method, uri, query, headers, auth_header).await;
        }
    }

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

async fn validate_auth_sigv2_header(
    state: &AppState,
    method: &str,
    uri: &str,
    query: &[(String, String)],
    headers: &axum::http::HeaderMap,
    auth_header: &str,
) -> Result<(SigV4Params, String), S3Error> {
    // Format: "AWS access_key:signature"
    let rest = &auth_header[4..]; // strip "AWS "
    let (access_key, signature) = rest.split_once(':')
        .ok_or_else(|| S3Error::new(S3ErrorCode::AccessDenied, "Invalid SigV2 Authorization header", StatusCode::FORBIDDEN))?;

    let user: S3User = state.user_meta.get_user(access_key).await?
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

    // Build canonical resource: path + relevant sub-resources from query
    // These sub-resources must be included in the SigV2 canonical resource
    const SIGV2_SUB_RESOURCES: &[&str] = &[
        "acl", "cors", "delete", "lifecycle", "location", "logging",
        "notification", "partNumber", "policy", "requestPayment", "restore",
        "tagging", "torrent", "uploadId", "uploads", "versionId", "versioning",
        "versions", "website",
    ];
    let mut sub_resources: Vec<String> = query.iter()
        .filter(|(k, _)| SIGV2_SUB_RESOURCES.contains(&k.as_str()))
        .map(|(k, v)| if v.is_empty() { k.clone() } else { format!("{}={}", k, v) })
        .collect();
    sub_resources.sort();
    let canonical_resource = if sub_resources.is_empty() {
        uri.to_string()
    } else {
        format!("{}?{}", uri, sub_resources.join("&"))
    };

    let is_valid = state.auth.validate_sigv2_header(method, &canonical_resource, &header_map, signature, &user.secret_key)
        .map_err(|e| S3Error::internal(e))?;

    if !is_valid {
        return Err(S3Error::new(S3ErrorCode::SignatureDoesNotMatch, "SigV2 signature mismatch", StatusCode::FORBIDDEN));
    }

    Ok((SigV4Params {
        access_key: access_key.to_string(),
        credential_scope: String::new(),
        signed_headers: vec![],
        signature: signature.to_string(),
        is_presigned: false,
        expires: None,
        amz_date: None,
    }, "UNSIGNED-PAYLOAD".to_string()))
}

async fn validate_auth_sigv2(
    state: &AppState,
    method: &str,
    uri: &str,
    query_map: &HashMap<String, String>,
) -> Result<(SigV4Params, String), S3Error> {
    let access_key = query_map.get("AWSAccessKeyId")
        .ok_or_else(|| S3Error::new(S3ErrorCode::AccessDenied, "Missing AWSAccessKeyId", StatusCode::FORBIDDEN))?;
    let signature = query_map.get("Signature")
        .ok_or_else(|| S3Error::new(S3ErrorCode::AccessDenied, "Missing Signature", StatusCode::FORBIDDEN))?;
    let expires = query_map.get("Expires")
        .ok_or_else(|| S3Error::new(S3ErrorCode::AccessDenied, "Missing Expires", StatusCode::FORBIDDEN))?;

    let user: S3User = state.user_meta.get_user(access_key).await?
        .ok_or_else(|| S3Error::new(S3ErrorCode::AccessDenied, "Invalid Access Key", StatusCode::FORBIDDEN))?;

    if !user.is_active {
        return Err(S3Error::new(S3ErrorCode::AccessDenied, "User account is inactive", StatusCode::FORBIDDEN));
    }

    // Build canonical resource using the same resource list as minio-go.
    // Values are written as URL-decoded (no re-encoding), matching minio-go's
    // writeCanonicalizedResource which calls url.ParseQuery (decodes) then writes vv[0] directly.
    const SIGV2_RESOURCE_LIST: &[&str] = &[
        "acl", "delete", "lifecycle", "location", "logging", "notification",
        "partNumber", "policy", "requestPayment",
        "response-cache-control", "response-content-disposition",
        "response-content-encoding", "response-content-language",
        "response-content-type", "response-expires",
        "torrent", "uploadId", "uploads", "versionId", "versioning", "versions", "website",
    ];
    let mut parts: Vec<String> = Vec::new();
    for resource in SIGV2_RESOURCE_LIST {
        if let Some(v) = query_map.get(*resource) {
            if v.is_empty() {
                parts.push(resource.to_string());
            } else {
                parts.push(format!("{}={}", resource, v));
            }
        }
    }
    let canonical_resource = if parts.is_empty() {
        uri.to_string()
    } else {
        format!("{}?{}", uri, parts.join("&"))
    };

    let is_valid = state.auth.validate_sigv2_presigned(method, &canonical_resource, expires, signature, &user.secret_key)
        .map_err(|e| S3Error::internal(e))?;

    if !is_valid {
        return Err(S3Error::new(S3ErrorCode::SignatureDoesNotMatch, "SigV2 signature mismatch or expired", StatusCode::FORBIDDEN));
    }

    Ok((SigV4Params {
        access_key: access_key.clone(),
        credential_scope: String::new(),
        signed_headers: vec![],
        signature: signature.clone(),
        is_presigned: true,
        expires: expires.parse::<u64>().ok(),
        amz_date: None,
    }, "UNSIGNED-PAYLOAD".to_string()))
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
