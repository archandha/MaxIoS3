/// PostgreSQL implementation of the metadata engines for the MaxIOs3 server.
/// This file contains the `PostgresMetadata` struct, which implements the
/// `BucketMetadataEngine`, `MultipartMetadataEngine`, and `UserMetadataEngine` traits.
/// It is functionally equivalent to `SqliteMetadata` but targets PostgreSQL,
/// enabling concurrent writes and multi-node deployments.

use async_trait::async_trait;
use anyhow::Result;
use sqlx::{PgPool, Row, postgres::PgConnectOptions};
use std::str::FromStr;
use crate::metadata::bucket::{BucketMetadataEngine, S3BucketMetadata, S3ObjectMetadata};
use crate::metadata::multipart::{MultipartMetadataEngine, MultipartPart, MultipartUpload};
use crate::metadata::user::{UserMetadataEngine, S3User};

// The shared structs use u32/u64, which have no native unsigned types in PostgreSQL.
// We cannot add manual FromRow<PgRow> impls because the derive macro generates a
// conflicting generic impl. Instead, we decode using sqlx::query() and construct
// structs manually wherever u32/u64 fields require an i32/i64 → cast.

pub struct PostgresMetadata {
    pool: PgPool,
}

impl PostgresMetadata {
    pub async fn new(database_url: &str) -> Result<Self> {
        let options = PgConnectOptions::from_str(database_url)?;
        let pool = PgPool::connect_with(options).await?;

        // Create tables if they don't exist
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS buckets (
                name TEXT PRIMARY KEY,
                created_at TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP
            )"
        ).execute(&pool).await?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS objects (
                bucket TEXT NOT NULL,
                key TEXT NOT NULL,
                size BIGINT NOT NULL,
                content_type TEXT NOT NULL,
                checksum TEXT NOT NULL,
                metadata TEXT,
                storage_class TEXT,
                created_at TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP,
                PRIMARY KEY (bucket, key)
            )"
        ).execute(&pool).await?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS multipart_uploads (
                upload_id TEXT PRIMARY KEY,
                bucket TEXT NOT NULL,
                key TEXT NOT NULL,
                metadata TEXT,
                content_type TEXT,
                storage_class TEXT,
                created_at TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP
            )"
        ).execute(&pool).await?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS multipart_parts (
                upload_id TEXT NOT NULL,
                part_number INTEGER NOT NULL,
                size BIGINT NOT NULL,
                checksum TEXT NOT NULL,
                PRIMARY KEY (upload_id, part_number)
            )"
        ).execute(&pool).await?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS users (
                access_key TEXT PRIMARY KEY,
                secret_key TEXT NOT NULL,
                user_id TEXT NOT NULL,
                is_active BOOLEAN DEFAULT true,
                created_at TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP
            )"
        ).execute(&pool).await?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS bucket_policies (
                bucket TEXT PRIMARY KEY,
                policy TEXT NOT NULL,
                created_at TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP
            )"
        ).execute(&pool).await?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS bucket_lifecycle (
                bucket TEXT PRIMARY KEY,
                lifecycle TEXT NOT NULL,
                created_at TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP
            )"
        ).execute(&pool).await?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS bucket_versioning (
                bucket TEXT PRIMARY KEY,
                status TEXT NOT NULL,
                created_at TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP
            )"
        ).execute(&pool).await?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS bucket_object_lock (
                bucket TEXT PRIMARY KEY,
                config TEXT NOT NULL,
                created_at TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP
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
impl BucketMetadataEngine for PostgresMetadata {
    async fn create_bucket(&self, name: &str) -> Result<()> {
        sqlx::query("INSERT INTO buckets (name) VALUES ($1) ON CONFLICT DO NOTHING")
            .bind(name)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn delete_bucket(&self, name: &str) -> Result<()> {
        sqlx::query("DELETE FROM buckets WHERE name = $1").bind(name).execute(&self.pool).await?;
        sqlx::query("DELETE FROM objects WHERE bucket = $1").bind(name).execute(&self.pool).await?;
        sqlx::query("DELETE FROM bucket_policies WHERE bucket = $1").bind(name).execute(&self.pool).await?;
        Ok(())
    }

    async fn list_buckets(&self) -> Result<Vec<S3BucketMetadata>> {
        let rows = sqlx::query("SELECT name, created_at FROM buckets")
            .fetch_all(&self.pool)
            .await?;
        rows.into_iter().map(|r| Ok(S3BucketMetadata {
            name: r.try_get("name")?,
            created_at: r.try_get("created_at")?,
        })).collect::<Result<Vec<_>, sqlx::Error>>().map_err(Into::into)
    }

    async fn bucket_exists(&self, name: &str) -> Result<bool> {
        let row = sqlx::query("SELECT name FROM buckets WHERE name = $1")
            .bind(name)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.is_some())
    }

    async fn save_object(&self, meta: S3ObjectMetadata) -> Result<()> {
        sqlx::query(
            "INSERT INTO objects (bucket, key, size, content_type, checksum, metadata, storage_class)
             VALUES ($1, $2, $3, $4, $5, $6, $7)
             ON CONFLICT (bucket, key) DO UPDATE SET
                size = EXCLUDED.size,
                content_type = EXCLUDED.content_type,
                checksum = EXCLUDED.checksum,
                metadata = EXCLUDED.metadata,
                storage_class = EXCLUDED.storage_class,
                created_at = CURRENT_TIMESTAMP"
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
        let row = sqlx::query(
            "SELECT bucket, key, size, content_type, checksum, metadata, storage_class, created_at
             FROM objects WHERE bucket = $1 AND key = $2"
        )
        .bind(bucket)
        .bind(key)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|r| S3ObjectMetadata {
            bucket: r.get("bucket"),
            key: r.get("key"),
            size: r.get::<i64, _>("size") as u64,
            content_type: r.get("content_type"),
            checksum: r.get("checksum"),
            metadata: r.get("metadata"),
            storage_class: r.get("storage_class"),
            created_at: r.get("created_at"),
        }))
    }

    async fn list_objects(&self, bucket: &str) -> Result<Vec<S3ObjectMetadata>> {
        let rows = sqlx::query(
            "SELECT bucket, key, size, content_type, checksum, metadata, storage_class, created_at
             FROM objects WHERE bucket = $1"
        )
        .bind(bucket)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(|r| S3ObjectMetadata {
            bucket: r.get("bucket"),
            key: r.get("key"),
            size: r.get::<i64, _>("size") as u64,
            content_type: r.get("content_type"),
            checksum: r.get("checksum"),
            metadata: r.get("metadata"),
            storage_class: r.get("storage_class"),
            created_at: r.get("created_at"),
        }).collect())
    }

    async fn delete_object(&self, bucket: &str, key: &str) -> Result<()> {
        sqlx::query("DELETE FROM objects WHERE bucket = $1 AND key = $2")
            .bind(bucket)
            .bind(key)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn get_bucket_policy(&self, bucket: &str) -> Result<Option<crate::metadata::bucket::BucketPolicy>> {
        let row: Option<(String,)> = sqlx::query_as(
            "SELECT policy FROM bucket_policies WHERE bucket = $1"
        )
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
            "INSERT INTO bucket_policies (bucket, policy) VALUES ($1, $2)
             ON CONFLICT (bucket) DO UPDATE SET policy = EXCLUDED.policy"
        )
        .bind(bucket)
        .bind(policy_json)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn delete_bucket_policy(&self, bucket: &str) -> Result<()> {
        sqlx::query("DELETE FROM bucket_policies WHERE bucket = $1")
            .bind(bucket)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn get_bucket_lifecycle(&self, bucket: &str) -> Result<Option<String>> {
        let row: Option<(String,)> = sqlx::query_as(
            "SELECT lifecycle FROM bucket_lifecycle WHERE bucket = $1"
        )
        .bind(bucket)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|(xml,)| xml))
    }

    async fn set_bucket_lifecycle(&self, bucket: &str, lifecycle_xml: String) -> Result<()> {
        sqlx::query(
            "INSERT INTO bucket_lifecycle (bucket, lifecycle) VALUES ($1, $2)
             ON CONFLICT (bucket) DO UPDATE SET lifecycle = EXCLUDED.lifecycle"
        )
        .bind(bucket)
        .bind(lifecycle_xml)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn delete_bucket_lifecycle(&self, bucket: &str) -> Result<()> {
        sqlx::query("DELETE FROM bucket_lifecycle WHERE bucket = $1")
            .bind(bucket)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn get_bucket_versioning(&self, bucket: &str) -> Result<Option<String>> {
        let row: Option<(String,)> = sqlx::query_as(
            "SELECT status FROM bucket_versioning WHERE bucket = $1"
        )
        .bind(bucket)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|(s,)| s))
    }

    async fn set_bucket_versioning(&self, bucket: &str, status: &str) -> Result<()> {
        sqlx::query(
            "INSERT INTO bucket_versioning (bucket, status) VALUES ($1, $2)
             ON CONFLICT (bucket) DO UPDATE SET status = EXCLUDED.status"
        )
        .bind(bucket)
        .bind(status)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn get_object_lock_config(&self, bucket: &str) -> Result<Option<String>> {
        let row: Option<(String,)> = sqlx::query_as(
            "SELECT config FROM bucket_object_lock WHERE bucket = $1"
        )
        .bind(bucket)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|(c,)| c))
    }

    async fn set_object_lock_config(&self, bucket: &str, config_xml: String) -> Result<()> {
        sqlx::query(
            "INSERT INTO bucket_object_lock (bucket, config) VALUES ($1, $2)
             ON CONFLICT (bucket) DO UPDATE SET config = EXCLUDED.config"
        )
        .bind(bucket)
        .bind(config_xml)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn get_bucket_config(&self, bucket: &str, config_type: &str) -> Result<Option<String>> {
        let row: Option<(String,)> = sqlx::query_as(
            "SELECT xml FROM bucket_configs WHERE bucket = $1 AND config_type = $2"
        )
        .bind(bucket)
        .bind(config_type)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|(x,)| x))
    }

    async fn set_bucket_config(&self, bucket: &str, config_type: &str, xml: String) -> Result<()> {
        sqlx::query(
            "INSERT INTO bucket_configs (bucket, config_type, xml) VALUES ($1, $2, $3)
             ON CONFLICT (bucket, config_type) DO UPDATE SET xml = EXCLUDED.xml"
        )
        .bind(bucket)
        .bind(config_type)
        .bind(xml)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn delete_bucket_config(&self, bucket: &str, config_type: &str) -> Result<()> {
        sqlx::query("DELETE FROM bucket_configs WHERE bucket = $1 AND config_type = $2")
            .bind(bucket)
            .bind(config_type)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}

#[async_trait]
impl MultipartMetadataEngine for PostgresMetadata {
    async fn initiate_multipart(
        &self, bucket: &str, key: &str,
        metadata: Option<String>, content_type: Option<String>, storage_class: Option<String>,
    ) -> Result<String> {
        let upload_id = uuid::Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO multipart_uploads (upload_id, bucket, key, metadata, content_type, storage_class)
             VALUES ($1, $2, $3, $4, $5, $6)"
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
            "INSERT INTO multipart_parts (upload_id, part_number, size, checksum)
             VALUES ($1, $2, $3, $4)
             ON CONFLICT (upload_id, part_number) DO UPDATE SET
                size = EXCLUDED.size,
                checksum = EXCLUDED.checksum"
        )
        .bind(upload_id)
        .bind(part.part_number as i32)
        .bind(part.size as i64)
        .bind(&part.checksum)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn get_multipart(&self, upload_id: &str) -> Result<Option<MultipartUpload>> {
        let upload_row = sqlx::query(
            "SELECT upload_id, bucket, key, metadata, content_type, storage_class
             FROM multipart_uploads WHERE upload_id = $1"
        )
        .bind(upload_id)
        .fetch_optional(&self.pool)
        .await?;

        if let Some(row) = upload_row {
            let part_rows = sqlx::query(
                "SELECT part_number, checksum, size FROM multipart_parts WHERE upload_id = $1"
            )
            .bind(upload_id)
            .fetch_all(&self.pool)
            .await?;
            let parts: Vec<MultipartPart> = part_rows.into_iter().map(|r| MultipartPart {
                part_number: r.get::<i32, _>("part_number") as u32,
                checksum: r.get("checksum"),
                size: r.get::<i64, _>("size") as u64,
            }).collect();

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
        sqlx::query("DELETE FROM multipart_parts WHERE upload_id = $1")
            .bind(upload_id)
            .execute(&self.pool)
            .await?;
        sqlx::query("DELETE FROM multipart_uploads WHERE upload_id = $1")
            .bind(upload_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn list_multipart_uploads(
        &self, bucket: &str, prefix: &str,
    ) -> Result<Vec<crate::metadata::multipart::MultipartUploadSummary>> {
        let rows = sqlx::query(
            "SELECT upload_id, bucket, key, created_at
             FROM multipart_uploads WHERE bucket = $1 AND key LIKE $2"
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
            "SELECT upload_id FROM multipart_uploads
             WHERE created_at < NOW() - MAKE_INTERVAL(secs => $1)"
        )
        .bind(older_than_seconds as f64)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.into_iter().map(|r| r.get("upload_id")).collect())
    }
}

#[async_trait]
impl UserMetadataEngine for PostgresMetadata {
    async fn get_user(&self, access_key: &str) -> Result<Option<S3User>> {
        let row = sqlx::query(
            "SELECT access_key, secret_key, user_id, is_active, created_at
             FROM users WHERE access_key = $1"
        )
        .bind(access_key)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|r| S3User {
            access_key: r.get("access_key"),
            secret_key: r.get("secret_key"),
            user_id: r.get("user_id"),
            is_active: r.get("is_active"),
            created_at: r.get("created_at"),
        }))
    }

    async fn create_user(&self, access_key: &str, secret_key: &str, user_id: &str) -> Result<()> {
        sqlx::query(
            "INSERT INTO users (access_key, secret_key, user_id) VALUES ($1, $2, $3)
             ON CONFLICT (access_key) DO UPDATE SET
                secret_key = EXCLUDED.secret_key,
                user_id = EXCLUDED.user_id"
        )
        .bind(access_key)
        .bind(secret_key)
        .bind(user_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}
