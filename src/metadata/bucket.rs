/// Bucket and object metadata definitions for the MaxIOs3 server.
/// This file defines the `BucketMetadataEngine` trait and associated data structures.
/// It includes definitions for bucket and object metadata, as well as
/// the structures required for S3-compatible bucket policies.

use async_trait::async_trait;
use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Serialize, Deserialize};

#[derive(Debug, Serialize, Deserialize, Clone, sqlx::FromRow)]
pub struct S3BucketMetadata {
    pub name: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, Deserialize, Clone, sqlx::FromRow)]
pub struct S3ObjectMetadata {
    pub bucket: String,
    pub key: String,
    pub size: u64,
    pub content_type: String,
    pub checksum: String,
    pub metadata: Option<String>,
    pub storage_class: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct BucketPolicy {
    #[serde(rename = "Version")]
    pub version: String,
    #[serde(rename = "Statement")]
    pub statements: Vec<PolicyStatement>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PolicyStatement {
    #[serde(rename = "Sid", skip_serializing_if = "Option::is_none")]
    pub sid: Option<String>,
    #[serde(rename = "Effect")]
    pub effect: String, // "Allow" or "Deny"
    #[serde(rename = "Principal")]
    pub principal: serde_json::Value,
    #[serde(rename = "Action")]
    pub action: serde_json::Value, // Can be String or Vec<String>
    #[serde(rename = "Resource")]
    pub resource: serde_json::Value, // Can be String or Vec<String>
}

#[async_trait]
pub trait BucketMetadataEngine: Send + Sync {
    async fn create_bucket(&self, name: &str) -> Result<()>;
    async fn delete_bucket(&self, name: &str) -> Result<()>;
    async fn list_buckets(&self) -> Result<Vec<S3BucketMetadata>>;
    async fn bucket_exists(&self, name: &str) -> Result<bool>;
    async fn save_object(&self, meta: S3ObjectMetadata) -> Result<()>;
    async fn get_object(&self, bucket: &str, key: &str) -> Result<Option<S3ObjectMetadata>>;
    async fn list_objects(&self, bucket: &str) -> Result<Vec<S3ObjectMetadata>>;
    async fn delete_object(&self, bucket: &str, key: &str) -> Result<()>;

    // Bucket Policy methods
    async fn get_bucket_policy(&self, bucket: &str) -> Result<Option<BucketPolicy>>;
    async fn set_bucket_policy(&self, bucket: &str, policy: BucketPolicy) -> Result<()>;
    async fn delete_bucket_policy(&self, bucket: &str) -> Result<()>;

    // Bucket Lifecycle methods (stored as raw XML)
    async fn get_bucket_lifecycle(&self, bucket: &str) -> Result<Option<String>>;
    async fn set_bucket_lifecycle(&self, bucket: &str, lifecycle_xml: String) -> Result<()>;
    async fn delete_bucket_lifecycle(&self, bucket: &str) -> Result<()>;
}
