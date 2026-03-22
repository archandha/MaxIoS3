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
    MalformedPolicy,
    XAmzContentSHA256Mismatch,
    MethodNotAllowed,
    MalformedXML,
    InvalidArgument,
    NotImplemented,
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
