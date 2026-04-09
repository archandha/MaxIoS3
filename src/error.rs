/// Error handling for the MaxIOs3 server.
/// This file defines the `S3Error` and `S3ErrorCode` enums for S3-compatible error responses.
/// It implements `IntoResponse` for `S3Error`, allowing it to be returned directly from Axum handlers.
/// Errors are serialized into the XML format expected by S3 clients.

use axum::{
    http::{header, StatusCode},
    response::{IntoResponse, Response},
};
use serde::Serialize;
use thiserror::Error;
use uuid::Uuid;
use quick_xml::se::to_string;

/// Standard S3 Error Codes
#[derive(Debug, Serialize, Clone, Copy)]
pub enum S3ErrorCode {
    NoSuchKey,
    NoSuchBucket,
    NoSuchUpload,
    InvalidBucketName,
    InvalidKey,
    SignatureDoesNotMatch,
    BadDigest,
    InternalError,
    AccessDenied,
    IncompleteBody,
    EntityTooSmall,
    EntityTooLarge,
    NoSuchBucketPolicy,
    NoSuchLifecycleConfiguration,
    ServerSideEncryptionConfigurationNotFoundError,
    MalformedPolicy,
    XAmzContentSHA256Mismatch,
    InvalidDigest,
    MethodNotAllowed,
    MalformedXML,
    InvalidArgument,
    NotImplemented,
    BucketNotEmpty,
    PreconditionFailed,
    BucketAlreadyOwnedByYou,
    InvalidTag,
    BadRequest,
}

/// Custom Error type for our S3 Server
#[derive(Debug, Error)]
pub enum S3Error {
    /// A structured S3 error with a code, message, and HTTP status
    #[error("S3 Error: {0:?} - {1}")]
    S3(S3ErrorCode, String, StatusCode),

    /// An internal error that we wrap using anyhow
    #[error(transparent)]
    Internal(#[from] anyhow::Error),
}

/// The XML structure returned to the client
#[derive(Serialize)]
#[serde(rename = "Error")]
#[serde(rename_all = "PascalCase")]
struct S3ErrorResponse {
    code: String,
    message: String,
    resource: Option<String>,
    request_id: String,
}

impl S3Error {
    /// Helper to create a new S3 error
    pub fn new(code: S3ErrorCode, message: impl Into<String>, status: StatusCode) -> Self {
        S3Error::S3(code, message.into(), status)
    }

    /// Helper to create an internal error from any error type
    pub fn internal(err: impl Into<anyhow::Error>) -> Self {
        S3Error::Internal(err.into())
    }
}

/// This implementation allows Axum to return S3Error directly from route handlers
impl IntoResponse for S3Error {
    fn into_response(self) -> Response {
        let request_id = Uuid::new_v4().to_string();

        let (status, code, message) = match self {
            S3Error::S3(code, msg, status) => (status, format!("{:?}", code), msg),
            S3Error::Internal(err) => {
                // Log the actual internal error for debugging
                tracing::error!(error = ?err, "Internal server error");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "InternalError".to_string(),
                    "An internal error occurred.".to_string(),
                )
            }
        };

        let response = S3ErrorResponse {
            code,
            message,
            resource: None,
            request_id,
        };

        // Serialize to XML
        let xml_body = match to_string(&response) {
            Ok(xml) => format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n{}", xml),
            Err(_) => "Internal Server Error".to_string(),
        };

        (
            status,
            [(header::CONTENT_TYPE, "application/xml")],
            xml_body,
        ).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::response::IntoResponse;

    #[test]
    fn test_no_such_bucket_xml_contains_code() {
        let err = S3Error::new(S3ErrorCode::NoSuchBucket, "The bucket does not exist", StatusCode::NOT_FOUND);
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX);
        let bytes = tokio::runtime::Runtime::new().unwrap().block_on(body).unwrap();
        let xml = std::str::from_utf8(&bytes).unwrap();
        assert!(xml.contains("<Code>NoSuchBucket</Code>"), "XML was: {}", xml);
        assert!(xml.contains("<Message>The bucket does not exist</Message>"), "XML was: {}", xml);
    }

    #[test]
    fn test_internal_error_returns_500() {
        let err = S3Error::internal(anyhow::anyhow!("something broke"));
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[test]
    fn test_access_denied_returns_403() {
        let err = S3Error::new(S3ErrorCode::AccessDenied, "Forbidden", StatusCode::FORBIDDEN);
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[test]
    fn test_response_content_type_is_xml() {
        let err = S3Error::new(S3ErrorCode::NoSuchKey, "Not found", StatusCode::NOT_FOUND);
        let response = err.into_response();
        let ct = response.headers().get(axum::http::header::CONTENT_TYPE).unwrap();
        assert_eq!(ct, "application/xml");
    }
}
