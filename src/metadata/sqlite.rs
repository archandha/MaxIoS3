/// SQLite implementation of the metadata engines for the MaxIOs3 server.
/// This file contains the `SqliteMetadata` struct, which implements the
/// `BucketMetadataEngine`, `MultipartMetadataEngine`, and `UserMetadataEngine` traits.
/// It handles all database operations, including table initialization and
/// CRUD operations for buckets, objects, policies, multipart uploads, and users.

use async_trait::async_trait;
use anyhow::Result;
use sqlx::{SqlitePool, Row, sqlite::SqliteConnectOptions};
use std::str::FromStr;
use crate::metadata::bucket::{BucketMetadataEngine, S3BucketMetadata, S3ObjectMetadata};
use crate::metadata::multipart::{MultipartMetadataEngine, MultipartPart, MultipartUpload};
use crate::metadata::user::{UserMetadataEngine, S3User};

pub struct SqliteMetadata {
    pool: SqlitePool,
}

impl SqliteMetadata {
    pub async fn new(database_url: &str) -> Result<Self> {
        let options = SqliteConnectOptions::from_str(database_url)?
            .create_if_missing(true);

        let pool = SqlitePool::connect_with(options).await?;

        // Create tables if they don't exist
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS buckets (
                name TEXT PRIMARY KEY,
                created_at DATETIME DEFAULT CURRENT_TIMESTAMP
            )"
        ).execute(&pool).await?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS objects (
                bucket TEXT,
                key TEXT,
                size INTEGER,
                content_type TEXT,
                checksum TEXT,
                metadata TEXT,
                storage_class TEXT,
                created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
                PRIMARY KEY (bucket, key)
            )"
        ).execute(&pool).await?;

        // Ensure columns exist (for existing databases)
        let _ = sqlx::query("ALTER TABLE objects ADD COLUMN metadata TEXT").execute(&pool).await;
        let _ = sqlx::query("ALTER TABLE objects ADD COLUMN storage_class TEXT").execute(&pool).await;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS multipart_uploads (
                upload_id TEXT PRIMARY KEY,
                bucket TEXT,
                key TEXT,
                metadata TEXT,
                content_type TEXT,
                storage_class TEXT,
                created_at DATETIME DEFAULT CURRENT_TIMESTAMP
            )"
        ).execute(&pool).await?;

        let _ = sqlx::query("ALTER TABLE multipart_uploads ADD COLUMN metadata TEXT").execute(&pool).await;
        let _ = sqlx::query("ALTER TABLE multipart_uploads ADD COLUMN content_type TEXT").execute(&pool).await;
        let _ = sqlx::query("ALTER TABLE multipart_uploads ADD COLUMN storage_class TEXT").execute(&pool).await;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS multipart_parts (
                upload_id TEXT,
                part_number INTEGER,
                size INTEGER,
                checksum TEXT,
                PRIMARY KEY (upload_id, part_number)
            )"
        ).execute(&pool).await?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS users (
                access_key TEXT PRIMARY KEY,
                secret_key TEXT,
                user_id TEXT,
                is_active BOOLEAN DEFAULT 1,
                created_at DATETIME DEFAULT CURRENT_TIMESTAMP
            )"
        ).execute(&pool).await?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS bucket_policies (
                bucket TEXT PRIMARY KEY,
                policy TEXT,
                created_at DATETIME DEFAULT CURRENT_TIMESTAMP
            )"
        ).execute(&pool).await?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS bucket_lifecycle (
                bucket TEXT PRIMARY KEY,
                lifecycle TEXT,
                created_at DATETIME DEFAULT CURRENT_TIMESTAMP
            )"
        ).execute(&pool).await?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS bucket_versioning (
                bucket TEXT PRIMARY KEY,
                status TEXT,
                created_at DATETIME DEFAULT CURRENT_TIMESTAMP
            )"
        ).execute(&pool).await?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS bucket_object_lock (
                bucket TEXT PRIMARY KEY,
                config TEXT,
                created_at DATETIME DEFAULT CURRENT_TIMESTAMP
            )"
        ).execute(&pool).await?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS bucket_configs (
                bucket TEXT NOT NULL,
                config_type TEXT NOT NULL,
                xml TEXT NOT NULL,
                PRIMARY KEY (bucket, config_type)
            )"
        ).execute(&pool).await?;

        Ok(Self { pool })
    }
}

#[async_trait]
impl BucketMetadataEngine for SqliteMetadata {
    async fn create_bucket(&self, name: &str) -> Result<()> {
        sqlx::query("INSERT OR IGNORE INTO buckets (name) VALUES (?)")
            .bind(name)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn delete_bucket(&self, name: &str) -> Result<()> {
        sqlx::query("DELETE FROM buckets WHERE name = ?").bind(name).execute(&self.pool).await?;
        sqlx::query("DELETE FROM objects WHERE bucket = ?").bind(name).execute(&self.pool).await?;
        sqlx::query("DELETE FROM bucket_policies WHERE bucket = ?").bind(name).execute(&self.pool).await?;
        Ok(())
    }

    async fn list_buckets(&self) -> Result<Vec<S3BucketMetadata>> {
        let rows = sqlx::query_as::<_, S3BucketMetadata>(
            "SELECT name, created_at FROM buckets"
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    async fn bucket_exists(&self, name: &str) -> Result<bool> {
        let row = sqlx::query("SELECT name FROM buckets WHERE name = ?")
            .bind(name)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.is_some())
    }

    async fn save_object(&self, meta: S3ObjectMetadata) -> Result<()> {
        sqlx::query(
            "INSERT OR REPLACE INTO objects (bucket, key, size, content_type, checksum, metadata, storage_class)
             VALUES (?, ?, ?, ?, ?, ?, ?)"
        )
        .bind(&meta.bucket)
        .bind(&meta.key)
        .bind(meta.size as i64)
        .bind(&meta.content_type)
        .bind(&meta.checksum)
        .bind(&meta.metadata)
        .bind(&meta.storage_class)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn get_object(&self, bucket: &str, key: &str) -> Result<Option<S3ObjectMetadata>> {
        let row = sqlx::query_as::<_, S3ObjectMetadata>(
            "SELECT bucket, key, size, content_type, checksum, metadata, storage_class, created_at FROM objects WHERE bucket = ? AND key = ?"
        )
        .bind(bucket)
        .bind(key)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    async fn list_objects(&self, bucket: &str) -> Result<Vec<S3ObjectMetadata>> {
        let rows = sqlx::query_as::<_, S3ObjectMetadata>(
            "SELECT bucket, key, size, content_type, checksum, metadata, storage_class, created_at FROM objects WHERE bucket = ?"
        )
        .bind(bucket)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    async fn delete_object(&self, bucket: &str, key: &str) -> Result<()> {
        sqlx::query("DELETE FROM objects WHERE bucket = ? AND key = ?")
            .bind(bucket)
            .bind(key)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn get_bucket_policy(&self, bucket: &str) -> Result<Option<crate::metadata::bucket::BucketPolicy>> {
        let row: Option<(String,)> = sqlx::query_as("SELECT policy FROM bucket_policies WHERE bucket = ?")
            .bind(bucket)
            .fetch_optional(&self.pool)
            .await?;

        if let Some((policy_json,)) = row {
            let policy = serde_json::from_str(&policy_json)?;
            Ok(Some(policy))
        } else {
            Ok(None)
        }
    }

    async fn set_bucket_policy(&self, bucket: &str, policy: crate::metadata::bucket::BucketPolicy) -> Result<()> {
        let policy_json = serde_json::to_string(&policy)?;
        sqlx::query(
            "INSERT OR REPLACE INTO bucket_policies (bucket, policy) VALUES (?, ?)"
        )
        .bind(bucket)
        .bind(policy_json)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn delete_bucket_policy(&self, bucket: &str) -> Result<()> {
        sqlx::query("DELETE FROM bucket_policies WHERE bucket = ?")
            .bind(bucket)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn get_bucket_lifecycle(&self, bucket: &str) -> Result<Option<String>> {
        let row: Option<(String,)> = sqlx::query_as("SELECT lifecycle FROM bucket_lifecycle WHERE bucket = ?")
            .bind(bucket)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.map(|(xml,)| xml))
    }

    async fn set_bucket_lifecycle(&self, bucket: &str, lifecycle_xml: String) -> Result<()> {
        sqlx::query("INSERT OR REPLACE INTO bucket_lifecycle (bucket, lifecycle) VALUES (?, ?)")
            .bind(bucket)
            .bind(lifecycle_xml)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn delete_bucket_lifecycle(&self, bucket: &str) -> Result<()> {
        sqlx::query("DELETE FROM bucket_lifecycle WHERE bucket = ?")
            .bind(bucket)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn get_bucket_versioning(&self, bucket: &str) -> Result<Option<String>> {
        let row: Option<(String,)> = sqlx::query_as("SELECT status FROM bucket_versioning WHERE bucket = ?")
            .bind(bucket)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.map(|(s,)| s))
    }

    async fn set_bucket_versioning(&self, bucket: &str, status: &str) -> Result<()> {
        sqlx::query("INSERT OR REPLACE INTO bucket_versioning (bucket, status) VALUES (?, ?)")
            .bind(bucket)
            .bind(status)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn get_object_lock_config(&self, bucket: &str) -> Result<Option<String>> {
        let row: Option<(String,)> = sqlx::query_as("SELECT config FROM bucket_object_lock WHERE bucket = ?")
            .bind(bucket)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.map(|(c,)| c))
    }

    async fn set_object_lock_config(&self, bucket: &str, config_xml: String) -> Result<()> {
        sqlx::query("INSERT OR REPLACE INTO bucket_object_lock (bucket, config) VALUES (?, ?)")
            .bind(bucket)
            .bind(config_xml)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn get_bucket_config(&self, bucket: &str, config_type: &str) -> Result<Option<String>> {
        let row: Option<(String,)> = sqlx::query_as(
            "SELECT xml FROM bucket_configs WHERE bucket = ? AND config_type = ?"
        )
            .bind(bucket)
            .bind(config_type)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.map(|(x,)| x))
    }

    async fn set_bucket_config(&self, bucket: &str, config_type: &str, xml: String) -> Result<()> {
        sqlx::query(
            "INSERT OR REPLACE INTO bucket_configs (bucket, config_type, xml) VALUES (?, ?, ?)"
        )
            .bind(bucket)
            .bind(config_type)
            .bind(xml)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn delete_bucket_config(&self, bucket: &str, config_type: &str) -> Result<()> {
        sqlx::query("DELETE FROM bucket_configs WHERE bucket = ? AND config_type = ?")
            .bind(bucket)
            .bind(config_type)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}

#[async_trait]
impl MultipartMetadataEngine for SqliteMetadata {
    async fn initiate_multipart(&self, bucket: &str, key: &str, metadata: Option<String>, content_type: Option<String>, storage_class: Option<String>) -> Result<String> {
        let upload_id = uuid::Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO multipart_uploads (upload_id, bucket, key, metadata, content_type, storage_class) VALUES (?, ?, ?, ?, ?, ?)"
        )
        .bind(&upload_id)
        .bind(bucket)
        .bind(key)
        .bind(&metadata)
        .bind(&content_type)
        .bind(&storage_class)
        .execute(&self.pool)
        .await?;
        Ok(upload_id)
    }

    async fn save_part(&self, upload_id: &str, part: MultipartPart) -> Result<()> {
        sqlx::query(
            "INSERT OR REPLACE INTO multipart_parts (upload_id, part_number, size, checksum)
             VALUES (?, ?, ?, ?)"
        )
        .bind(upload_id)
        .bind(part.part_number as i64)
        .bind(part.size as i64)
        .bind(&part.checksum)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn get_multipart(&self, upload_id: &str) -> Result<Option<MultipartUpload>> {
        let upload_row = sqlx::query(
            "SELECT upload_id, bucket, key, metadata, content_type, storage_class FROM multipart_uploads WHERE upload_id = ?"
        )
        .bind(upload_id)
        .fetch_optional(&self.pool)
        .await?;

        if let Some(row) = upload_row {
            let parts = sqlx::query_as::<_, MultipartPart>(
                "SELECT part_number, checksum, size FROM multipart_parts WHERE upload_id = ?"
            )
            .bind(upload_id)
            .fetch_all(&self.pool)
            .await?;

            Ok(Some(MultipartUpload {
                upload_id: row.get("upload_id"),
                bucket: row.get("bucket"),
                key: row.get("key"),
                parts,
                metadata: row.get("metadata"),
                content_type: row.get("content_type"),
                storage_class: row.get("storage_class"),
            }))
        } else {
            Ok(None)
        }
    }

    async fn complete_multipart(&self, upload_id: &str) -> Result<()> {
        sqlx::query("DELETE FROM multipart_parts WHERE upload_id = ?")
            .bind(upload_id)
            .execute(&self.pool)
            .await?;
        sqlx::query("DELETE FROM multipart_uploads WHERE upload_id = ?")
            .bind(upload_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn list_multipart_uploads(&self, bucket: &str, prefix: &str) -> Result<Vec<crate::metadata::multipart::MultipartUploadSummary>> {
        let rows = sqlx::query(
            "SELECT upload_id, bucket, key, created_at FROM multipart_uploads WHERE bucket = ? AND key LIKE ?"
        )
        .bind(bucket)
        .bind(format!("{}%", prefix))
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.into_iter().map(|r| crate::metadata::multipart::MultipartUploadSummary {
            upload_id: r.get("upload_id"),
            bucket: r.get("bucket"),
            key: r.get("key"),
            initiated: r.get("created_at"),
        }).collect())
    }

    async fn find_stale_multipart_uploads(&self, older_than_seconds: u64) -> Result<Vec<String>> {
        let rows = sqlx::query(
            "SELECT upload_id FROM multipart_uploads WHERE created_at < datetime('now', '-' || ? || ' seconds')"
        )
        .bind(older_than_seconds as i64)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.into_iter().map(|r| r.get("upload_id")).collect())
    }
}

#[async_trait]
impl UserMetadataEngine for SqliteMetadata {
    async fn get_user(&self, access_key: &str) -> Result<Option<S3User>> {
        let row = sqlx::query_as::<_, S3User>(
            "SELECT access_key, secret_key, user_id, is_active, created_at FROM users WHERE access_key = ?"
        )
        .bind(access_key)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    async fn create_user(&self, access_key: &str, secret_key: &str, user_id: &str) -> Result<()> {
        sqlx::query(
            "INSERT OR REPLACE INTO users (access_key, secret_key, user_id) VALUES (?, ?, ?)"
        )
        .bind(access_key)
        .bind(secret_key)
        .bind(user_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn list_users(&self) -> Result<Vec<crate::metadata::user::S3User>> {
        let rows = sqlx::query_as::<_, crate::metadata::user::S3User>(
            "SELECT access_key, secret_key, user_id, is_active, created_at FROM users"
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    async fn delete_user(&self, access_key: &str) -> Result<()> {
        sqlx::query("DELETE FROM users WHERE access_key = ?")
            .bind(access_key)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn set_user_active(&self, access_key: &str, is_active: bool) -> Result<()> {
        sqlx::query("UPDATE users SET is_active = ? WHERE access_key = ?")
            .bind(is_active)
            .bind(access_key)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metadata::user::UserMetadataEngine;

    async fn in_memory_db() -> SqliteMetadata {
        SqliteMetadata::new("sqlite::memory:").await.unwrap()
    }

    #[tokio::test]
    async fn test_list_users_empty() {
        let db = in_memory_db().await;
        let users = db.list_users().await.unwrap();
        assert!(users.is_empty());
    }

    #[tokio::test]
    async fn test_list_users_returns_created_user() {
        let db = in_memory_db().await;
        db.create_user("KEY1", "secret1", "alice").await.unwrap();
        db.create_user("KEY2", "secret2", "bob").await.unwrap();

        let users = db.list_users().await.unwrap();
        assert_eq!(users.len(), 2);
        let keys: Vec<&str> = users.iter().map(|u| u.access_key.as_str()).collect();
        assert!(keys.contains(&"KEY1"));
        assert!(keys.contains(&"KEY2"));
    }

    #[tokio::test]
    async fn test_delete_user_removes_it() {
        let db = in_memory_db().await;
        db.create_user("KEY1", "secret", "alice").await.unwrap();
        assert!(db.get_user("KEY1").await.unwrap().is_some());

        db.delete_user("KEY1").await.unwrap();
        assert!(db.get_user("KEY1").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_delete_nonexistent_user_is_noop() {
        let db = in_memory_db().await;
        // Should not error on missing key
        assert!(db.delete_user("DOESNOTEXIST").await.is_ok());
    }

    #[tokio::test]
    async fn test_set_user_active_disables_user() {
        let db = in_memory_db().await;
        db.create_user("KEY1", "secret", "alice").await.unwrap();

        db.set_user_active("KEY1", false).await.unwrap();
        let user = db.get_user("KEY1").await.unwrap().unwrap();
        assert!(!user.is_active);
    }

    #[tokio::test]
    async fn test_set_user_active_re_enables_user() {
        let db = in_memory_db().await;
        db.create_user("KEY1", "secret", "alice").await.unwrap();
        db.set_user_active("KEY1", false).await.unwrap();
        db.set_user_active("KEY1", true).await.unwrap();

        let user = db.get_user("KEY1").await.unwrap().unwrap();
        assert!(user.is_active);
    }

    #[tokio::test]
    async fn test_full_user_lifecycle() {
        let db = in_memory_db().await;

        // Create
        db.create_user("KEY1", "secret", "alice").await.unwrap();
        assert_eq!(db.list_users().await.unwrap().len(), 1);

        // Disable
        db.set_user_active("KEY1", false).await.unwrap();
        assert!(!db.get_user("KEY1").await.unwrap().unwrap().is_active);

        // Re-enable
        db.set_user_active("KEY1", true).await.unwrap();
        assert!(db.get_user("KEY1").await.unwrap().unwrap().is_active);

        // Delete
        db.delete_user("KEY1").await.unwrap();
        assert!(db.list_users().await.unwrap().is_empty());
    }
}
