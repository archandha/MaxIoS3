// Tests for SigV4 authorization header and pre-signed URL query-string parsing.

use std::collections::HashMap;
use crate::auth::{SigV4Params};

// ── Header parsing ────────────────────────────────────────────────────────────

#[test]
fn test_parse_valid_header() {
    let header = "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7/20130524/us-east-1/s3/aws4_request,SignedHeaders=host;x-amz-date,Signature=abcd1234";
    let params = SigV4Params::parse(header).unwrap();
    assert_eq!(params.access_key, "AKIAIOSFODNN7");
    assert_eq!(params.credential_scope, "20130524/us-east-1/s3/aws4_request");
    assert_eq!(params.signed_headers, vec!["host", "x-amz-date"]);
    assert_eq!(params.signature, "abcd1234");
    assert!(!params.is_presigned);
}

#[test]
fn test_parse_header_wrong_algorithm() {
    let header = "AWS AKIAIOSFODNN7:signature";
    assert!(SigV4Params::parse(header).is_err());
}

#[test]
fn test_parse_header_missing_credential() {
    let header = "AWS4-HMAC-SHA256 SignedHeaders=host,Signature=abcd";
    assert!(SigV4Params::parse(header).is_err());
}

#[test]
fn test_parse_header_missing_signature() {
    let header = "AWS4-HMAC-SHA256 Credential=KEY/scope,SignedHeaders=host";
    assert!(SigV4Params::parse(header).is_err());
}

#[test]
fn test_parse_header_missing_signed_headers() {
    let header = "AWS4-HMAC-SHA256 Credential=KEY/scope,Signature=abcd";
    assert!(SigV4Params::parse(header).is_err());
}

// ── Query-string (pre-signed URL) parsing ────────────────────────────────────

fn base_presign_query() -> HashMap<String, String> {
    let mut m = HashMap::new();
    m.insert("X-Amz-Algorithm".into(), "AWS4-HMAC-SHA256".into());
    m.insert("X-Amz-Credential".into(), "AKIAIOSFODNN7/20130524/us-east-1/s3/aws4_request".into());
    m.insert("X-Amz-Date".into(), "20130524T000000Z".into());
    m.insert("X-Amz-Expires".into(), "3600".into());
    m.insert("X-Amz-SignedHeaders".into(), "host".into());
    m.insert("X-Amz-Signature".into(), "deadbeef".into());
    m
}

#[test]
fn test_parse_query_valid() {
    let query = base_presign_query();
    let params = SigV4Params::parse_query(&query).unwrap();
    assert_eq!(params.access_key, "AKIAIOSFODNN7");
    assert_eq!(params.expires, Some(3600));
    assert_eq!(params.amz_date.as_deref(), Some("20130524T000000Z"));
    assert!(params.is_presigned);
}

#[test]
fn test_parse_query_missing_expires() {
    let mut query = base_presign_query();
    query.remove("X-Amz-Expires");
    assert!(SigV4Params::parse_query(&query).is_err());
}

#[test]
fn test_parse_query_invalid_expires() {
    let mut query = base_presign_query();
    query.insert("X-Amz-Expires".into(), "not-a-number".into());
    assert!(SigV4Params::parse_query(&query).is_err());
}

#[test]
fn test_parse_query_wrong_algorithm() {
    let mut query = base_presign_query();
    query.insert("X-Amz-Algorithm".into(), "AWS-HMAC-MD5".into());
    assert!(SigV4Params::parse_query(&query).is_err());
}

#[test]
fn test_parse_query_missing_date() {
    let mut query = base_presign_query();
    query.remove("X-Amz-Date");
    assert!(SigV4Params::parse_query(&query).is_err());
}
