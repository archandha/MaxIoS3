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
}

#[async_trait]
pub trait MultipartMetadataEngine: Send + Sync {
    async fn initiate_multipart(&self, bucket: &str, key: &str) -> Result<String>;
    async fn save_part(&self, upload_id: &str, part: MultipartPart) -> Result<()>;
    async fn get_multipart(&self, upload_id: &str) -> Result<Option<MultipartUpload>>;
    async fn complete_multipart(&self, upload_id: &str) -> Result<()>;
    async fn find_stale_multipart_uploads(&self, older_than_seconds: u64) -> Result<Vec<String>>;
}
