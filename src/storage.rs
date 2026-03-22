/// Physical storage abstraction for the MaxIOs3 server.
/// This file defines the `StorageEngine` trait and its `FileSystemStorage` implementation.
/// It handles the actual reading and writing of object data to the local filesystem,
/// including support for multipart uploads and checksum verification.

use async_trait::async_trait;
use futures::StreamExt;
use anyhow::{Result, anyhow};
use tokio_util::io::ReaderStream;
use axum::body::Body;
use sha2::{Digest, Sha256};
use md5::Md5;
use tokio::io::AsyncWriteExt;

/// The StorageEngine trait defines how data is physically stored.
#[async_trait]
pub trait StorageEngine: Send + Sync {
    /// Creates a new bucket (directory).
    async fn create_bucket(&self, name: &str) -> Result<()>;

    /// Deletes a bucket and all its contents.
    async fn delete_bucket(&self, name: &str) -> Result<()>;

    /// Streams data into a new object, verifying SHA256 checksum on the fly.
    /// Returns (size, md5_hex) — md5_hex is used as the S3 ETag.
    async fn put(&self, bucket: &str, key: &str, body: Body, expected_checksum: &str) -> Result<(u64, String)>;

    /// Returns a stream to read the object efficiently (Streaming).
    async fn get(&self, bucket: &str, key: &str) -> Result<Body>;

    /// Checks if an object exists.
    async fn exists(&self, bucket: &str, key: &str) -> Result<bool>;

    /// Deletes an object.
    async fn delete(&self, bucket: &str, key: &str) -> Result<()>;

    /// Streams data into a multipart part, verifying SHA256 checksum on the fly.
    /// Returns (size, md5_hex) — md5_hex is stored as the part ETag.
    async fn put_part(&self, upload_id: &str, part_number: u32, body: Body, expected_checksum: &str) -> Result<(u64, String)>;

    /// Assembles all parts of a multipart upload into a final object.
    async fn assemble(&self, upload_id: &str, bucket: &str, key: &str, part_numbers: Vec<u32>) -> Result<u64>;

    /// Copies an object from source to destination.
    async fn copy(&self, src_bucket: &str, src_key: &str, dst_bucket: &str, dst_key: &str) -> Result<()>;

    /// Writes bytes directly to an object. Returns (size, md5_hex).
    async fn put_bytes(&self, bucket: &str, key: &str, data: bytes::Bytes, expected_checksum: &str) -> Result<(u64, String)>;

    /// Cleans up temporary files after a multipart upload.
    async fn cleanup_multipart(&self, upload_id: &str) -> Result<()>;
}

/// A filesystem implementation of the StorageEngine.
pub struct FileSystemStorage {
    root: std::path::PathBuf,
    temp: std::path::PathBuf,
}

impl FileSystemStorage {
    pub fn new(root: &str) -> Self {
        let root = std::path::PathBuf::from(root);
        let temp = root.join(".temp");
        std::fs::create_dir_all(&root).ok();
        std::fs::create_dir_all(&temp).ok();
        Self { root, temp }
    }

    fn get_path(&self, bucket: &str, key: &str) -> std::path::PathBuf {
        let mut path = self.root.join(bucket);
        if key.ends_with('/') {
            // If the key ends with a slash, it's a directory marker in S3.
            // We store it as a file named "$folder$" inside the directory to avoid "Is a directory" errors.
            path.push(key);
            path.push("$folder$");
        } else {
            path.push(key);
        }
        path
    }
}

#[async_trait]
impl StorageEngine for FileSystemStorage {
    async fn create_bucket(&self, name: &str) -> Result<()> {
        let bucket_path = self.root.join(name);
        tokio::fs::create_dir_all(bucket_path).await?;
        Ok(())
    }

    async fn delete_bucket(&self, name: &str) -> Result<()> {
        let bucket_path = self.root.join(name);
        if bucket_path.exists() {
            tokio::fs::remove_dir_all(bucket_path).await?;
        }
        Ok(())
    }

    async fn put(&self, bucket: &str, key: &str, body: Body, expected_checksum: &str) -> Result<(u64, String)> {
        let file_path = self.get_path(bucket, key);

        if let Some(parent) = file_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        let mut file = tokio::fs::File::create(&file_path).await?;
        let mut stream = body.into_data_stream();
        let mut total_size = 0u64;
        let mut sha256 = Sha256::new();
        let mut md5 = Md5::new();

        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| anyhow!("Stream error: {}", e))?;
            file.write_all(&chunk).await?;
            sha256.update(&chunk);
            md5.update(&chunk);
            total_size += chunk.len() as u64;
        }

        let calculated_sha256 = hex::encode(sha256.finalize());
        let md5_hex = hex::encode(md5.finalize());

        if expected_checksum != "UNSIGNED-PAYLOAD" && calculated_sha256 != expected_checksum {
            drop(file);
            tokio::fs::remove_file(&file_path).await?;
            return Err(anyhow!("Checksum mismatch: expected {}, got {}", expected_checksum, calculated_sha256));
        }

        Ok((total_size, md5_hex))
    }

    async fn get(&self, bucket: &str, key: &str) -> Result<Body> {
        let file_path = self.get_path(bucket, key);
        let file = tokio::fs::File::open(file_path).await?;
        let stream = ReaderStream::new(file);
        Ok(Body::from_stream(stream))
    }

    async fn exists(&self, bucket: &str, key: &str) -> Result<bool> {
        let file_path = self.get_path(bucket, key);
        Ok(tokio::fs::metadata(file_path).await.is_ok())
    }

    async fn delete(&self, bucket: &str, key: &str) -> Result<()> {
        let file_path = self.get_path(bucket, key);
        if file_path.exists() {
            tokio::fs::remove_file(file_path).await?;
        }
        Ok(())
    }

    async fn put_part(&self, upload_id: &str, part_number: u32, body: Body, expected_checksum: &str) -> Result<(u64, String)> {
        let upload_path = self.temp.join(upload_id);
        tokio::fs::create_dir_all(&upload_path).await?;
        let part_path = upload_path.join(part_number.to_string());

        let mut file = tokio::fs::File::create(&part_path).await?;
        let mut stream = body.into_data_stream();
        let mut total_size = 0u64;
        let mut sha256 = Sha256::new();
        let mut md5 = Md5::new();

        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| anyhow!("Stream error: {}", e))?;
            file.write_all(&chunk).await?;
            sha256.update(&chunk);
            md5.update(&chunk);
            total_size += chunk.len() as u64;
        }

        let calculated_sha256 = hex::encode(sha256.finalize());
        let md5_hex = hex::encode(md5.finalize());

        if expected_checksum != "UNSIGNED-PAYLOAD" && calculated_sha256 != expected_checksum {
            drop(file);
            tokio::fs::remove_file(&part_path).await?;
            return Err(anyhow!("Checksum mismatch for part {}: expected {}, got {}", part_number, expected_checksum, calculated_sha256));
        }

        Ok((total_size, md5_hex))
    }

    async fn assemble(&self, upload_id: &str, bucket: &str, key: &str, part_numbers: Vec<u32>) -> Result<u64> {
        let file_path = self.get_path(bucket, key);

        // Ensure parent directory exists
        if let Some(parent) = file_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        let mut final_file = tokio::fs::File::create(&file_path).await?;
        let upload_path = self.temp.join(upload_id);
        let mut total_size = 0;

        // Sort part numbers to ensure they are merged in the correct order
        let mut sorted_parts = part_numbers;
        sorted_parts.sort_unstable();

        for part_number in sorted_parts {
            let part_path = upload_path.join(part_number.to_string());
            let mut part_file = tokio::fs::File::open(&part_path).await?;

            // Efficiently copy the part content to the final file
            let copied = tokio::io::copy(&mut part_file, &mut final_file).await?;
            total_size += copied;
        }

        Ok(total_size)
    }

    async fn copy(&self, src_bucket: &str, src_key: &str, dst_bucket: &str, dst_key: &str) -> Result<()> {
        let src_path = self.get_path(src_bucket, src_key);
        let dst_path = self.get_path(dst_bucket, dst_key);

        if let Some(parent) = dst_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        tokio::fs::copy(src_path, dst_path).await?;
        Ok(())
    }

    async fn put_bytes(&self, bucket: &str, key: &str, data: bytes::Bytes, expected_checksum: &str) -> Result<(u64, String)> {
        let file_path = self.get_path(bucket, key);
        if let Some(parent) = file_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        let sha256 = hex::encode(Sha256::digest(&data));
        if expected_checksum != "UNSIGNED-PAYLOAD" && sha256 != expected_checksum {
            return Err(anyhow!("Checksum mismatch: expected {}, got {}", expected_checksum, sha256));
        }

        let md5_hex = hex::encode(Md5::digest(&data));
        tokio::fs::write(file_path, &data).await?;
        Ok((data.len() as u64, md5_hex))
    }

    async fn cleanup_multipart(&self, upload_id: &str) -> Result<()> {
        let upload_path = self.temp.join(upload_id);
        if upload_path.exists() {
            tokio::fs::remove_dir_all(upload_path).await?;
        }
        Ok(())
    }
}
