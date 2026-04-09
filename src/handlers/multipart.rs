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

/// Validate an x-amz-checksum-* value against the provided data.
/// Returns BadDigest if the checksum doesn't match.
fn validate_amz_checksum(algo: &str, data: &[u8], expected_b64: &str) -> Result<(), S3Error> {
    use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
    let computed_b64 = match algo {
        "crc32" => {
            let crc = crc32fast::hash(data);
            BASE64.encode(crc.to_be_bytes())
        }
        "crc32c" => {
            let crc = crc32c::crc32c(data);
            BASE64.encode(crc.to_be_bytes())
        }
        "sha1" => {
            use sha1::{Sha1, Digest};
            BASE64.encode(Sha1::digest(data))
        }
        "sha256" => {
            use sha2::{Sha256, Digest};
            BASE64.encode(Sha256::digest(data))
        }
        _ => return Ok(()),
    };
    if computed_b64 != expected_b64 {
        return Err(S3Error::new(
            S3ErrorCode::BadDigest,
            format!("The {} you specified did not match what we received.", algo.to_uppercase()),
            StatusCode::BAD_REQUEST,
        ));
    }
    Ok(())
}

/// Decode AWS SigV4 streaming chunked body format.
/// Each chunk: `{hex-size};chunk-signature={sig}\r\n{data}\r\n`
/// Final chunk: `0;chunk-signature={sig}\r\n\r\n`
fn decode_aws_chunked(data: &[u8]) -> anyhow::Result<Vec<u8>> {
    let mut result = Vec::new();
    let mut pos = 0;
    while pos < data.len() {
        let header_end = data[pos..].windows(2)
            .position(|w| w == b"\r\n")
            .ok_or_else(|| anyhow::anyhow!("Invalid AWS chunked header"))?;
        let header_bytes = &data[pos..pos + header_end];
        let header_str = std::str::from_utf8(header_bytes)?;
        let size_str = header_str.split(';').next().unwrap_or("").trim();
        let chunk_size = usize::from_str_radix(size_str, 16)?;
        pos += header_end + 2;
        if chunk_size == 0 {
            break;
        }
        result.extend_from_slice(&data[pos..pos + chunk_size]);
        pos += chunk_size + 2; // skip data + trailing \r\n
    }
    Ok(result)
}

#[derive(serde::Serialize)]
#[serde(rename = "CopyObjectResult")]
#[serde(rename_all = "PascalCase")]
struct CopyObjectResponse {
    last_modified: String,
    #[serde(rename = "ETag")]
    etag: String,
}

#[derive(serde::Serialize)]
#[serde(rename = "CopyPartResult")]
#[serde(rename_all = "PascalCase")]
struct CopyPartResponse {
    last_modified: String,
    #[serde(rename = "ETag")]
    etag: String,
}

/// Handler for PUT /bucket/key?tagging — set object tags
async fn put_object_tagging(
    method: axum::http::Method,
    uri: axum::http::Uri,
    bucket: String,
    key: String,
    all_query: Vec<(String, String)>,
    state: std::sync::Arc<AppState>,
    headers: axum::http::HeaderMap,
    body: axum::body::Body,
) -> Result<impl IntoResponse, S3Error> {
    let (params, _) = validate_auth(
        &state, method.as_str(), &uri.path(), &all_query, &headers,
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
    ).await?;
    super::validate_policy(&state, &params, method.as_str(), &bucket, Some(&key)).await?;

    let body_bytes = axum::body::to_bytes(body, usize::MAX).await.map_err(|e| S3Error::internal(e))?;
    let body_str = std::str::from_utf8(&body_bytes).map_err(|e| S3Error::internal(e))?;

    // Parse <Tagging><TagSet><Tag><Key>k</Key><Value>v</Value></Tag>...</TagSet></Tagging>
    let mut tag_pairs: Vec<(String, String)> = Vec::new();
    let mut key_buf = String::new();
    let mut val_buf = String::new();
    let mut in_key = false;
    let mut in_val = false;
    let mut reader = quick_xml::Reader::from_str(body_str);
    reader.config_mut().trim_text(true);
    loop {
        match reader.read_event() {
            Ok(quick_xml::events::Event::Start(e)) => match e.name().as_ref() {
                b"Key" => { in_key = true; key_buf.clear(); }
                b"Value" => { in_val = true; val_buf.clear(); }
                _ => {}
            },
            Ok(quick_xml::events::Event::Text(e)) => {
                let text = e.unescape().unwrap_or_default().to_string();
                if in_key { key_buf.push_str(&text); }
                if in_val { val_buf.push_str(&text); }
            }
            Ok(quick_xml::events::Event::End(e)) => match e.name().as_ref() {
                b"Key" => { in_key = false; }
                b"Value" => { in_val = false; }
                b"Tag" => { tag_pairs.push((key_buf.clone(), val_buf.clone())); }
                _ => {}
            },
            Ok(quick_xml::events::Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
    }

    // Validate: max 10 tags
    if tag_pairs.len() > 10 {
        return Err(S3Error::new(
            S3ErrorCode::BadRequest,
            "Object tags cannot be greater than 10",
            StatusCode::BAD_REQUEST,
        ));
    }

    // Validate: no duplicate keys, key length <= 128, value length <= 256
    let mut seen_keys = std::collections::HashSet::new();
    for (k, v) in &tag_pairs {
        if k.len() > 128 {
            return Err(S3Error::new(
                S3ErrorCode::InvalidTag,
                "The TagKey you have provided is invalid",
                StatusCode::BAD_REQUEST,
            ));
        }
        if v.len() > 256 {
            return Err(S3Error::new(
                S3ErrorCode::InvalidTag,
                "The TagValue you have provided is invalid",
                StatusCode::BAD_REQUEST,
            ));
        }
        if !seen_keys.insert(k.as_str()) {
            return Err(S3Error::new(
                S3ErrorCode::InvalidTag,
                "Cannot provide multiple Tags with the same key",
                StatusCode::BAD_REQUEST,
            ));
        }
    }

    let tags: HashMap<String, String> = tag_pairs.into_iter().collect();

    // Get existing metadata and update _s3_tags
    let mut meta = state.bucket_meta.get_object(&bucket, &key).await?
        .ok_or_else(|| S3Error::new(S3ErrorCode::NoSuchKey, "Object not found", StatusCode::NOT_FOUND))?;

    let mut meta_map: HashMap<String, String> = meta.metadata
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_default();

    let tags_json = serde_json::to_string(&tags).map_err(|e| S3Error::internal(e))?;
    meta_map.insert("_s3_tags".to_string(), tags_json);
    meta.metadata = serde_json::to_string(&meta_map).ok();

    state.bucket_meta.save_object(meta).await?;
    Ok(StatusCode::OK)
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
    // PUT /bucket/key?tagging — set object tags
    if all_query.iter().any(|(k, _)| k == "tagging") {
        return put_object_tagging(method, uri, bucket, key, all_query, state, headers, body).await
            .map(|r| r.into_response());
    }

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
        // Check for UploadPartCopy: uploadId + partNumber + x-amz-copy-source
        if let Some(copy_source) = headers.get("x-amz-copy-source").and_then(|h| h.to_str().ok()) {
            tracing::info!(upload_id = %upload_id, part = part_number, source = %copy_source, "Upload Part Copy");

            let source_path = copy_source.trim_start_matches('/');
            let (full_key, _) = source_path.split_once('?').unwrap_or((source_path, ""));
            let (src_bucket, src_key_encoded) = full_key.split_once('/').ok_or_else(|| {
                S3Error::new(S3ErrorCode::InvalidArgument, "Invalid copy source", StatusCode::BAD_REQUEST)
            })?;
            let src_key = percent_decode_str(src_key_encoded).decode_utf8_lossy().to_string();

            let src_meta = state.bucket_meta.get_object(src_bucket, &src_key).await?
                .ok_or_else(|| S3Error::new(S3ErrorCode::NoSuchKey, "Source object not found", StatusCode::NOT_FOUND))?;

            let (size, md5) = state.storage.copy_to_part(src_bucket, &src_key, upload_id, part_number).await
                .map_err(|e| S3Error::internal(e))?;

            state.multipart_meta.save_part(upload_id, MultipartPart {
                part_number,
                size,
                checksum: md5.clone(),
            }).await?;

            let now = chrono::Utc::now();
            let response = CopyPartResponse {
                last_modified: now.format("%Y-%m-%dT%H:%M:%S.000Z").to_string(),
                etag: format!("\"{}\"", md5),
            };
            let xml = quick_xml::se::to_string(&response).map_err(|e| S3Error::internal(e))?;

            // Also attach the source object's size to metadata for completeness (not strictly needed)
            let _ = src_meta.size;

            return Ok((
                StatusCode::OK,
                [(axum::http::header::CONTENT_TYPE, "application/xml")],
                format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n{}", xml),
            ).into_response());
        }

        tracing::info!(upload_id = %upload_id, part = part_number, "Upload Part (Streaming)");
        let (size, md5) = if expected_payload_hash == "STREAMING-AWS4-HMAC-SHA256-PAYLOAD" {
            let raw = axum::body::to_bytes(body, usize::MAX).await
                .map_err(|e| S3Error::internal(e))?;
            let decoded = decode_aws_chunked(&raw)
                .map_err(|e| S3Error::internal(e))?;
            state.storage.put_part(upload_id, part_number, axum::body::Body::from(bytes::Bytes::from(decoded)), "UNSIGNED-PAYLOAD").await
                .map_err(|e| S3Error::internal(e))?
        } else {
            state.storage.put_part(upload_id, part_number, body, &expected_payload_hash).await
                .map_err(|e| {
                    if e.to_string().contains("Checksum mismatch") {
                        S3Error::new(S3ErrorCode::XAmzContentSHA256Mismatch, e.to_string(), StatusCode::BAD_REQUEST)
                    } else {
                        S3Error::internal(e)
                    }
                })?
        };

        state.multipart_meta.save_part(upload_id, MultipartPart {
            part_number,
            size,
            checksum: md5.clone(),
        }).await?;
        Ok((
            StatusCode::OK,
            [(axum::http::header::ETAG, format!("\"{}\"", md5))],
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

        // Check copy conditions
        let src_etag = src_meta.checksum.trim_matches('"').to_string();
        if let Some(if_match) = headers.get("x-amz-copy-source-if-match").and_then(|h| h.to_str().ok()) {
            let if_match = if_match.trim_matches('"');
            if if_match != src_etag {
                return Err(S3Error::new(S3ErrorCode::AccessDenied, "At least one of the preconditions you specified did not hold", StatusCode::PRECONDITION_FAILED));
            }
        }
        if let Some(if_none_match) = headers.get("x-amz-copy-source-if-none-match").and_then(|h| h.to_str().ok()) {
            let if_none_match = if_none_match.trim_matches('"');
            if if_none_match == src_etag {
                return Err(S3Error::new(S3ErrorCode::AccessDenied, "At least one of the preconditions you specified did not hold", StatusCode::PRECONDITION_FAILED));
            }
        }
        if let Some(since) = headers.get("x-amz-copy-source-if-unmodified-since").and_then(|h| h.to_str().ok()) {
            if let Ok(since_dt) = chrono::DateTime::parse_from_rfc2822(since).or_else(|_| {
                chrono::DateTime::parse_from_str(since, "%a, %d %b %Y %H:%M:%S %Z")
            }) {
                if src_meta.created_at > since_dt.with_timezone(&chrono::Utc) {
                    return Err(S3Error::new(S3ErrorCode::AccessDenied, "At least one of the preconditions you specified did not hold", StatusCode::PRECONDITION_FAILED));
                }
            }
        }
        if let Some(since) = headers.get("x-amz-copy-source-if-modified-since").and_then(|h| h.to_str().ok()) {
            if let Ok(since_dt) = chrono::DateTime::parse_from_rfc2822(since).or_else(|_| {
                chrono::DateTime::parse_from_str(since, "%a, %d %b %Y %H:%M:%S %Z")
            }) {
                if src_meta.created_at <= since_dt.with_timezone(&chrono::Utc) {
                    return Err(S3Error::new(S3ErrorCode::AccessDenied, "At least one of the preconditions you specified did not hold", StatusCode::PRECONDITION_FAILED));
                }
            }
        }

        state.storage.copy(src_bucket, &src_key, &bucket, &key).await?;

        // x-amz-storage-class on the copy request overrides the source; absent means inherit
        let storage_class = headers.get("x-amz-storage-class")
            .and_then(|h| h.to_str().ok())
            .map(|s| s.to_string())
            .or_else(|| src_meta.storage_class.clone());

        let now = chrono::Utc::now();
        state.bucket_meta.save_object(S3ObjectMetadata {
            bucket: bucket.clone(),
            key: key.clone(),
            size: src_meta.size,
            content_type: src_meta.content_type,
            checksum: src_meta.checksum.clone(),
            metadata: src_meta.metadata.clone(),
            storage_class,
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
        let snowball_auto_extract = headers.get("x-amz-meta-snowball-auto-extract")
            .and_then(|h| h.to_str().ok())
            .map(|v| v.eq_ignore_ascii_case("true"))
            .unwrap_or(false);

        if snowball_auto_extract {
            tracing::info!(bucket = %bucket, key = %key, "PUT Snowball (auto-extract)");
            // Buffer the entire body
            let body_bytes = axum::body::to_bytes(body, usize::MAX).await
                .map_err(|e| S3Error::internal(e))?;

            // Parse tar in a blocking thread (tar::Archive is not Send)
            let extracted: Vec<(String, bytes::Bytes)> = tokio::task::spawn_blocking(move || {
                use std::io::Read;

                // Decompress if gzip (magic bytes 0x1f 0x8b)
                let tar_data: Vec<u8> = if body_bytes.starts_with(&[0x1f, 0x8b]) {
                    let mut decoder = flate2::read::GzDecoder::new(&body_bytes[..]);
                    let mut buf = Vec::new();
                    decoder.read_to_end(&mut buf)?;
                    buf
                } else {
                    body_bytes.to_vec()
                };

                let mut archive = tar::Archive::new(std::io::Cursor::new(tar_data));
                let mut result = Vec::new();
                for entry in archive.entries()? {
                    let mut entry = entry?;
                    if entry.header().entry_type().is_dir() {
                        continue;
                    }
                    let entry_key = entry.path()?
                        .to_string_lossy()
                        .trim_start_matches("./")
                        .to_string();
                    if entry_key.is_empty() {
                        continue;
                    }
                    let mut data = Vec::new();
                    entry.read_to_end(&mut data)?;
                    result.push((entry_key, bytes::Bytes::from(data)));
                }
                Ok::<_, std::io::Error>(result)
            })
            .await
            .map_err(|e| S3Error::internal(e))?
            .map_err(|e| S3Error::internal(e))?;

            for (entry_key, data_bytes) in extracted {
                let (size, md5) = state.storage.put_bytes(&bucket, &entry_key, data_bytes, "UNSIGNED-PAYLOAD").await
                    .map_err(|e| S3Error::internal(e))?;
                state.bucket_meta.save_object(S3ObjectMetadata {
                    bucket: bucket.clone(),
                    key: entry_key,
                    size,
                    content_type: "application/octet-stream".to_string(),
                    checksum: md5,
                    metadata: None,
                    storage_class: None,
                    created_at: chrono::Utc::now(),
                }).await?;
            }

            Ok(StatusCode::OK.into_response())
        } else {
            // Validate Content-MD5 if provided (before streaming, check format; after, check value)
            let content_md5_header = headers.get("content-md5")
                .and_then(|h| h.to_str().ok())
                .map(|s| s.trim().to_string());

            tracing::info!(bucket = %bucket, key = %key, "PUT Object (Streaming)");

            // Detect any x-amz-checksum-* header that needs validation
            const CHECKSUM_NAMES: &[(&str, &str)] = &[
                ("x-amz-checksum-crc32", "crc32"),
                ("x-amz-checksum-crc32c", "crc32c"),
                ("x-amz-checksum-sha1", "sha1"),
                ("x-amz-checksum-sha256", "sha256"),
            ];
            let provided_checksum: Option<(&str, String)> = CHECKSUM_NAMES.iter()
                .find_map(|(hdr, algo)| {
                    headers.get(*hdr).and_then(|h| h.to_str().ok())
                        .map(|v| (*algo, v.to_string()))
                });

            let (size, md5) = if expected_payload_hash == "STREAMING-AWS4-HMAC-SHA256-PAYLOAD" {
                let raw = axum::body::to_bytes(body, usize::MAX).await
                    .map_err(|e| S3Error::internal(e))?;
                let decoded = decode_aws_chunked(&raw)
                    .map_err(|e| S3Error::internal(e))?;
                if let Some((algo, ref expected_b64)) = provided_checksum {
                    validate_amz_checksum(algo, &decoded, expected_b64)?;
                }
                state.storage.put_bytes(&bucket, &key, bytes::Bytes::from(decoded), "UNSIGNED-PAYLOAD").await
                    .map_err(|e| S3Error::internal(e))?
            } else if provided_checksum.is_some() {
                // Buffer to validate checksum
                let data = axum::body::to_bytes(body, usize::MAX).await
                    .map_err(|e| S3Error::internal(e))?;
                let (algo, ref expected_b64) = provided_checksum.as_ref().unwrap();
                validate_amz_checksum(algo, &data, expected_b64)?;
                state.storage.put_bytes(&bucket, &key, data, &expected_payload_hash).await
                    .map_err(|e| S3Error::internal(e))?
            } else {
                state.storage.put(&bucket, &key, body, &expected_payload_hash).await
                    .map_err(|e| {
                        if e.to_string().contains("Checksum mismatch") {
                            S3Error::new(S3ErrorCode::XAmzContentSHA256Mismatch, e.to_string(), StatusCode::BAD_REQUEST)
                        } else {
                            S3Error::internal(e)
                        }
                    })?
            };

            // If Content-MD5 was provided, validate it against the computed MD5
            if let Some(ref provided_md5_b64) = content_md5_header {
                use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
                let provided_md5_bytes = BASE64.decode(provided_md5_b64)
                    .map_err(|_| S3Error::new(S3ErrorCode::InvalidDigest, "The Content-MD5 you specified is not valid.", StatusCode::BAD_REQUEST))?;
                if provided_md5_bytes.len() != 16 {
                    return Err(S3Error::new(S3ErrorCode::InvalidDigest, "The Content-MD5 you specified is not valid.", StatusCode::BAD_REQUEST));
                }
                let computed_md5_bytes = hex::decode(&md5)
                    .map_err(|e| S3Error::internal(e))?;
                if provided_md5_bytes != computed_md5_bytes {
                    return Err(S3Error::new(S3ErrorCode::BadDigest, "The Content-MD5 you specified did not match what we received.", StatusCode::BAD_REQUEST));
                }
            }

            let mut metadata: HashMap<String, String> = headers.iter()
                .filter(|(k, _)| k.as_str().starts_with("x-amz-meta-"))
                .map(|(k, v)| (k.as_str().to_string(), v.to_str().unwrap_or("").to_string()))
                .collect();
            // Store standard content headers so they're returned on GET/HEAD
            for hdr in &["content-language", "content-disposition", "content-encoding", "cache-control", "expires"] {
                if let Some(v) = headers.get(*hdr).and_then(|h| h.to_str().ok()) {
                    metadata.insert(hdr.to_string(), v.to_string());
                }
            }
            // Store x-amz-checksum-* headers so they can be returned on GET
            for hdr in &["x-amz-checksum-crc32", "x-amz-checksum-crc32c", "x-amz-checksum-sha1", "x-amz-checksum-sha256"] {
                if let Some(v) = headers.get(*hdr).and_then(|h| h.to_str().ok()) {
                    metadata.insert(hdr.to_string(), v.to_string());
                }
            }
            // Parse x-amz-tagging header (URL-encoded key=value pairs) and store as _s3_tags
            if let Some(tagging) = headers.get("x-amz-tagging").and_then(|h| h.to_str().ok()) {
                let tags: HashMap<String, String> = tagging.split('&')
                    .filter_map(|pair| {
                        let mut parts = pair.splitn(2, '=');
                        let k = parts.next().map(|s| percent_encoding::percent_decode_str(s).decode_utf8_lossy().to_string())?;
                        let v = parts.next().unwrap_or("").to_string();
                        let v = percent_encoding::percent_decode_str(&v).decode_utf8_lossy().to_string();
                        if k.is_empty() { None } else { Some((k, v)) }
                    })
                    .collect();
                if let Ok(tags_json) = serde_json::to_string(&tags) {
                    metadata.insert("_s3_tags".to_string(), tags_json);
                }
            }
            let metadata_json = serde_json::to_string(&metadata).ok();

            const VALID_STORAGE_CLASSES: &[&str] = &[
                "STANDARD", "REDUCED_REDUNDANCY", "STANDARD_IA", "ONEZONE_IA",
                "INTELLIGENT_TIERING", "GLACIER", "DEEP_ARCHIVE", "GLACIER_IR",
            ];
            let storage_class = if let Some(sc) = headers.get("x-amz-storage-class").and_then(|h| h.to_str().ok()) {
                if !VALID_STORAGE_CLASSES.contains(&sc) {
                    return Err(S3Error::new(
                        S3ErrorCode::InvalidArgument,
                        format!("Invalid storage class: {}", sc),
                        StatusCode::BAD_REQUEST,
                    ));
                }
                Some(sc.to_string())
            } else {
                None
            };

            state.bucket_meta.save_object(S3ObjectMetadata {
                bucket: bucket.clone(),
                key: key.clone(),
                size,
                content_type: headers.get("content-type").and_then(|h| h.to_str().ok()).unwrap_or("application/octet-stream").to_string(),
                checksum: md5.clone(),
                metadata: metadata_json,
                storage_class,
                created_at: chrono::Utc::now(),
            }).await?;

            // Build response headers: ETag + any checksum headers echoed back
            let mut resp_headers = axum::http::HeaderMap::new();
            resp_headers.insert(
                axum::http::header::ETAG,
                axum::http::HeaderValue::from_str(&format!("\"{}\"", md5)).unwrap(),
            );
            for hdr in &["x-amz-checksum-crc32", "x-amz-checksum-crc32c", "x-amz-checksum-sha1", "x-amz-checksum-sha256"] {
                if let Some(v) = headers.get(*hdr).and_then(|h| h.to_str().ok()) {
                    if let (Ok(name), Ok(val)) = (
                        axum::http::HeaderName::from_bytes(hdr.as_bytes()),
                        axum::http::HeaderValue::from_str(v),
                    ) {
                        resp_headers.insert(name, val);
                    }
                }
            }
            Ok((StatusCode::OK, resp_headers, "").into_response())
        }
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

    // Validate MaxParts and PartNumberMarker (must be non-negative integers)
    if let Some(max_parts_str) = query_map.get("max-parts") {
        match max_parts_str.parse::<i64>() {
            Ok(n) if n < 0 => return Err(S3Error::new(S3ErrorCode::InvalidArgument, "max-parts must be a positive integer", StatusCode::BAD_REQUEST)),
            Err(_) => return Err(S3Error::new(S3ErrorCode::InvalidArgument, "max-parts must be a positive integer", StatusCode::BAD_REQUEST)),
            _ => {}
        }
    }
    if let Some(marker_str) = query_map.get("part-number-marker") {
        match marker_str.parse::<i64>() {
            Ok(n) if n < 0 => return Err(S3Error::new(S3ErrorCode::InvalidArgument, "part-number-marker must be a positive integer", StatusCode::BAD_REQUEST)),
            Err(_) => return Err(S3Error::new(S3ErrorCode::InvalidArgument, "part-number-marker must be a positive integer", StatusCode::BAD_REQUEST)),
            _ => {}
        }
    }

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

/// Encode a single AWS Event Stream message.
/// Format: [total_len:4BE][headers_len:4BE][prelude_crc32:4BE][headers][payload][msg_crc32:4BE]
/// Header entry: [name_len:1][name_bytes][type:1=7 for string][value_len:2BE][value_bytes]
/// Header names must include the ":" prefix (e.g., ":message-type").
fn encode_event_message(event_headers: &[(&str, &str)], payload: &[u8]) -> Vec<u8> {
    let mut hdr_bytes: Vec<u8> = Vec::new();
    for (name, val) in event_headers {
        let nb = name.as_bytes();
        let vb = val.as_bytes();
        hdr_bytes.push(nb.len() as u8);
        hdr_bytes.extend_from_slice(nb);
        hdr_bytes.push(7u8); // string value type
        hdr_bytes.extend_from_slice(&(vb.len() as u16).to_be_bytes());
        hdr_bytes.extend_from_slice(vb);
    }
    let total_len = 4 + 4 + 4 + hdr_bytes.len() + payload.len() + 4;
    let mut prelude = Vec::with_capacity(8);
    prelude.extend_from_slice(&(total_len as u32).to_be_bytes());
    prelude.extend_from_slice(&(hdr_bytes.len() as u32).to_be_bytes());
    let prelude_crc = crc32fast::hash(&prelude);
    let mut msg = Vec::with_capacity(total_len);
    msg.extend_from_slice(&prelude);
    msg.extend_from_slice(&prelude_crc.to_be_bytes());
    msg.extend_from_slice(&hdr_bytes);
    msg.extend_from_slice(payload);
    let msg_crc = crc32fast::hash(&msg);
    msg.extend_from_slice(&msg_crc.to_be_bytes());
    msg
}

/// Extract a column name from an SQL identifier like `s."Name"` or `"Name"` or `Name`.
fn extract_col_name(s: &str) -> Option<String> {
    let s = s.trim();
    let s = if let Some(dot) = s.rfind('.') { &s[dot + 1..] } else { s };
    let s = s.trim().trim_matches('"').trim_matches('`');
    if s.is_empty() { None } else { Some(s.to_string()) }
}

/// Parse WHERE conditions from an SQL expression.
/// Returns Vec of (column, value) equality pairs.
fn parse_where_conditions(expr: &str) -> Vec<(String, String)> {
    let mut conditions = Vec::new();
    let upper = expr.to_uppercase();
    let where_start = match upper.find(" WHERE ") {
        Some(i) => i + 7,
        None => return conditions,
    };
    let clause = &expr[where_start..];
    // Split on " AND " case-insensitively
    let upper_clause = clause.to_uppercase();
    let mut start = 0;
    let mut parts: Vec<&str> = Vec::new();
    loop {
        match upper_clause[start..].find(" AND ") {
            Some(i) => { parts.push(&clause[start..start + i]); start += i + 5; }
            None => { parts.push(&clause[start..]); break; }
        }
    }
    for part in parts {
        let part = part.trim();
        if let Some(eq_pos) = part.find('=') {
            let lhs = part[..eq_pos].trim();
            let rhs = part[eq_pos + 1..].trim();
            if let Some(col) = extract_col_name(lhs) {
                let val = if rhs.starts_with('\'') && rhs.ends_with('\'') {
                    rhs[1..rhs.len() - 1].to_string()
                } else if rhs.starts_with('"') && rhs.ends_with('"') {
                    rhs[1..rhs.len() - 1].to_string()
                } else {
                    rhs.to_string()
                };
                conditions.push((col, val));
            }
        }
    }
    conditions
}

/// Strip outer CSV double-quotes and unescape internal "" pairs.
fn unquote_csv_field(s: &str) -> String {
    let s = s.trim();
    if s.starts_with('"') && s.ends_with('"') && s.len() >= 2 {
        s[1..s.len() - 1].replace("\"\"", "\"")
    } else {
        s.to_string()
    }
}

/// Split a CSV row by the given delimiter, handling double-quoted fields.
fn split_csv_row(row: &str, delim: &str) -> Vec<String> {
    // Simple split with quote stripping — handles basic cases
    row.split(delim).map(unquote_csv_field).collect()
}

/// Process CSV data with a SQL SELECT/WHERE expression.
fn process_csv_select(
    data: &str,
    expression: &str,
    file_header_info: &str,
    in_field_delim: &str,
    in_record_delim: &str,
    out_field_delim: &str,
    out_record_delim: &str,
) -> Vec<u8> {
    let in_fd = if in_field_delim.is_empty() { "," } else { in_field_delim };
    let in_rd = if in_record_delim.is_empty() { "\n" } else { in_record_delim };
    let out_fd = if out_field_delim.is_empty() { "," } else { out_field_delim };
    let out_rd = if out_record_delim.is_empty() { "\n" } else { out_record_delim };

    let where_conds = parse_where_conditions(expression);
    let upper_expr = expression.to_uppercase();
    let from_pos = upper_expr.find(" FROM ").unwrap_or(upper_expr.len());
    let select_clause = upper_expr[7..from_pos].trim(); // skip "SELECT "
    let select_all = select_clause == "*";
    let select_cols: Vec<String> = if !select_all {
        expression[7..from_pos].split(',')
            .filter_map(|c| extract_col_name(c))
            .collect()
    } else { Vec::new() };

    let rows: Vec<&str> = data.split(in_rd).collect();
    // col_names: the names used in the row_map for WHERE/SELECT lookup
    // data_rows: the rows to process (may or may not skip header)
    let (col_names, data_rows): (Vec<String>, &[&str]) = match file_header_info.to_uppercase().as_str() {
        "USE" if !rows.is_empty() => {
            // Header row provides column names; skip it from output
            let hdrs = split_csv_row(rows[0], in_fd);
            (hdrs, &rows[1..])
        }
        "IGNORE" if !rows.is_empty() => {
            // Skip header row; use positional names _1, _2, ...
            let n = rows[0].split(in_fd).count();
            let hdrs = (1..=n).map(|i| format!("_{}", i)).collect();
            (hdrs, &rows[1..])
        }
        _ => {
            // No header; use positional names for all rows
            let max = rows.iter().map(|r| r.split(in_fd).count()).max().unwrap_or(0);
            let hdrs = (1..=max).map(|i| format!("_{}", i)).collect();
            (hdrs, &rows[..])
        }
    };

    let mut result = Vec::new();
    for row in data_rows {
        if row.trim().is_empty() { continue; }
        let fields: Vec<String> = split_csv_row(row, in_fd);
        // Build column map (unquoted values) for WHERE evaluation and SELECT
        let row_map: HashMap<&str, &str> = col_names.iter()
            .enumerate()
            .filter_map(|(i, h)| fields.get(i).map(|f| (h.as_str(), f.as_str())))
            .collect();
        let matches = where_conds.iter().all(|(col, val)| {
            row_map.get(col.as_str()).map(|v| *v == val.as_str()).unwrap_or(false)
        });
        if matches {
            let out_fields: Vec<&str> = if select_all {
                fields.iter().map(|f| f.as_str()).collect()
            } else {
                select_cols.iter().filter_map(|c| row_map.get(c.as_str()).copied()).collect()
            };
            let out_row = out_fields.join(out_fd);
            result.extend_from_slice(out_row.as_bytes());
            result.extend_from_slice(out_rd.as_bytes());
        }
    }
    result
}

/// Handler for POST /bucket/key?select — S3 Select Object Content
async fn select_object_content(
    bucket: String,
    key: String,
    state: Arc<AppState>,
    headers: axum::http::HeaderMap,
    body: axum::body::Body,
) -> Result<impl IntoResponse, S3Error> {
    // Parse request XML
    let body_bytes = axum::body::to_bytes(body, usize::MAX).await.map_err(|e| S3Error::internal(e))?;
    let body_str = std::str::from_utf8(&body_bytes).map_err(|e| S3Error::internal(e))?;

    // Extract fields from XML using quick_xml
    let mut expression = String::new();
    let mut file_header_info = String::new();
    let mut in_field_delim = String::from(",");
    let mut in_record_delim = String::from("\n");
    let mut out_field_delim = String::from(",");
    let mut out_record_delim = String::from("\n");

    let mut reader = quick_xml::Reader::from_str(body_str);
    reader.config_mut().trim_text(true);
    let mut current_tag = String::new();
    let mut in_input = false;
    let mut in_output = false;
    loop {
        match reader.read_event() {
            Ok(quick_xml::events::Event::Start(e)) => {
                let name = std::str::from_utf8(e.name().as_ref()).unwrap_or("").to_string();
                match name.as_str() {
                    "InputSerialization" => { in_input = true; in_output = false; }
                    "OutputSerialization" => { in_output = true; in_input = false; }
                    _ => {}
                }
                current_tag = name;
            }
            Ok(quick_xml::events::Event::End(e)) => {
                let name_bytes = e.name();
                let name = std::str::from_utf8(name_bytes.as_ref()).unwrap_or("");
                if name == "InputSerialization" { in_input = false; }
                if name == "OutputSerialization" { in_output = false; }
            }
            Ok(quick_xml::events::Event::Text(e)) => {
                let text = e.unescape().unwrap_or_default().to_string();
                match current_tag.as_str() {
                    "Expression" => { expression = text; }
                    "FileHeaderInfo" if in_input => { file_header_info = text; }
                    "FieldDelimiter" if in_input => { in_field_delim = text; }
                    "RecordDelimiter" if in_input => { in_record_delim = text; }
                    "FieldDelimiter" if in_output => { out_field_delim = text; }
                    "RecordDelimiter" if in_output => { out_record_delim = text; }
                    _ => {}
                }
            }
            Ok(quick_xml::events::Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
    }

    // Read the S3 object
    let obj_body = state.storage.get(&bucket, &key).await
        .map_err(|_| S3Error::new(S3ErrorCode::NoSuchKey, "Object not found", StatusCode::NOT_FOUND))?;
    let obj_bytes = axum::body::to_bytes(obj_body, usize::MAX).await
        .map_err(|e| S3Error::internal(e))?;
    let obj_str = std::str::from_utf8(&obj_bytes).map_err(|e| S3Error::internal(e))?;

    let bytes_scanned = obj_bytes.len() as u64;

    // Execute the select
    let result_data = process_csv_select(
        obj_str, &expression, &file_header_info,
        &in_field_delim, &in_record_delim,
        &out_field_delim, &out_record_delim,
    );

    let bytes_returned = result_data.len() as u64;

    // Build the event stream
    let mut stream_bytes = Vec::new();

    // Records event (only if there's data)
    if !result_data.is_empty() {
        stream_bytes.extend(encode_event_message(
            &[(":message-type", "event"), (":event-type", "Records"), (":content-type", "application/octet-stream")],
            &result_data,
        ));
    }

    // Stats event (content-type must be text/xml per minio-js parser)
    let stats_xml = format!(
        "<Stats><BytesScanned>{}</BytesScanned><BytesProcessed>{}</BytesProcessed><BytesReturned>{}</BytesReturned></Stats>",
        bytes_scanned, bytes_scanned, bytes_returned,
    );
    stream_bytes.extend(encode_event_message(
        &[(":message-type", "event"), (":event-type", "Stats"), (":content-type", "text/xml")],
        stats_xml.as_bytes(),
    ));

    // End event
    stream_bytes.extend(encode_event_message(
        &[(":message-type", "event"), (":event-type", "End")],
        &[],
    ));

    let _ = headers; // auth already validated by caller
    let mut resp_headers = axum::http::HeaderMap::new();
    resp_headers.insert(axum::http::header::CONTENT_TYPE, "application/octet-stream".parse().unwrap());
    Ok((StatusCode::OK, resp_headers, stream_bytes).into_response())
}

/// Unified handler for POST requests (Initiate and Complete Multipart)
pub async fn post_handler(
    method: axum::http::Method,
    axum::extract::OriginalUri(uri): axum::extract::OriginalUri,
    Path((bucket, key)): Path<(String, String)>,
    Query(all_query): Query<Vec<(String, String)>>,
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    body: axum::body::Body,
) -> Result<impl IntoResponse, S3Error> {
    let payload_hash = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"; // SHA256 of empty string

    let query_map: HashMap<String, String> = all_query.iter().cloned().collect();

    // POST /{bucket}/{key} is only valid for multipart upload operations or select.
    // Return MethodNotAllowed before auth for any other case (e.g. malformed presigned POST URLs).
    if !query_map.contains_key("uploads") && !query_map.contains_key("uploadId")
        && !query_map.contains_key("select") && !query_map.contains_key("select-type")
    {
        return Err(S3Error::new(S3ErrorCode::MethodNotAllowed, "The specified method is not allowed against this resource.", StatusCode::METHOD_NOT_ALLOWED));
    }

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

    // 2. Proceed with Logic
    if query_map.contains_key("uploads") {
        let mut metadata: HashMap<String, String> = headers.iter()
            .filter(|(k, _)| k.as_str().starts_with("x-amz-meta-"))
            .map(|(k, v)| (k.as_str().to_string(), v.to_str().unwrap_or("").to_string()))
            .collect();
        // Store standard content headers so they're preserved in the final object
        for hdr in &["content-language", "content-disposition", "content-encoding", "cache-control", "expires"] {
            if let Some(v) = headers.get(*hdr).and_then(|h| h.to_str().ok()) {
                metadata.insert(hdr.to_string(), v.to_string());
            }
        }
        let metadata_json = if metadata.is_empty() { None } else { serde_json::to_string(&metadata).ok() };
        let content_type = headers.get("content-type").and_then(|h| h.to_str().ok()).map(|s| s.to_string());
        let storage_class = headers.get("x-amz-storage-class").and_then(|h| h.to_str().ok()).map(|s| s.to_string());
        let upload_id: String = state.multipart_meta.initiate_multipart(&bucket, &key, metadata_json, content_type, storage_class).await?;
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
                S3Error::new(S3ErrorCode::NoSuchUpload, "The specified upload does not exist.", StatusCode::NOT_FOUND)
            })?;

        // Check part sizes: all parts except the last must be >= 5MB
        const MIN_PART_SIZE: u64 = 5 * 1024 * 1024; // 5MB
        let mut sorted_for_check = upload.parts.clone();
        sorted_for_check.sort_by_key(|p| p.part_number);
        if sorted_for_check.len() > 1 {
            for part in &sorted_for_check[..sorted_for_check.len() - 1] {
                if part.size < MIN_PART_SIZE {
                    return Err(S3Error::new(
                        S3ErrorCode::EntityTooSmall,
                        "Your proposed upload is smaller than the minimum allowed object size.",
                        StatusCode::BAD_REQUEST,
                    ));
                }
            }
        }

        let part_numbers: Vec<u32> = upload.parts.iter().map(|p| p.part_number).collect();
        tracing::info!(upload_id = %upload_id, parts = ?part_numbers, "Assembling parts");
        let total_size: u64 = state.storage.assemble(upload_id, &bucket, &key, part_numbers).await?;

        // S3 multipart ETag = MD5(concat of each part's MD5 bytes) + "-" + part_count
        let etag = {
            use md5::{Md5, Digest as Md5Digest};
            let mut sorted_parts = upload.parts.clone();
            sorted_parts.sort_by_key(|p| p.part_number);
            let mut combined = Vec::with_capacity(sorted_parts.len() * 16);
            for part in &sorted_parts {
                if let Ok(bytes) = hex::decode(&part.checksum) {
                    combined.extend_from_slice(&bytes);
                }
            }
            let md5_hex = hex::encode(Md5::digest(&combined));
            format!("{}-{}", md5_hex, sorted_parts.len())
        };

        tracing::info!(upload_id = %upload_id, total_size = total_size, etag = %etag, "Completing Multipart Upload ETag");

        // Store part info in metadata so GetObjectAttributes can return it
        let mut object_metadata: HashMap<String, String> = if let Some(ref mj) = upload.metadata {
            match serde_json::from_str(mj) {
                Ok(m) => m,
                Err(e) => {
                    tracing::warn!(upload_id = %upload_id, metadata = %mj, error = ?e, "Failed to parse multipart upload metadata JSON");
                    HashMap::new()
                }
            }
        } else {
            HashMap::new()
        };
        tracing::debug!(upload_id = %upload_id, metadata_keys = ?object_metadata.keys().collect::<Vec<_>>(), "Parsed multipart metadata");
        {
            let mut sorted_parts = upload.parts.clone();
            sorted_parts.sort_by_key(|p| p.part_number);
            let parts_json = serde_json::to_string(&sorted_parts.iter().map(|p| {
                serde_json::json!({"part_number": p.part_number, "size": p.size, "checksum": p.checksum})
            }).collect::<Vec<_>>()).unwrap_or_default();
            object_metadata.insert("_s3_parts".to_string(), parts_json);
        }

        state.bucket_meta.save_object(S3ObjectMetadata {
            bucket: bucket.clone(),
            key: key.clone(),
            size: total_size,
            content_type: upload.content_type.unwrap_or_else(|| "application/octet-stream".to_string()),
            checksum: etag.clone(),
            metadata: serde_json::to_string(&object_metadata).ok(),
            storage_class: upload.storage_class,
            created_at: chrono::Utc::now(),
        }).await?;

        state.multipart_meta.complete_multipart(upload_id).await?;
        state.storage.cleanup_multipart(upload_id).await?;

        #[derive(serde::Serialize)]
        #[serde(rename = "CompleteMultipartUploadResult")]
        #[serde(rename_all = "PascalCase")]
        struct CompleteMultipartResponse {
            location: String,
            bucket: String,
            key: String,
            #[serde(rename = "ETag")]
            etag: String,
        }

        let host = headers.get("host").and_then(|h| h.to_str().ok()).unwrap_or("localhost");
        let response = CompleteMultipartResponse {
            location: format!("http://{}/{}/{}", host, bucket, key),
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
        tracing::info!(bucket = %bucket, key = %key, "Select Object Content");
        select_object_content(bucket, key, state, headers, body).await.map(|r| r.into_response())
    } else {
        Err(S3Error::new(S3ErrorCode::InternalError, "Invalid POST request", StatusCode::BAD_REQUEST))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── decode_aws_chunked ────────────────────────────────────────────────────

    #[test]
    fn test_decode_single_chunk() {
        let input = b"5\r\nhello\r\n0\r\n\r\n";
        assert_eq!(decode_aws_chunked(input).unwrap(), b"hello");
    }

    #[test]
    fn test_decode_multiple_chunks() {
        let input = b"5\r\nhello\r\n5\r\nworld\r\n0\r\n\r\n";
        assert_eq!(decode_aws_chunked(input).unwrap(), b"helloworld");
    }

    #[test]
    fn test_decode_empty_body() {
        let input = b"0\r\n\r\n";
        assert_eq!(decode_aws_chunked(input).unwrap(), b"");
    }

    #[test]
    fn test_decode_chunk_with_signature_extension() {
        // Real AWS chunked format: size;chunk-signature=<hex>\r\n
        let input = b"5;chunk-signature=abc123\r\nhello\r\n0;chunk-signature=xyz\r\n\r\n";
        assert_eq!(decode_aws_chunked(input).unwrap(), b"hello");
    }

    #[test]
    fn test_decode_invalid_no_crlf() {
        let input = b"5hello";
        assert!(decode_aws_chunked(input).is_err());
    }

    #[test]
    fn test_decode_invalid_hex_size() {
        let input = b"zz\r\nhello\r\n";
        assert!(decode_aws_chunked(input).is_err());
    }

    // ── validate_amz_checksum ─────────────────────────────────────────────────

    fn sha256_b64(data: &[u8]) -> String {
        use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
        use sha2::{Sha256, Digest};
        BASE64.encode(Sha256::digest(data))
    }

    fn crc32_b64(data: &[u8]) -> String {
        use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
        BASE64.encode(crc32fast::hash(data).to_be_bytes())
    }

    #[test]
    fn test_validate_checksum_sha256_ok() {
        let data = b"hello world";
        assert!(validate_amz_checksum("sha256", data, &sha256_b64(data)).is_ok());
    }

    #[test]
    fn test_validate_checksum_sha256_bad() {
        let data = b"hello world";
        let result = validate_amz_checksum("sha256", data, "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=");
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_checksum_crc32_ok() {
        let data = b"hello world";
        assert!(validate_amz_checksum("crc32", data, &crc32_b64(data)).is_ok());
    }

    #[test]
    fn test_validate_checksum_crc32_bad() {
        let data = b"hello world";
        assert!(validate_amz_checksum("crc32", data, "AAAAAAAA").is_err());
    }

    #[test]
    fn test_validate_checksum_unknown_algo_passthrough() {
        // Unknown algorithms should not error — they are ignored.
        assert!(validate_amz_checksum("adler32", b"any data", "anything").is_ok());
    }
}
