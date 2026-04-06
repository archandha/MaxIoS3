/// Metadata management for the MaxIOs3 server.
/// This module defines the core traits and data structures for managing bucket, object, 
/// multipart upload, and user metadata. It provides a unified interface for 
/// different metadata backends, with SQLite being the default implementation.

pub mod bucket;
pub mod multipart;
pub mod user;
pub mod sqlite;
pub mod postgres;

pub use bucket::{BucketMetadataEngine, S3BucketMetadata, S3ObjectMetadata};
pub use multipart::{MultipartMetadataEngine, MultipartPart, MultipartUpload};
pub use user::{UserMetadataEngine, S3User};
pub use sqlite::SqliteMetadata;
pub use postgres::PostgresMetadata;
