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

    // GET /bucket/key?attributes — GetObjectAttributes
    if all_query.iter().any(|(k, _)| k == "attributes") {
        let (params, _) = validate_auth(
            &state, method.as_str(), uri.path(), &all_query, &headers,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        ).await?;
        super::validate_policy(&state, &params, method.as_str(), &bucket, Some(&key)).await?;
        let meta = state.bucket_meta.get_object(&bucket, &key).await?
            .ok_or_else(|| S3Error::new(S3ErrorCode::NoSuchKey, "Object not found", StatusCode::NOT_FOUND))?;

        // SDK may send multiple X-Amz-Object-Attributes headers (one per attribute)
        // or a single comma-separated header — collect all values
        let requested_attrs: Vec<String> = headers.get_all("x-amz-object-attributes")
            .iter()
            .filter_map(|h| h.to_str().ok())
            .flat_map(|v| v.split(','))
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();

        let etag_xml = if requested_attrs.iter().any(|a| a.eq_ignore_ascii_case("ETag")) {
            format!("<ETag>{}</ETag>", meta.checksum.trim_matches('"'))
        } else { String::new() };

        let size_xml = if requested_attrs.iter().any(|a| a.eq_ignore_ascii_case("ObjectSize")) {
            format!("<ObjectSize>{}</ObjectSize>", meta.size)
        } else { String::new() };

        let sc_xml = if requested_attrs.iter().any(|a| a.eq_ignore_ascii_case("StorageClass")) {
            format!("<StorageClass>{}</StorageClass>", meta.storage_class.as_deref().unwrap_or("STANDARD"))
        } else { String::new() };

        // Checksum if stored and requested
        let checksum_xml = if requested_attrs.iter().any(|a| a.eq_ignore_ascii_case("Checksum")) {
            if let Some(ref mj) = meta.metadata {
                if let Ok(mmap) = serde_json::from_str::<HashMap<String, String>>(mj) {
                    let mut parts = String::new();
                    if let Some(v) = mmap.get("x-amz-checksum-crc32") { parts.push_str(&format!("<ChecksumCRC32>{}</ChecksumCRC32>", v)); }
                    if let Some(v) = mmap.get("x-amz-checksum-crc32c") { parts.push_str(&format!("<ChecksumCRC32C>{}</ChecksumCRC32C>", v)); }
                    if let Some(v) = mmap.get("x-amz-checksum-sha1") { parts.push_str(&format!("<ChecksumSHA1>{}</ChecksumSHA1>", v)); }
                    if let Some(v) = mmap.get("x-amz-checksum-sha256") { parts.push_str(&format!("<ChecksumSHA256>{}</ChecksumSHA256>", v)); }
                    if !parts.is_empty() { format!("<Checksum>{}</Checksum>", parts) } else { String::new() }
                } else { String::new() }
            } else { String::new() }
        } else { String::new() };

        // Multipart ObjectParts if requested
        let multipart_xml = if requested_attrs.iter().any(|a| a.eq_ignore_ascii_case("ObjectParts")) {
            if let Some(ref mj) = meta.metadata {
                if let Ok(mmap) = serde_json::from_str::<HashMap<String, String>>(mj) {
                    if let Some(parts_str) = mmap.get("_s3_parts") {
                        if let Ok(parts) = serde_json::from_str::<Vec<serde_json::Value>>(parts_str) {
                            let part_entries: String = parts.iter().map(|p| {
                                let num = p.get("part_number").and_then(|v| v.as_u64()).unwrap_or(0);
                                let sz = p.get("size").and_then(|v| v.as_u64()).unwrap_or(0);
                                format!("<Part><PartNumber>{}</PartNumber><Size>{}</Size></Part>", num, sz)
                            }).collect();
                            format!("<ObjectParts><TotalPartsCount>{}</TotalPartsCount>{}</ObjectParts>", parts.len(), part_entries)
                        } else { String::new() }
                    } else { String::new() }
                } else { String::new() }
            } else { String::new() }
        } else { String::new() };

        let body = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<GetObjectAttributesResponse \
            xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">{}{}{}{}{}</GetObjectAttributesResponse>",
            etag_xml, size_xml, sc_xml, checksum_xml, multipart_xml
        );
        let mut resp_headers = axum::http::HeaderMap::new();
        resp_headers.insert(axum::http::header::CONTENT_TYPE, "application/xml".parse().unwrap());
        resp_headers.insert(
            axum::http::header::LAST_MODIFIED,
            meta.created_at.format("%a, %d %b %Y %H:%M:%S GMT").to_string().parse().unwrap_or_else(|_| "".parse().unwrap()),
        );
        return Ok((StatusCode::OK, resp_headers, body).into_response());
    }

    // GET /bucket/key?tagging — return object tags as XML
    if all_query.iter().any(|(k, _)| k == "tagging") {
        let (params, _) = validate_auth(
            &state, method.as_str(), uri.path(), &all_query, &headers,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        ).await?;
        super::validate_policy(&state, &params, method.as_str(), &bucket, Some(&key)).await?;
        let meta = state.bucket_meta.get_object(&bucket, &key).await?
            .ok_or_else(|| S3Error::new(S3ErrorCode::NoSuchKey, "Object not found", StatusCode::NOT_FOUND))?;
        let tags_xml = if let Some(meta_json) = &meta.metadata {
            if let Ok(meta_map) = serde_json::from_str::<HashMap<String, String>>(meta_json) {
                if let Some(tags_str) = meta_map.get("_s3_tags") {
                    if let Ok(tags) = serde_json::from_str::<HashMap<String, String>>(tags_str) {
                        let tag_entries: String = tags.iter()
                            .map(|(k, v)| format!("<Tag><Key>{}</Key><Value>{}</Value></Tag>", k, v))
                            .collect();
                        format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<Tagging><TagSet>{}</TagSet></Tagging>", tag_entries)
                    } else {
                        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<Tagging><TagSet/></Tagging>".to_string()
                    }
                } else {
                    "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<Tagging><TagSet/></Tagging>".to_string()
                }
            } else {
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<Tagging><TagSet/></Tagging>".to_string()
            }
        } else {
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<Tagging><TagSet/></Tagging>".to_string()
        };
        return Ok((
            StatusCode::OK,
            [(axum::http::header::CONTENT_TYPE, "application/xml")],
            tags_xml,
        ).into_response());
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

    let meta = state.bucket_meta.get_object(&bucket, &key).await?
        .ok_or_else(|| S3Error::new(S3ErrorCode::NoSuchKey, "Object metadata not found", StatusCode::NOT_FOUND))?;

    tracing::info!(bucket = %bucket, key = %key, size = meta.size, checksum = %meta.checksum, "GET Object metadata");

    let etag = meta.checksum.trim_matches('"').to_string();
    let quoted_etag = format!("\"{}\"", etag);
    let last_modified = meta.created_at.format("%a, %d %b %Y %H:%M:%S GMT").to_string();
    let total_size = meta.size;

    // Parse Range header: "bytes=start-end" or "bytes=start-"
    let range_header = headers.get("range").or_else(|| headers.get("Range"))
        .and_then(|h| h.to_str().ok())
        .and_then(|s| s.strip_prefix("bytes="));
    let (range_start, range_end) = if let Some(range_str) = range_header {
        let parts: Vec<&str> = range_str.splitn(2, '-').collect();
        if parts.get(0).map(|s| s.is_empty()).unwrap_or(false) {
            // Suffix range: bytes=-N means last N bytes
            let suffix_len: u64 = parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);
            let start = total_size.saturating_sub(suffix_len);
            (Some(start), Some(total_size.saturating_sub(1)))
        } else {
            let start: u64 = parts.get(0).and_then(|s| s.parse().ok()).unwrap_or(0);
            let end: u64 = parts.get(1).and_then(|s| if s.is_empty() { None } else { s.parse().ok() })
                .unwrap_or(total_size.saturating_sub(1));
            (Some(start), Some(end))
        }
    } else {
        (None, None)
    };

    let is_range = range_start.is_some();

    // Get full body then slice for range requests
    let body_bytes = {
        use http_body_util::BodyExt;
        let full_body = state.storage.get(&bucket, &key).await.map_err(|e| S3Error::internal(e))?;
        full_body.collect().await.map_err(|e| S3Error::internal(anyhow::anyhow!("{}", e)))?.to_bytes()
    };

    let (response_bytes, content_length, status) = if let (Some(start), Some(end)) = (range_start, range_end) {
        let start = start as usize;
        let end = (end as usize).min(body_bytes.len().saturating_sub(1));
        let sliced = body_bytes.slice(start..=end);
        let len = sliced.len() as u64;
        (sliced, len, StatusCode::PARTIAL_CONTENT)
    } else {
        let len = body_bytes.len() as u64;
        (body_bytes, len, StatusCode::OK)
    };

    tracing::info!(bucket = %bucket, key = %key, etag = %quoted_etag, "GET Object ETag");

    // Support S3 response header overrides via query params
    let query_map: HashMap<String, String> = all_query.iter().cloned().collect();
    let content_type = query_map.get("response-content-type")
        .map(|s| s.as_str())
        .unwrap_or(&meta.content_type)
        .to_string();

    let mut response_headers = HeaderMap::new();
    response_headers.insert(axum::http::header::ETAG, quoted_etag.parse().unwrap());
    response_headers.insert(axum::http::header::CONTENT_TYPE, content_type.parse().unwrap_or_else(|_| "application/octet-stream".parse().unwrap()));
    response_headers.insert(axum::http::header::CONTENT_LENGTH, content_length.to_string().parse().unwrap());
    response_headers.insert(axum::http::header::LAST_MODIFIED, last_modified.parse().unwrap());
    if is_range {
        let end = range_end.unwrap_or(total_size.saturating_sub(1));
        let cr = format!("bytes {}-{}/{}", range_start.unwrap(), end, total_size);
        if let Ok(v) = cr.parse() {
            response_headers.insert(axum::http::header::CONTENT_RANGE, v);
        }
    }
    if let Some(v) = query_map.get("response-content-disposition") {
        if let Ok(hv) = v.parse() {
            response_headers.insert(axum::http::header::CONTENT_DISPOSITION, hv);
        }
    }
    if let Some(v) = query_map.get("response-content-encoding") {
        if let Ok(hv) = v.parse() {
            response_headers.insert(axum::http::header::CONTENT_ENCODING, hv);
        }
    }
    if let Some(v) = query_map.get("response-content-language") {
        if let Ok(hv) = v.parse() {
            response_headers.insert(axum::http::header::CONTENT_LANGUAGE, hv);
        }
    }
    if let Some(v) = query_map.get("response-cache-control") {
        if let Ok(hv) = v.parse() {
            response_headers.insert(axum::http::header::CACHE_CONTROL, hv);
        }
    }
    if let Some(v) = query_map.get("response-expires") {
        if let Ok(hv) = v.parse() {
            response_headers.insert(axum::http::header::EXPIRES, hv);
        }
    }
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
            for (key, value) in &metadata {
                // Skip internal keys not meant to be HTTP headers
                if key.starts_with('_') { continue; }
                tracing::debug!(key = %key, value = %value, "Setting metadata header");
                if let Ok(header_name) = HeaderName::from_str(key) {
                    if let Ok(header_value) = HeaderValue::from_str(value) {
                        response_headers.insert(header_name, header_value);
                    } else {
                        tracing::warn!(key = %key, value = %value, "Header value invalid");
                    }
                } else {
                    tracing::warn!(key = %key, "Header name invalid");
                }
            }
        } else {
            tracing::warn!(json = %metadata_json, "Failed to parse object metadata JSON");
        }
    }

    Ok((
        status,
        response_headers,
        axum::body::Body::from(response_bytes),
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
    // x-minio-force-delete: delete all objects matching key as prefix
    let force_delete = headers.get("x-minio-force-delete")
        .and_then(|h| h.to_str().ok())
        .map(|v| v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);

    if force_delete {
        let (params, _) = validate_auth(
            &state, method.as_str(), uri.path(), &all_query, &headers,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        ).await?;
        super::validate_policy(&state, &params, method.as_str(), &bucket, Some(&key)).await?;

        // Normalize key: strip leading/trailing slashes to use as prefix
        let prefix = key.trim_start_matches('/').trim_end_matches('/').to_string();
        let objects = state.bucket_meta.list_objects(&bucket).await?;
        for obj in objects {
            if obj.key == prefix || obj.key.starts_with(&format!("{}/", prefix)) {
                let _ = state.storage.delete(&bucket, &obj.key).await;
                let _ = state.bucket_meta.delete_object(&bucket, &obj.key).await;
            }
        }
        return Ok(StatusCode::NO_CONTENT);
    }

    // AbortMultipartUpload: DELETE /bucket/key?uploadId=...
    if let Some(upload_id) = all_query.iter().find(|(k, _)| k == "uploadId").map(|(_, v)| v.clone()) {
        tracing::info!(bucket = %bucket, key = %key, upload_id = %upload_id, "Abort Multipart Upload");

        let (params, _) = validate_auth(
            &state, method.as_str(), uri.path(), &all_query, &headers,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        ).await?;
        super::validate_policy(&state, &params, method.as_str(), &bucket, Some(&key)).await?;

        state.storage.cleanup_multipart(&upload_id).await?;
        state.multipart_meta.complete_multipart(&upload_id).await?;
        return Ok(StatusCode::NO_CONTENT);
    }

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

    // Conditional delete: If-Match header
    if let Some(if_match) = headers.get("if-match").or_else(|| headers.get("If-Match"))
        .and_then(|h| h.to_str().ok())
    {
        let if_match_val = if_match.trim();
        match state.bucket_meta.get_object(&bucket, &key).await? {
            Some(meta) => {
                let etag = format!("\"{}\"", meta.checksum.trim_matches('"'));
                if if_match_val != "*" && if_match_val != etag && if_match_val != meta.checksum.trim_matches('"') {
                    return Err(S3Error::new(
                        S3ErrorCode::PreconditionFailed,
                        "At least one of the pre-conditions you specified did not hold",
                        StatusCode::PRECONDITION_FAILED,
                    ));
                }
            }
            None => {
                // If-Match: * on a non-existent object is a precondition failure
                return Err(S3Error::new(
                    S3ErrorCode::PreconditionFailed,
                    "At least one of the pre-conditions you specified did not hold",
                    StatusCode::PRECONDITION_FAILED,
                ));
            }
        }
    }

    state.storage.delete(&bucket, &key).await?;
    state.bucket_meta.delete_object(&bucket, &key).await?;

    Ok(StatusCode::NO_CONTENT)
}
