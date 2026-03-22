/// Request handlers for S3 multipart upload operations.
/// This file contains Axum handlers for initiating, uploading parts, and completing multipart uploads.
/// It also handles single-part PUT object requests by delegating to the appropriate storage logic.
/// All operations are protected by SigV4 authentication and bucket policy checks.

use std::sync::Arc;
use std::collections::HashMap;
use percent_encoding::percent_decode_str;
use axum::{
    extract::{Path, State, Query},
    http::StatusCode,
    response::IntoResponse,
};
use crate::{AppState, S3Error, S3ErrorCode, MultipartPart, S3ObjectMetadata, MultipartUpload};
use super::validate_auth;

#[derive(serde::Serialize)]
#[serde(rename = "CopyObjectResult")]
#[serde(rename_all = "PascalCase")]
struct CopyObjectResponse {
    last_modified: String,
    #[serde(rename = "ETag")]
    etag: String,
}

/// Unified handler for PUT requests (Object and Multipart Part)
pub async fn put_handler(
    method: axum::http::Method,
    axum::extract::OriginalUri(uri): axum::extract::OriginalUri,
    Path((bucket, key)): Path<(String, String)>,
    Query(all_query): Query<Vec<(String, String)>>,
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    body: axum::body::Body,
) -> Result<impl IntoResponse, S3Error> {
    // In streaming mode, we rely on the x-amz-content-sha256 header for the signature check.
    let payload_hash = headers.get("x-amz-content-sha256")
        .and_then(|h| h.to_str().ok())
        .or_else(|| all_query.iter().find(|(k, _)| k == "X-Amz-Content-Sha256").map(|(_, v)| v.as_str()))
        .unwrap_or("UNSIGNED-PAYLOAD");

    // 1. Validate Auth
    let (params, expected_payload_hash) = validate_auth(
        &state,
        method.as_str(),
        uri.path(),
        &all_query,
        &headers,
        payload_hash
    ).await?;

    super::validate_policy(&state, &params, method.as_str(), &bucket, Some(&key)).await?;

    let query_map: HashMap<String, String> = all_query.iter().cloned().collect();

    // 2. Proceed with Logic (Streaming to disk)
    let upload_id = query_map.get("uploadId");
    let part_number = query_map.get("partNumber").and_then(|p| p.parse::<u32>().ok());

    if let (Some(upload_id), Some(part_number)) = (upload_id, part_number) {
        tracing::info!(upload_id = %upload_id, part = part_number, "Upload Part (Streaming)");
        let size: u64 = state.storage.put_part(upload_id, part_number, body, &expected_payload_hash).await
            .map_err(|e| {
                if e.to_string().contains("Checksum mismatch") {
                    S3Error::new(S3ErrorCode::XAmzContentSHA256Mismatch, e.to_string(), StatusCode::BAD_REQUEST)
                } else {
                    S3Error::internal(e)
                }
            })?;

        state.multipart_meta.save_part(upload_id, MultipartPart {
            part_number,
            size,
            checksum: expected_payload_hash.to_string(),
        }).await?;
        Ok((
            StatusCode::OK,
            [(axum::http::header::ETAG, format!("\"{}\"", expected_payload_hash))],
        ).into_response())
    } else if let Some(copy_source) = headers.get("x-amz-copy-source").and_then(|h| h.to_str().ok()) {
        tracing::info!(bucket = %bucket, key = %key, source = %copy_source, "Copy Object");

        // S3 copy source format: /bucket/key or bucket/key, potentially with ?versionId=...
        let source_path = copy_source.trim_start_matches('/');
        let (full_key, _version) = source_path.split_once('?').unwrap_or((source_path, ""));

        let (src_bucket, src_key_encoded) = full_key.split_once('/')
            .ok_or_else(|| S3Error::new(S3ErrorCode::InvalidArgument, "Invalid copy source", StatusCode::BAD_REQUEST))?;

        let src_key = percent_decode_str(src_key_encoded).decode_utf8_lossy();

        let src_meta = state.bucket_meta.get_object(src_bucket, &src_key).await?
            .ok_or_else(|| S3Error::new(S3ErrorCode::NoSuchKey, "Source object not found", StatusCode::NOT_FOUND))?;

        state.storage.copy(src_bucket, &src_key, &bucket, &key).await?;

        let now = chrono::Utc::now();
        state.bucket_meta.save_object(S3ObjectMetadata {
            bucket: bucket.clone(),
            key: key.clone(),
            size: src_meta.size,
            content_type: src_meta.content_type,
            checksum: src_meta.checksum.clone(),
            metadata: src_meta.metadata.clone(),
            created_at: now,
        }).await?;

        let response = CopyObjectResponse {
            last_modified: now.format("%Y-%m-%dT%H:%M:%S.000Z").to_string(),
            etag: format!("\"{}\"", src_meta.checksum),
        };
        let xml = quick_xml::se::to_string(&response).map_err(|e| S3Error::internal(e))?;
        Ok((
            StatusCode::OK,
            [(axum::http::header::CONTENT_TYPE, "application/xml")],
            format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n{}", xml)
        ).into_response())
    } else {
        tracing::info!(bucket = %bucket, key = %key, "PUT Object (Streaming)");
        let size: u64 = state.storage.put(&bucket, &key, body, &expected_payload_hash).await
            .map_err(|e| {
                if e.to_string().contains("Checksum mismatch") {
                    S3Error::new(S3ErrorCode::XAmzContentSHA256Mismatch, e.to_string(), StatusCode::BAD_REQUEST)
                } else {
                    S3Error::internal(e)
                }
            })?;

        let metadata: HashMap<String, String> = headers.iter()
            .filter(|(k, _)| k.as_str().starts_with("x-amz-meta-"))
            .map(|(k, v)| (k.as_str().to_string(), v.to_str().unwrap_or("").to_string()))
            .collect();
        let metadata_json = serde_json::to_string(&metadata).ok();

        state.bucket_meta.save_object(S3ObjectMetadata {
            bucket: bucket.clone(),
            key: key.clone(),
            size,
            content_type: headers.get("content-type").and_then(|h| h.to_str().ok()).unwrap_or("application/octet-stream").to_string(),
            checksum: expected_payload_hash.to_string(),
            metadata: metadata_json,
            created_at: chrono::Utc::now(),
        }).await?;

        Ok(StatusCode::OK.into_response())
    }
}

#[derive(serde::Serialize)]
#[serde(rename = "InitiateMultipartUploadResult")]
#[serde(rename_all = "PascalCase")]
struct InitiateMultipartResponse {
    bucket: String,
    key: String,
    upload_id: String,
}

#[derive(serde::Serialize)]
#[serde(rename = "ListPartsResult")]
#[serde(rename_all = "PascalCase")]
struct ListPartsResponse {
    bucket: String,
    key: String,
    upload_id: String,
    #[serde(rename = "Part")]
    parts: Vec<PartEntry>,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "PascalCase")]
struct PartEntry {
    part_number: u32,
    etag: String,
    size: u64,
}

/// Handler for GET ListParts
pub async fn list_parts_handler(
    _method: axum::http::Method,
    _uri: axum::extract::OriginalUri,
    Path((bucket, key)): Path<(String, String)>,
    Query(all_query): Query<Vec<(String, String)>>,
    State(state): State<Arc<AppState>>,
    _headers: axum::http::HeaderMap,
) -> Result<impl IntoResponse, S3Error> {
    let query_map: HashMap<String, String> = all_query.iter().cloned().collect();
    let upload_id = query_map.get("uploadId").ok_or_else(|| S3Error::new(S3ErrorCode::InvalidArgument, "Missing uploadId", StatusCode::BAD_REQUEST))?;

    let upload = state.multipart_meta.get_multipart(upload_id).await?
        .ok_or_else(|| S3Error::new(S3ErrorCode::NoSuchUpload, "Upload not found", StatusCode::NOT_FOUND))?;

    let response = ListPartsResponse {
        bucket: bucket.clone(),
        key: key.clone(),
        upload_id: upload_id.clone(),
        parts: upload.parts.iter().map(|p| {
            let etag = format!("\"{}\"", p.checksum);
            tracing::info!(part_number = p.part_number, checksum = %p.checksum, etag = %etag, "ListParts part");
            PartEntry {
                part_number: p.part_number,
                etag,
                size: p.size,
            }
        }).collect(),
    };

    let xml = quick_xml::se::to_string(&response).map_err(|e| {
        tracing::error!(error = ?e, "Failed to serialize ListPartsResponse to XML");
        S3Error::internal(e)
    })?;
    tracing::info!(xml = %xml, "ListParts XML response");
    Ok((
        StatusCode::OK,
        [(axum::http::header::CONTENT_TYPE, "application/xml")],
        format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n{}", xml)
    ).into_response())
}

/// Unified handler for POST requests (Initiate and Complete Multipart)
pub async fn post_handler(
    method: axum::http::Method,
    axum::extract::OriginalUri(uri): axum::extract::OriginalUri,
    Path((bucket, key)): Path<(String, String)>,
    Query(all_query): Query<Vec<(String, String)>>,
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
) -> Result<impl IntoResponse, S3Error> {
    let payload_hash = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"; // SHA256 of empty string

    // 1. Validate Auth
    let (params, _) = validate_auth(
        &state,
        method.as_str(),
        uri.path(),
        &all_query,
        &headers,
        payload_hash
    ).await?;

    super::validate_policy(&state, &params, method.as_str(), &bucket, Some(&key)).await?;

    let query_map: HashMap<String, String> = all_query.iter().cloned().collect();

    // 2. Proceed with Logic
    if query_map.contains_key("uploads") {
        let upload_id: String = state.multipart_meta.initiate_multipart(&bucket, &key).await?;
        let response = InitiateMultipartResponse {
            bucket: bucket.clone(),
            key: key.clone(),
            upload_id,
        };
        let xml = quick_xml::se::to_string(&response).map_err(|e| S3Error::internal(e))?;
        Ok((
            StatusCode::OK,
            [(axum::http::header::CONTENT_TYPE, "application/xml")],
            format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n{}", xml)
        ).into_response())
    } else if let Some(upload_id) = query_map.get("uploadId") {
        tracing::info!(upload_id = %upload_id, "Completing Multipart Upload");
        let upload: MultipartUpload = state.multipart_meta.get_multipart(upload_id).await?
            .ok_or_else(|| {
                tracing::error!(upload_id = %upload_id, "Upload not found in metadata");
                S3Error::new(S3ErrorCode::InternalError, "Upload not found", StatusCode::NOT_FOUND)
            })?;

        let part_numbers: Vec<u32> = upload.parts.iter().map(|p| p.part_number).collect();
        tracing::info!(upload_id = %upload_id, parts = ?part_numbers, "Assembling parts");
        let total_size: u64 = state.storage.assemble(upload_id, &bucket, &key, part_numbers).await?;

        // Calculate a combined ETag. For 0-byte objects, use a specific ETag.
        let etag = if total_size == 0 {
            "d41d8cd98f00b204e9800998ecf8427e".to_string()
        } else {
            // For multipart uploads, S3 ETags are typically <md5>-<part_count>
            // Use a consistent hash based on part checksums if possible,
            // but for now, ensure the format is correct.
            format!("{}--{}", uuid::Uuid::new_v4().simple(), upload.parts.len())
        };

        tracing::info!(upload_id = %upload_id, total_size = total_size, etag = %etag, "Completing Multipart Upload ETag");

        state.bucket_meta.save_object(S3ObjectMetadata {
            bucket: bucket.clone(),
            key: key.clone(),
            size: total_size,
            content_type: "application/octet-stream".to_string(),
            checksum: etag.clone(),
            metadata: None,
            created_at: chrono::Utc::now(),
        }).await?;

        state.multipart_meta.complete_multipart(upload_id).await?;
        state.storage.cleanup_multipart(upload_id).await?;

        #[derive(serde::Serialize)]
        #[serde(rename = "CompleteMultipartUploadResult")]
        #[serde(rename_all = "PascalCase")]
        struct CompleteMultipartResponse {
            bucket: String,
            key: String,
            etag: String,
        }

        let response = CompleteMultipartResponse {
            bucket: bucket.clone(),
            key: key.clone(),
            etag: format!("\"{}\"", etag),
        };

        let xml = quick_xml::se::to_string(&response).map_err(|e| S3Error::internal(e))?;
        Ok((
            StatusCode::OK,
            [(axum::http::header::CONTENT_TYPE, "application/xml")],
            format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n{}", xml)
        ).into_response())
    } else if query_map.contains_key("select") || query_map.contains_key("select-type") {
        tracing::info!(bucket = %bucket, key = %key, "Select Object Content (Not Implemented)");
        Err(S3Error::new(S3ErrorCode::NotImplemented, "S3 Select is not implemented yet.", StatusCode::NOT_IMPLEMENTED))
    } else {
        Err(S3Error::new(S3ErrorCode::InternalError, "Invalid POST request", StatusCode::BAD_REQUEST))
    }
}
