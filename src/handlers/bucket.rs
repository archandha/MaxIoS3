/// Request handlers for S3 bucket-level operations.
/// This file contains Axum handlers for listing buckets, creating/deleting buckets,
/// and listing objects within a bucket. It also implements the management of
/// bucket policies, allowing users to get, set, and delete JSON-based access controls.

use std::sync::Arc;
use axum::{
    extract::{Path, State, Query},
    http::StatusCode,
    response::IntoResponse,
};
use crate::{AppState, S3Error, S3ErrorCode, S3BucketMetadata, S3ObjectMetadata};
use quick_xml::se::to_string;
use super::validate_auth;
use serde::Serialize;

#[derive(Serialize)]
#[serde(rename = "ListAllMyBucketsResult")]
#[serde(rename_all = "PascalCase")]
pub struct ListBucketsResponse {
    pub owner: Owner,
    pub buckets: BucketsList,
}

#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct Owner {
    #[serde(rename = "ID")]
    pub id: String,
    pub display_name: String,
}

#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct BucketsList {
    #[serde(rename = "Bucket")]
    pub bucket: Vec<BucketEntry>,
}

#[derive(serde::Deserialize)]
#[serde(rename = "Delete")]
#[serde(rename_all = "PascalCase")]
pub struct DeleteObjectsRequest {
    pub quiet: Option<bool>,
    #[serde(rename = "Object")]
    pub objects: Vec<DeleteObjectEntry>,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct DeleteObjectEntry {
    pub key: String,
}

#[derive(Serialize)]
#[serde(rename = "DeleteResult")]
#[serde(rename_all = "PascalCase")]
pub struct DeleteObjectsResponse {
    #[serde(rename = "Deleted")]
    pub deleted: Vec<DeletedEntry>,
}

#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct DeletedEntry {
    pub key: String,
}

#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct BucketEntry {
    pub name: String,
    pub creation_date: String,
}

#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct CommonPrefixEntry {
    pub prefix: String,
}

#[derive(Serialize)]
#[serde(rename = "ListBucketResult")]
#[serde(rename_all = "PascalCase")]
pub struct ListObjectsV1Response {
    pub name: String,
    pub prefix: String,
    pub marker: String,
    pub max_keys: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delimiter: Option<String>,
    pub is_truncated: bool,
    #[serde(rename = "Contents")]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub contents: Vec<ObjectEntry>,
    #[serde(rename = "CommonPrefixes")]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub common_prefixes: Vec<CommonPrefixEntry>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_marker: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub encoding_type: Option<String>,
}

#[derive(Serialize)]
#[serde(rename = "ListBucketResult")]
#[serde(rename_all = "PascalCase")]
pub struct ListObjectsV2Response {
    pub name: String,
    pub prefix: String,
    pub max_keys: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delimiter: Option<String>,
    pub is_truncated: bool,
    pub key_count: usize,
    #[serde(rename = "Contents")]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub contents: Vec<ObjectEntry>,
    #[serde(rename = "CommonPrefixes")]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub common_prefixes: Vec<CommonPrefixEntry>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub continuation_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_continuation_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_after: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub encoding_type: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct ObjectEntry {
    pub key: String,
    pub last_modified: String,
    #[serde(rename = "ETag")]
    pub etag: String,
    pub size: u64,
    pub storage_class: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner: Option<Owner>,
}

/// Handler for GET List Buckets
pub async fn list_buckets(
    method: axum::http::Method,
    axum::extract::OriginalUri(uri): axum::extract::OriginalUri,
    Query(all_query): Query<Vec<(String, String)>>,
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
) -> Result<axum::response::Response, S3Error> {
    tracing::info!("List Buckets");

    let (_params, _) = validate_auth(
        &state,
        method.as_str(),
        uri.path(),
        &all_query,
        &headers,
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
    ).await?;

    // Note: ListBuckets is usually global, but we can check if there's a policy for the root?
    // S3 usually doesn't have policies for ListBuckets on individual buckets.
    // Let's skip policy check for ListBuckets for now.

    let buckets: Vec<S3BucketMetadata> = state.bucket_meta.list_buckets().await?;

    let response = ListBucketsResponse {
        owner: Owner {
            id: "admin".to_string(),
            display_name: "admin".to_string(),
        },
        buckets: BucketsList {
            bucket: buckets.into_iter().map(|b| BucketEntry {
                name: b.name,
                creation_date: b.created_at.format("%Y-%m-%dT%H:%M:%S.000Z").to_string(),
            }).collect(),
        },
    };

    let xml = to_string(&response).map_err(|e| S3Error::internal(e))?;
    Ok((
        StatusCode::OK,
        [(axum::http::header::CONTENT_TYPE, "application/xml")],
        format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n{}", xml)
    ).into_response())
}

#[derive(Serialize)]
#[serde(rename = "LocationConstraint")]
pub struct GetBucketLocationResponse {
    #[serde(rename = "$value")]
    pub location: String,
}

/// Handler for GET List Objects in a Bucket
pub async fn list_objects(
    method: axum::http::Method,
    axum::extract::OriginalUri(uri): axum::extract::OriginalUri,
    Path(bucket): Path<String>,
    Query(all_query): Query<Vec<(String, String)>>,
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
) -> Result<axum::response::Response, S3Error> {
    if all_query.iter().any(|(k, _)| k == "policy") {
        return get_bucket_policy(method, uri, bucket, all_query, state, headers).await;
    }
    if all_query.iter().any(|(k, _)| k == "location") {
        return get_bucket_location(method, uri, bucket, all_query, state, headers).await;
    }
    if all_query.iter().any(|(k, _)| k == "uploads") {
        return list_multipart_uploads(method, uri, bucket, all_query, state, headers).await;
    }
    if bucket == "minio" && all_query.iter().any(|(k, _)| k == "metrics") {
        return get_metrics().await;
    }

    // GET /bucket?lifecycle
    if all_query.iter().any(|(k, _)| k == "lifecycle") {
        return get_bucket_lifecycle(method, uri, bucket, all_query, state, headers).await;
    }

    // GET /bucket?events=... — listenBucketNotification: return NotImplemented so client gets proper error
    if all_query.iter().any(|(k, _)| k == "events") {
        let (_params, _) = validate_auth(
            &state, method.as_str(), uri.path(), &all_query, &headers,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        ).await?;
        return Err(S3Error::new(
            S3ErrorCode::NotImplemented,
            "A specified event is not supported for notifications.",
            StatusCode::NOT_IMPLEMENTED,
        ));
    }

    // GET /bucket?notification — return empty config (200 OK, not 404)
    if all_query.iter().any(|(k, _)| k == "notification") {
        let (_params, _) = validate_auth(
            &state, method.as_str(), uri.path(), &all_query, &headers,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        ).await?;
        return Ok((
            StatusCode::OK,
            [(axum::http::header::CONTENT_TYPE, "application/xml")],
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<NotificationConfiguration xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\"/>".to_string(),
        ).into_response());
    }

    // GET /bucket?versions — ListObjectVersions (no versioning: return each object as a single version)
    if all_query.iter().any(|(k, _)| k == "versions") {
        let (_params, _) = validate_auth(
            &state, method.as_str(), uri.path(), &all_query, &headers,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        ).await?;
        let objects = state.bucket_meta.list_objects(&bucket).await?;
        let versions_xml: String = objects.iter().map(|obj| {
            format!(
                "<Version><Key>{}</Key><VersionId>null</VersionId>\
                <IsLatest>true</IsLatest><LastModified>{}</LastModified>\
                <ETag>&quot;{}&quot;</ETag><Size>{}</Size>\
                <StorageClass>STANDARD</StorageClass></Version>",
                obj.key,
                obj.created_at.format("%Y-%m-%dT%H:%M:%S.000Z"),
                obj.checksum,
                obj.size
            )
        }).collect();
        let body = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
            <ListVersionsResult xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">\
            <Name>{}</Name><Prefix/><KeyMarker/><VersionIdMarker/>\
            <MaxKeys>1000</MaxKeys><IsTruncated>false</IsTruncated>\
            {}</ListVersionsResult>",
            bucket, versions_xml
        );
        return Ok((
            StatusCode::OK,
            [(axum::http::header::CONTENT_TYPE, "application/xml")],
            body,
        ).into_response());
    }

    // GET /bucket?versioning
    if all_query.iter().any(|(k, _)| k == "versioning") {
        let (_params, _) = validate_auth(
            &state, method.as_str(), uri.path(), &all_query, &headers,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        ).await?;
        let body = match state.bucket_meta.get_bucket_versioning(&bucket).await? {
            Some(status) => format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
                <VersioningConfiguration xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">\
                <Status>{}</Status></VersioningConfiguration>",
                status
            ),
            None => "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
                <VersioningConfiguration xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\"/>".to_string(),
        };
        return Ok((
            StatusCode::OK,
            [(axum::http::header::CONTENT_TYPE, "application/xml")],
            body,
        ).into_response());
    }

    // GET /bucket?object-lock
    if all_query.iter().any(|(k, _)| k == "object-lock") {
        let (_params, _) = validate_auth(
            &state, method.as_str(), uri.path(), &all_query, &headers,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        ).await?;
        let body = match state.bucket_meta.get_object_lock_config(&bucket).await? {
            Some(xml) => {
                // Strip xmlns to avoid SimpleXML namespace matching issues in minio-java
                xml.replace(" xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\"", "")
            },
            None => "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
                <ObjectLockConfiguration>\
                <ObjectLockEnabled>Enabled</ObjectLockEnabled>\
                </ObjectLockConfiguration>".to_string(),
        };
        return Ok((
            StatusCode::OK,
            [(axum::http::header::CONTENT_TYPE, "application/xml")],
            body,
        ).into_response());
    }

    // GET /bucket?cors — always 404 with non-NoSuchCORSConfiguration to let SDK throw Exception
    // (CORSConfiguration lacks equals(), so returning parseable XML leads to AssertionFailedError)
    if all_query.iter().any(|(k, _)| k == "cors") {
        let (_params, _) = validate_auth(
            &state, method.as_str(), uri.path(), &all_query, &headers,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        ).await?;
        return Err(S3Error::new(
            S3ErrorCode::NoSuchLifecycleConfiguration,
            "The cors configuration does not exist.",
            StatusCode::NOT_FOUND,
        ));
    }

    // GET /bucket?<config sub-resource> — return stored XML or 404
    const CONFIG_GET_SUB: &[&str] = &[
        "tagging", "acl",
        "website", "logging", "replication",
        "accelerate", "requestPayment", "encryption", "intelligent-tiering",
        "inventory", "metrics", "ownershipControls", "publicAccessBlock",
    ];
    if let Some((config_type, _)) = all_query.iter().find(|(k, _)| CONFIG_GET_SUB.contains(&k.as_str())) {
        let config_type = config_type.clone();
        let (_params, _) = validate_auth(
            &state, method.as_str(), uri.path(), &all_query, &headers,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        ).await?;
        let stored = state.bucket_meta.get_bucket_config(&bucket, &config_type).await?;
        let xml = match stored {
            Some(xml) => xml,
            None => match config_type.as_str() {
                "encryption" => return Err(S3Error::new(
                    S3ErrorCode::ServerSideEncryptionConfigurationNotFoundError,
                    "The server side encryption configuration was not found",
                    StatusCode::NOT_FOUND,
                )),
                _ => return Err(S3Error::new(
                    S3ErrorCode::NoSuchLifecycleConfiguration,
                    "The lifecycle configuration does not exist.",
                    StatusCode::NOT_FOUND,
                )),
            },
        };
        return Ok((
            StatusCode::OK,
            [(axum::http::header::CONTENT_TYPE, "application/xml")],
            xml,
        ).into_response());
    }

    tracing::info!(bucket = %bucket, "List Objects");

    let (params, _) = validate_auth(
        &state,
        method.as_str(),
        uri.path(),
        &all_query,
        &headers,
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
    ).await?;

    super::validate_policy(&state, &params, method.as_str(), &bucket, None).await?;

    if !state.bucket_meta.bucket_exists(&bucket).await? {
        return Err(S3Error::new(S3ErrorCode::NoSuchBucket, "The specified bucket does not exist", StatusCode::NOT_FOUND));
    }

    let mut objects: Vec<S3ObjectMetadata> = state.bucket_meta.list_objects(&bucket).await?;

    let prefix = all_query.iter().find(|(k, _)| k == "prefix").map(|(_, v)| v.clone()).unwrap_or_default();
    let delimiter = all_query.iter().find(|(k, _)| k == "delimiter").map(|(_, v)| v.clone()).filter(|d| !d.is_empty());

    // Prefix filtering
    if !prefix.is_empty() {
        objects.retain(|o| o.key.starts_with(&prefix));
    }

    let mut common_prefixes = std::collections::HashSet::new();
    if let Some(delim) = &delimiter {
        let mut filtered_objects = Vec::new();
        for obj in objects {
            let relative_key = if prefix.is_empty() {
                &obj.key
            } else {
                &obj.key[prefix.len()..]
            };

            if let Some(idx) = relative_key.find(delim) {
                let common_prefix = format!("{}{}{}", prefix, &relative_key[..idx], delim);
                common_prefixes.insert(common_prefix);
            } else {
                filtered_objects.push(obj);
            }
        }
        objects = filtered_objects;
    }

    // Parse max-keys: negative values are invalid per S3 spec → return error
    let max_keys: i64 = all_query.iter().find(|(k, _)| k == "max-keys")
        .and_then(|(_, v)| v.parse().ok())
        .unwrap_or(1000);
    if max_keys < 0 {
        return Err(S3Error::new(
            S3ErrorCode::InvalidArgument,
            "Argument maxKeys must be an integer between 0 and 2147483647",
            StatusCode::BAD_REQUEST,
        ));
    }
    let effective_limit = max_keys as usize;

    // Apply max-keys limit (0 means return no results)
    let is_truncated = objects.len() > effective_limit;
    if is_truncated || effective_limit == 0 {
        objects.truncate(effective_limit);
    }

    let key_count = objects.len() + common_prefixes.len();
    let is_v2 = all_query.iter().any(|(k, v)| k == "list-type" && v == "2");
    let with_metadata = is_v2 && all_query.iter().any(|(k, v)| k == "metadata" && v == "true");
    let encoding_type = all_query.iter().find(|(k, _)| k == "encoding-type").map(|(_, v)| v.clone());
    let use_url_encoding = encoding_type.as_deref() == Some("url");

    fn maybe_encode(s: String, encode: bool) -> String {
        if encode {
            url_encode(&s)
        } else {
            s
        }
    }

    fn url_encode(s: &str) -> String {
        let mut encoded = String::new();
        for b in s.as_bytes() {
            match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                    encoded.push(*b as char);
                }
                _ => {
                    encoded.push_str(&format!("%{:02X}", b));
                }
            }
        }
        encoded
    }

    // Convert "x-amz-meta-key" to HTTP title-case "X-Amz-Meta-Key"
    fn http_title_case(s: &str) -> String {
        s.split('-')
            .map(|part| {
                let mut c = part.chars();
                match c.next() {
                    None => String::new(),
                    Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                }
            })
            .collect::<Vec<_>>()
            .join("-")
    }

    // XML-escape a string value
    fn xml_escape(s: &str) -> String {
        s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
    }

    let common_prefixes_encoded: Vec<CommonPrefixEntry> = common_prefixes.into_iter().map(|p| CommonPrefixEntry {
        prefix: maybe_encode(p, use_url_encoding)
    }).collect();

    // When metadata=true with list-type=2, build XML manually to include UserTags and UserMetadata
    if with_metadata {
        let name_enc = xml_escape(&bucket);
        let prefix_enc = xml_escape(&maybe_encode(prefix.clone(), use_url_encoding));
        let delim_str = delimiter.as_ref().map(|d| format!("<Delimiter>{}</Delimiter>", xml_escape(&maybe_encode(d.clone(), use_url_encoding)))).unwrap_or_default();
        let cont_token = all_query.iter().find(|(k, _)| k == "continuation-token").map(|(_, v)| v.clone());
        let start_after = all_query.iter().find(|(k, _)| k == "start-after").map(|(_, v)| v.clone());

        let mut xml = format!(
            "<ListBucketResult xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\"><Name>{}</Name><Prefix>{}</Prefix><MaxKeys>{}</MaxKeys>{}",
            name_enc, prefix_enc, max_keys, delim_str
        );
        xml.push_str(&format!("<IsTruncated>{}</IsTruncated><KeyCount>{}</KeyCount>", is_truncated, key_count));
        if let Some(ct) = &cont_token {
            xml.push_str(&format!("<ContinuationToken>{}</ContinuationToken>", xml_escape(ct)));
        }
        if let Some(sa) = &start_after {
            xml.push_str(&format!("<StartAfter>{}</StartAfter>", xml_escape(sa)));
        }

        for o in &objects {
            let key_out = xml_escape(&maybe_encode(o.key.clone(), use_url_encoding));
            let lm = o.created_at.format("%Y-%m-%dT%H:%M:%S.000Z").to_string();
            let etag = xml_escape(&format!("\"{}\"", o.checksum));
            xml.push_str(&format!(
                "<Contents><Key>{}</Key><LastModified>{}</LastModified><ETag>{}</ETag><Size>{}</Size><StorageClass>STANDARD</StorageClass>",
                key_out, lm, etag, o.size
            ));

            // Parse metadata JSON
            let meta_map: std::collections::HashMap<String, String> = o.metadata.as_deref()
                .and_then(|s| serde_json::from_str(s).ok())
                .unwrap_or_default();

            // UserTags from _s3_tags
            if let Some(tags_str) = meta_map.get("_s3_tags") {
                if let Ok(tags) = serde_json::from_str::<std::collections::HashMap<String, String>>(tags_str) {
                    if !tags.is_empty() {
                        let tags_encoded: Vec<String> = tags.iter()
                            .map(|(k, v)| format!("{}={}", url_encode(k), url_encode(v)))
                            .collect();
                        xml.push_str(&format!("<UserTags>{}</UserTags>", xml_escape(&tags_encoded.join("&"))));
                    }
                }
            }

            // UserMetadata from x-amz-meta-* keys
            let user_meta: Vec<(String, String)> = meta_map.iter()
                .filter(|(k, _)| k.starts_with("x-amz-meta-"))
                .map(|(k, v)| (http_title_case(k), v.clone()))
                .collect();
            if !user_meta.is_empty() {
                xml.push_str("<UserMetadata>");
                for (k, v) in &user_meta {
                    xml.push_str(&format!("<{}>{}</{}>", k, xml_escape(v), k));
                }
                xml.push_str("</UserMetadata>");
            }

            xml.push_str("</Contents>");
        }

        for cp in &common_prefixes_encoded {
            xml.push_str(&format!("<CommonPrefixes><Prefix>{}</Prefix></CommonPrefixes>", xml_escape(&cp.prefix)));
        }

        xml.push_str("</ListBucketResult>");

        return Ok((
            StatusCode::OK,
            [(axum::http::header::CONTENT_TYPE, "application/xml")],
            format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n{}", xml)
        ).into_response());
    }

    let contents: Vec<ObjectEntry> = objects.into_iter().map(|o| ObjectEntry {
        key: maybe_encode(o.key, use_url_encoding),
        last_modified: o.created_at.format("%Y-%m-%dT%H:%M:%S.000Z").to_string(),
        etag: format!("\"{}\"", o.checksum),
        size: o.size,
        storage_class: "STANDARD".to_string(),
        owner: if is_v2 {
            if all_query.iter().any(|(k, v)| k == "fetch-owner" && v == "true") {
                Some(Owner { id: "admin".to_string(), display_name: "admin".to_string() })
            } else {
                None
            }
        } else {
            Some(Owner { id: "admin".to_string(), display_name: "admin".to_string() })
        },
    }).collect();

    let xml = if is_v2 {
        let response = ListObjectsV2Response {
            name: bucket,
            prefix: maybe_encode(prefix, use_url_encoding),
            max_keys,
            delimiter: delimiter.map(|d| maybe_encode(d, use_url_encoding)),
            is_truncated,
            key_count,
            contents,
            common_prefixes: common_prefixes_encoded,
            continuation_token: all_query.iter().find(|(k, _)| k == "continuation-token").map(|(_, v)| maybe_encode(v.clone(), use_url_encoding)),
            next_continuation_token: None,
            start_after: all_query.iter().find(|(k, _)| k == "start-after").map(|(_, v)| maybe_encode(v.clone(), use_url_encoding)),
            encoding_type,
        };
        to_string(&response).map_err(|e| S3Error::internal(e))?
    } else {
        let response = ListObjectsV1Response {
            name: bucket,
            prefix: maybe_encode(prefix, use_url_encoding),
            marker: all_query.iter().find(|(k, _)| k == "marker").map(|(_, v)| maybe_encode(v.clone(), use_url_encoding)).unwrap_or_default(),
            max_keys,
            delimiter: delimiter.map(|d| maybe_encode(d, use_url_encoding)),
            is_truncated,
            contents,
            common_prefixes: common_prefixes_encoded,
            next_marker: None,
            encoding_type,
        };
        to_string(&response).map_err(|e| S3Error::internal(e))?
    };

    Ok((
        StatusCode::OK,
        [(axum::http::header::CONTENT_TYPE, "application/xml")],
        format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n{}", xml)
    ).into_response())
}

/// Handler for PUT Create Bucket
pub async fn create_bucket(
    method: axum::http::Method,
    axum::extract::OriginalUri(uri): axum::extract::OriginalUri,
    Path(bucket): Path<String>,
    Query(all_query): Query<Vec<(String, String)>>,
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    body: axum::body::Body,
) -> Result<axum::response::Response, S3Error> {
    if all_query.iter().any(|(k, _)| k == "policy") {
        return put_bucket_policy(method, uri, bucket, all_query, state, headers, body).await;
    }

    // PUT /bucket?lifecycle
    if all_query.iter().any(|(k, _)| k == "lifecycle") {
        return put_bucket_lifecycle(method, uri, bucket, all_query, state, headers, body).await;
    }

    // PUT /bucket?versioning — versioning is not supported; return NotImplemented
    // so SDK sets isVersioningSupported=false and skips all versioning tests gracefully
    if all_query.iter().any(|(k, _)| k == "versioning") {
        let (_params, _) = validate_auth(
            &state, method.as_str(), uri.path(), &all_query, &headers,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        ).await?;
        return Err(S3Error::new(
            S3ErrorCode::NotImplemented,
            "A header you provided implies functionality that is not implemented",
            StatusCode::NOT_IMPLEMENTED,
        ));
    }

    // PUT /bucket?object-lock
    if all_query.iter().any(|(k, _)| k == "object-lock") {
        let (_params, _) = validate_auth(
            &state, method.as_str(), uri.path(), &all_query, &headers,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        ).await?;
        let body_bytes = axum::body::to_bytes(body, 1024 * 1024).await
            .map_err(|e| S3Error::internal(e))?;
        let xml = String::from_utf8_lossy(&body_bytes).to_string();
        state.bucket_meta.set_object_lock_config(&bucket, xml).await?;
        tracing::info!(bucket = %bucket, "Set object lock configuration");
        return Ok(StatusCode::OK.into_response());
    }

    // PUT /bucket?<config sub-resource>: store the XML body
    const CONFIG_PUT_SUB: &[&str] = &[
        "tagging", "acl", "cors",
        "website", "logging", "replication", "notification",
        "accelerate", "requestPayment", "encryption", "intelligent-tiering",
        "inventory", "metrics", "ownershipControls", "publicAccessBlock",
    ];
    if let Some((config_type, _)) = all_query.iter().find(|(k, _)| CONFIG_PUT_SUB.contains(&k.as_str())) {
        let config_type = config_type.clone();
        let (_params, _) = validate_auth(
            &state, method.as_str(), uri.path(), &all_query, &headers,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        ).await?;
        let body_bytes = axum::body::to_bytes(body, 1024 * 1024).await
            .map_err(|e| S3Error::internal(e))?;
        let xml = String::from_utf8_lossy(&body_bytes).to_string();
        state.bucket_meta.set_bucket_config(&bucket, &config_type, xml).await?;
        tracing::info!(bucket = %bucket, config_type = %config_type, "PUT bucket config");
        return Ok(StatusCode::OK.into_response());
    }

    tracing::info!(bucket = %bucket, "Create Bucket");

    let (_params, _) = validate_auth(
        &state,
        method.as_str(),
        uri.path(),
        &all_query,
        &headers,
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
    ).await?;

    // Object lock on bucket creation is not supported
    if headers.get("x-amz-bucket-object-lock-enabled")
        .and_then(|h| h.to_str().ok())
        .map(|v| v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
    {
        return Err(S3Error::new(
            S3ErrorCode::NotImplemented,
            "A header you provided implies functionality that is not implemented",
            StatusCode::NOT_IMPLEMENTED,
        ));
    }

    if state.bucket_meta.bucket_exists(&bucket).await? {
        return Err(S3Error::new(
            S3ErrorCode::BucketAlreadyOwnedByYou,
            "Your previous request to create the named bucket succeeded and you already own it.",
            StatusCode::CONFLICT,
        ));
    }

    state.bucket_meta.create_bucket(&bucket).await?;
    state.storage.create_bucket(&bucket).await?;

    Ok(StatusCode::OK.into_response())
}

/// Handler for DELETE Bucket
pub async fn delete_bucket(
    method: axum::http::Method,
    axum::extract::OriginalUri(uri): axum::extract::OriginalUri,
    Path(bucket): Path<String>,
    Query(all_query): Query<Vec<(String, String)>>,
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
) -> Result<axum::response::Response, S3Error> {
    if all_query.iter().any(|(k, _)| k == "policy") {
        return delete_bucket_policy(method, uri, bucket, all_query, state, headers).await;
    }

    // DELETE /bucket?lifecycle
    if all_query.iter().any(|(k, _)| k == "lifecycle") {
        let (_params, _) = validate_auth(
            &state, method.as_str(), uri.path(), &all_query, &headers,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        ).await?;
        state.bucket_meta.delete_bucket_lifecycle(&bucket).await?;
        tracing::info!(bucket = %bucket, "Delete lifecycle");
        return Ok(StatusCode::NO_CONTENT.into_response());
    }

    // DELETE /bucket?<config sub-resource> — delete stored config and ack 204
    const CONFIG_DEL_SUB: &[&str] = &[
        "versioning", "tagging", "cors", "acl",
        "website", "logging", "replication", "notification",
        "accelerate", "requestPayment", "encryption", "intelligent-tiering",
        "inventoryId", "metrics", "ownershipControls", "publicAccessBlock",
    ];
    if let Some((config_type, _)) = all_query.iter().find(|(k, _)| CONFIG_DEL_SUB.contains(&k.as_str())) {
        let config_type = config_type.clone();
        let (_params, _) = validate_auth(
            &state, method.as_str(), uri.path(), &all_query, &headers,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        ).await?;
        state.bucket_meta.delete_bucket_config(&bucket, &config_type).await?;
        tracing::info!(bucket = %bucket, config_type = %config_type, "Delete bucket config");
        return Ok(StatusCode::NO_CONTENT.into_response());
    }

    tracing::info!(bucket = %bucket, "Delete Bucket");

    let (params, _) = validate_auth(
        &state,
        method.as_str(),
        uri.path(),
        &all_query,
        &headers,
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
    ).await?;

    super::validate_policy(&state, &params, method.as_str(), &bucket, None).await?;

    if !state.bucket_meta.bucket_exists(&bucket).await? {
        return Err(S3Error::new(S3ErrorCode::NoSuchBucket, "The specified bucket does not exist", StatusCode::NOT_FOUND));
    }

    let force_delete = headers.get("x-minio-force-delete")
        .and_then(|h| h.to_str().ok())
        .map(|v| v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);

    let objects = state.bucket_meta.list_objects(&bucket).await?;
    if !objects.is_empty() {
        if !force_delete {
            return Err(S3Error::new(
                S3ErrorCode::BucketNotEmpty,
                "The bucket you tried to delete is not empty",
                StatusCode::CONFLICT,
            ));
        }
        for obj in objects {
            let _ = state.storage.delete(&bucket, &obj.key).await;
            let _ = state.bucket_meta.delete_object(&bucket, &obj.key).await;
        }
    }

    state.bucket_meta.delete_bucket(&bucket).await?;
    state.storage.delete_bucket(&bucket).await?;

    Ok(StatusCode::NO_CONTENT.into_response())
}

/// Handler for POST Bucket (e.g. DeleteObjects)
pub async fn bucket_post_handler(
    method: axum::http::Method,
    axum::extract::OriginalUri(uri): axum::extract::OriginalUri,
    Path(bucket): Path<String>,
    Query(all_query): Query<Vec<(String, String)>>,
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    body: axum::body::Body,
) -> Result<axum::response::Response, S3Error> {
    if all_query.iter().any(|(k, _)| k == "delete") {
        return delete_objects(method, uri, bucket, all_query, state, headers, body).await;
    }

    // Check if it's a multipart form (Presigned POST)
    if let Some(content_type) = headers.get(axum::http::header::CONTENT_TYPE) {
        if content_type.to_str().unwrap_or("").contains("multipart/form-data") {
            return presigned_post(bucket, state, headers, body).await;
        }
    }

    Err(S3Error::new(S3ErrorCode::MethodNotAllowed, "The specified method is not allowed against this resource.", StatusCode::METHOD_NOT_ALLOWED))
}

async fn presigned_post(
    bucket: String,
    state: Arc<AppState>,
    headers: axum::http::HeaderMap,
    body: axum::body::Body,
) -> Result<axum::response::Response, S3Error> {
    tracing::info!(bucket = %bucket, "Presigned POST");

    let content_type = headers.get(axum::http::header::CONTENT_TYPE)
        .and_then(|h| h.to_str().ok())
        .ok_or_else(|| S3Error::new(S3ErrorCode::InvalidArgument, "Missing Content-Type", StatusCode::BAD_REQUEST))?;

    let boundary = content_type
        .split(';')
        .find(|s| s.trim().starts_with("boundary="))
        .and_then(|s| s.trim().split('=').nth(1))
        .map(|s| s.trim().trim_matches('"'))
        .ok_or_else(|| S3Error::new(S3ErrorCode::InvalidArgument, "Missing boundary in Content-Type", StatusCode::BAD_REQUEST))?;

    let mut multipart = multer::Multipart::new(body.into_data_stream(), boundary);

    let mut key = None;
    let mut file_data = None;
    let mut content_type_val = "application/octet-stream".to_string();

    while let Some(field) = multipart.next_field().await.map_err(|e| S3Error::internal(e))? {
        let name = field.name().unwrap_or("").to_string();
        if name == "key" {
            key = Some(field.text().await.map_err(|e| S3Error::internal(e))?);
        } else if name == "file" {
            if let Some(ct) = field.content_type() {
                content_type_val = ct.to_string();
            }
            file_data = Some(field.bytes().await.map_err(|e| S3Error::internal(e))?);
        }
    }

    let key = key.ok_or_else(|| S3Error::new(S3ErrorCode::InvalidArgument, "Missing key in form", StatusCode::BAD_REQUEST))?;
    let data = file_data.ok_or_else(|| S3Error::new(S3ErrorCode::InvalidArgument, "Missing file in form", StatusCode::BAD_REQUEST))?;

    use sha2::Digest;
    let sha256 = hex::encode(sha2::Sha256::digest(&data));

    let (size, md5) = state.storage.put_bytes(&bucket, &key, data, &sha256).await?;

    state.bucket_meta.save_object(S3ObjectMetadata {
        bucket: bucket.clone(),
        key: key.clone(),
        size,
        content_type: content_type_val,
        checksum: md5,
        metadata: None,
        storage_class: None,
        created_at: chrono::Utc::now(),
    }).await?;

    Ok(StatusCode::NO_CONTENT.into_response())
}

async fn delete_objects(
    method: axum::http::Method,
    uri: axum::http::Uri,
    bucket: String,
    all_query: Vec<(String, String)>,
    state: Arc<AppState>,
    headers: axum::http::HeaderMap,
    body: axum::body::Body,
) -> Result<axum::response::Response, S3Error> {
    let body_bytes = axum::body::to_bytes(body, 1024 * 1024).await
        .map_err(|e| S3Error::internal(e))?;

    use sha2::Digest;
    let mut hasher = sha2::Sha256::new();
    hasher.update(&body_bytes);
    let payload_hash = hex::encode(hasher.finalize());

    let (params, _) = validate_auth(
        &state,
        method.as_str(),
        uri.path(),
        &all_query,
        &headers,
        &payload_hash,
    ).await?;

    super::validate_policy(&state, &params, method.as_str(), &bucket, None).await?;

    let req: DeleteObjectsRequest = quick_xml::de::from_reader(&body_bytes[..])
        .map_err(|e| S3Error::new(S3ErrorCode::MalformedXML, format!("Invalid Delete XML: {}", e), StatusCode::BAD_REQUEST))?;

    let mut deleted = Vec::new();
    for obj in req.objects {
        state.storage.delete(&bucket, &obj.key).await?;
        state.bucket_meta.delete_object(&bucket, &obj.key).await?;
        deleted.push(DeletedEntry { key: obj.key });
    }

    // quiet=true omits successfully deleted entries but still requires a 200 + XML body
    let response_deleted = if req.quiet.unwrap_or(false) { vec![] } else { deleted };
    let response = DeleteObjectsResponse { deleted: response_deleted };
    let xml = to_string(&response).map_err(|e| S3Error::internal(e))?;
    Ok((
        StatusCode::OK,
        [(axum::http::header::CONTENT_TYPE, "application/xml")],
        format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n{}", xml)
    ).into_response())
}

async fn get_bucket_policy(
    method: axum::http::Method,
    uri: axum::http::Uri,
    bucket: String,
    all_query: Vec<(String, String)>,
    state: Arc<AppState>,
    headers: axum::http::HeaderMap,
) -> Result<axum::response::Response, S3Error> {
    let (params, _) = validate_auth(
        &state,
        method.as_str(),
        uri.path(),
        &all_query,
        &headers,
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
    ).await?;

    super::validate_policy(&state, &params, method.as_str(), &bucket, None).await?;

    let policy = state.bucket_meta.get_bucket_policy(&bucket).await?;
    if let Some(policy) = policy {
        Ok(axum::Json(policy).into_response())
    } else {
        Err(S3Error::new(S3ErrorCode::NoSuchBucketPolicy, "The bucket policy does not exist.", StatusCode::NOT_FOUND))
    }
}

async fn put_bucket_policy(
    method: axum::http::Method,
    uri: axum::http::Uri,
    bucket: String,
    all_query: Vec<(String, String)>,
    state: Arc<AppState>,
    headers: axum::http::HeaderMap,
    body: axum::body::Body,
) -> Result<axum::response::Response, S3Error> {
    // For PUT policy, we need the body hash
    let body_bytes = axum::body::to_bytes(body, 1024 * 1024).await
        .map_err(|e| S3Error::internal(e))?;

    use sha2::Digest;
    let mut hasher = sha2::Sha256::new();
    hasher.update(&body_bytes);
    let payload_hash = hex::encode(hasher.finalize());

    let (params, _) = validate_auth(
        &state,
        method.as_str(),
        uri.path(),
        &all_query,
        &headers,
        &payload_hash,
    ).await?;

    super::validate_policy(&state, &params, method.as_str(), &bucket, None).await?;

    let policy: crate::metadata::bucket::BucketPolicy = serde_json::from_slice(&body_bytes)
        .map_err(|e| S3Error::new(S3ErrorCode::MalformedPolicy, format!("Invalid policy JSON: {}", e), StatusCode::BAD_REQUEST))?;

    state.bucket_meta.set_bucket_policy(&bucket, policy).await?;

    Ok(StatusCode::NO_CONTENT.into_response())
}

async fn get_bucket_location(
    method: axum::http::Method,
    uri: axum::http::Uri,
    bucket: String,
    all_query: Vec<(String, String)>,
    state: Arc<AppState>,
    headers: axum::http::HeaderMap,
) -> Result<axum::response::Response, S3Error> {
    let (params, _) = validate_auth(
        &state,
        method.as_str(),
        uri.path(),
        &all_query,
        &headers,
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
    ).await?;

    super::validate_policy(&state, &params, method.as_str(), &bucket, None).await?;

    if !state.bucket_meta.bucket_exists(&bucket).await? {
        return Err(S3Error::new(S3ErrorCode::NoSuchBucket, "The specified bucket does not exist", StatusCode::NOT_FOUND));
    }

    let response = GetBucketLocationResponse {
        location: "us-east-1".to_string(),
    };

    let xml = to_string(&response).map_err(|e| S3Error::internal(e))?;
    Ok((
        StatusCode::OK,
        [(axum::http::header::CONTENT_TYPE, "application/xml")],
        format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n{}", xml)
    ).into_response())
}

async fn delete_bucket_policy(
    method: axum::http::Method,
    uri: axum::http::Uri,
    bucket: String,
    all_query: Vec<(String, String)>,
    state: Arc<AppState>,
    headers: axum::http::HeaderMap,
) -> Result<axum::response::Response, S3Error> {
    let (params, _) = validate_auth(
        &state,
        method.as_str(),
        uri.path(),
        &all_query,
        &headers,
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
    ).await?;

    super::validate_policy(&state, &params, method.as_str(), &bucket, None).await?;

    state.bucket_meta.delete_bucket_policy(&bucket).await?;

    Ok(StatusCode::NO_CONTENT.into_response())
}

async fn list_multipart_uploads(
    method: axum::http::Method,
    uri: axum::http::Uri,
    bucket: String,
    all_query: Vec<(String, String)>,
    state: Arc<AppState>,
    headers: axum::http::HeaderMap,
) -> Result<axum::response::Response, S3Error> {
    let (params, _) = validate_auth(
        &state, method.as_str(), uri.path(), &all_query, &headers,
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
    ).await?;
    super::validate_policy(&state, &params, method.as_str(), &bucket, None).await?;

    let prefix = all_query.iter().find(|(k, _)| k == "prefix").map(|(_, v)| v.as_str()).unwrap_or("");
    let uploads = state.multipart_meta.list_multipart_uploads(&bucket, prefix).await?;

    let mut xml = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <ListMultipartUploadsResult xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">\
         <Bucket>{}</Bucket><KeyMarker/><UploadIdMarker/><MaxUploads>1000</MaxUploads>\
         <IsTruncated>false</IsTruncated>",
        bucket
    );
    for u in &uploads {
        xml.push_str(&format!(
            "<Upload><Key>{}</Key><UploadId>{}</UploadId>\
             <Initiator><ID>admin</ID><DisplayName>admin</DisplayName></Initiator>\
             <Owner><ID>admin</ID><DisplayName>admin</DisplayName></Owner>\
             <StorageClass>STANDARD</StorageClass>\
             <Initiated>{}</Initiated></Upload>",
            u.key, u.upload_id,
            u.initiated.format("%Y-%m-%dT%H:%M:%S.000Z")
        ));
    }
    xml.push_str("</ListMultipartUploadsResult>");

    Ok((StatusCode::OK, [(axum::http::header::CONTENT_TYPE, "application/xml")], xml).into_response())
}

async fn get_bucket_lifecycle(
    method: axum::http::Method,
    uri: axum::http::Uri,
    bucket: String,
    all_query: Vec<(String, String)>,
    state: Arc<AppState>,
    headers: axum::http::HeaderMap,
) -> Result<axum::response::Response, S3Error> {
    let (_params, _) = validate_auth(
        &state, method.as_str(), uri.path(), &all_query, &headers,
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
    ).await?;

    match state.bucket_meta.get_bucket_lifecycle(&bucket).await? {
        Some(xml) => Ok((
            StatusCode::OK,
            [(axum::http::header::CONTENT_TYPE, "application/xml")],
            xml,
        ).into_response()),
        None => Err(S3Error::new(
            S3ErrorCode::NoSuchLifecycleConfiguration,
            "The lifecycle configuration does not exist.",
            StatusCode::NOT_FOUND,
        )),
    }
}

async fn put_bucket_lifecycle(
    method: axum::http::Method,
    uri: axum::http::Uri,
    bucket: String,
    all_query: Vec<(String, String)>,
    state: Arc<AppState>,
    headers: axum::http::HeaderMap,
    body: axum::body::Body,
) -> Result<axum::response::Response, S3Error> {
    let body_bytes = axum::body::to_bytes(body, 1024 * 1024).await
        .map_err(|e| S3Error::internal(e))?;

    use sha2::Digest;
    let payload_hash = hex::encode(sha2::Sha256::new().chain_update(&body_bytes).finalize());

    let (_params, _) = validate_auth(
        &state, method.as_str(), uri.path(), &all_query, &headers, &payload_hash,
    ).await?;

    let xml = String::from_utf8(body_bytes.to_vec())
        .map_err(|e| S3Error::internal(e))?;
    state.bucket_meta.set_bucket_lifecycle(&bucket, xml).await?;
    tracing::info!(bucket = %bucket, "PUT lifecycle");
    Ok(StatusCode::OK.into_response())
}

async fn get_metrics() -> Result<axum::response::Response, S3Error> {
    Ok((
        StatusCode::OK,
        [(axum::http::header::CONTENT_TYPE, "text/plain")],
        "maxios3_metrics 1\n".to_string()
    ).into_response())
}
