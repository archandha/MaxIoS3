/// Multipart upload metadata definitions for the MaxIOs3 server.
/// This file defines the `MultipartMetadataEngine` trait and associated data structures.
/// It includes definitions for multipart parts and upload sessions, 
/// as well as methods for initiating, saving parts, and completing uploads.

use async_trait::async_trait;
use anyhow::Result;
use serde::{Serialize, Deserialize};

#[derive(Debug, Serialize, Deserialize, Clone, sqlx::FromRow)]
pub struct MultipartPart {
    pub part_number: u32,
    pub checksum: String,
    pub size: u64,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct MultipartUpload {
    pub upload_id: String,
    pub bucket: String,
    pub key: String,
    pub parts: Vec<MultipartPart>,
    pub metadata: Option<String>,
    pub content_type: Option<String>,
    pub storage_class: Option<String>,
}

#[derive(Debug, Clone)]
pub struct MultipartUploadSummary {
    pub upload_id: String,
    pub bucket: String,
    pub key: String,
    pub initiated: chrono::DateTime<chrono::Utc>,
}

#[async_trait]
pub trait MultipartMetadataEngine: Send + Sync {
    async fn initiate_multipart(&self, bucket: &str, key: &str, metadata: Option<String>, content_type: Option<String>, storage_class: Option<String>) -> Result<String>;
    async fn save_part(&self, upload_id: &str, part: MultipartPart) -> Result<()>;
    async fn get_multipart(&self, upload_id: &str) -> Result<Option<MultipartUpload>>;
    async fn complete_multipart(&self, upload_id: &str) -> Result<()>;
    async fn find_stale_multipart_uploads(&self, older_than_seconds: u64) -> Result<Vec<String>>;
    async fn list_multipart_uploads(&self, bucket: &str, prefix: &str) -> Result<Vec<MultipartUploadSummary>>;
}
