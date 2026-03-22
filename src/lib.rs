/// Library entry point for the MaxIOs3 project.
/// It defines the core modules and exports key types used throughout the application.
/// This file also contains the `AppState` struct, which is shared across all request handlers.

pub mod error;
pub mod metadata;
pub mod storage;
pub mod auth;
pub mod handlers;
pub mod config;

pub use error::{S3Error, S3ErrorCode};
pub use metadata::{
    BucketMetadataEngine, MultipartMetadataEngine, UserMetadataEngine,
    SqliteMetadata, S3BucketMetadata, S3ObjectMetadata, MultipartPart, S3User,
    MultipartUpload
};
pub use storage::{FileSystemStorage, StorageEngine};
pub use auth::{SignatureValidator, SigV4Params};
pub use config::Config;

/// Global application state shared across all request handlers
pub struct AppState {
    pub storage: std::sync::Arc<dyn StorageEngine>,
    pub bucket_meta: std::sync::Arc<dyn BucketMetadataEngine>,
    pub multipart_meta: std::sync::Arc<dyn MultipartMetadataEngine>,
    pub user_meta: std::sync::Arc<dyn UserMetadataEngine>,
    pub auth: SignatureValidator,
}
