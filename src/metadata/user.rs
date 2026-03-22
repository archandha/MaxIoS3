/// User metadata definitions for the MaxIOs3 server.
/// This file defines the `UserMetadataEngine` trait and the `S3User` data structure.
/// It provides the interface for retrieving and creating user accounts, 
/// which are essential for SigV4 authentication and access control.

use async_trait::async_trait;
use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Serialize, Deserialize};

#[derive(Debug, Serialize, Deserialize, Clone, sqlx::FromRow)]
pub struct S3User {
    pub access_key: String,
    pub secret_key: String,
    pub user_id: String,
    pub is_active: bool,
    pub created_at: DateTime<Utc>,
}

#[async_trait]
pub trait UserMetadataEngine: Send + Sync {
    async fn get_user(&self, access_key: &str) -> Result<Option<S3User>>;
    async fn create_user(&self, access_key: &str, secret_key: &str, user_id: &str) -> Result<()>;
}
