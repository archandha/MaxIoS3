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

/// Returns true only when the checksum is an actual hex SHA256 that should be verified.
/// Sentinel values like UNSIGNED-PAYLOAD and STREAMING-AWS4-HMAC-SHA256-PAYLOAD are skipped.
pub(crate) fn is_real_sha256(s: &str) -> bool {
    s.len() == 64 && s.chars().all(|c| c.is_ascii_hexdigit())
}

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

    /// Copies an existing object into a multipart upload part.
    /// Returns (size, md5_hex) for use as the part ETag.
    async fn copy_to_part(&self, src_bucket: &str, src_key: &str, upload_id: &str, part_number: u32) -> Result<(u64, String)>;

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

        if is_real_sha256(expected_checksum) && calculated_sha256 != expected_checksum {
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

        if is_real_sha256(expected_checksum) && calculated_sha256 != expected_checksum {
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
        if is_real_sha256(expected_checksum) && sha256 != expected_checksum {
            return Err(anyhow!("Checksum mismatch: expected {}, got {}", expected_checksum, sha256));
        }

        let md5_hex = hex::encode(Md5::digest(&data));
        tokio::fs::write(file_path, &data).await?;
        Ok((data.len() as u64, md5_hex))
    }

    async fn copy_to_part(&self, src_bucket: &str, src_key: &str, upload_id: &str, part_number: u32) -> Result<(u64, String)> {
        let src_path = self.get_path(src_bucket, src_key);
        let upload_path = self.temp.join(upload_id);
        tokio::fs::create_dir_all(&upload_path).await?;
        let part_path = upload_path.join(part_number.to_string());

        // Copy while computing MD5
        let mut src_file = tokio::fs::File::open(&src_path).await?;
        let mut dst_file = tokio::fs::File::create(&part_path).await?;
        let mut md5 = Md5::new();
        let mut total_size = 0u64;
        let mut buf = vec![0u8; 256 * 1024];
        loop {
            let n = tokio::io::AsyncReadExt::read(&mut src_file, &mut buf).await?;
            if n == 0 { break; }
            dst_file.write_all(&buf[..n]).await?;
            md5.update(&buf[..n]);
            total_size += n as u64;
        }
        Ok((total_size, hex::encode(md5.finalize())))
    }

    async fn cleanup_multipart(&self, upload_id: &str) -> Result<()> {
        let upload_path = self.temp.join(upload_id);
        if upload_path.exists() {
            tokio::fs::remove_dir_all(upload_path).await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use sha2::{Digest, Sha256};

    fn make_storage() -> (FileSystemStorage, tempfile::TempDir) {
        let dir = tempfile::TempDir::new().unwrap();
        let storage = FileSystemStorage::new(dir.path().to_str().unwrap());
        (storage, dir)
    }

    // ── is_real_sha256 ────────────────────────────────────────────────────────

    #[test]
    fn test_is_real_sha256_valid() {
        let hash = "a".repeat(64);
        assert!(is_real_sha256(&hash));
    }

    #[test]
    fn test_is_real_sha256_uppercase_valid() {
        let hash = "A".repeat(64);
        assert!(is_real_sha256(&hash));
    }

    #[test]
    fn test_is_real_sha256_sentinel_unsigned() {
        assert!(!is_real_sha256("UNSIGNED-PAYLOAD"));
    }

    #[test]
    fn test_is_real_sha256_sentinel_streaming() {
        assert!(!is_real_sha256("STREAMING-AWS4-HMAC-SHA256-PAYLOAD"));
    }

    #[test]
    fn test_is_real_sha256_too_short() {
        let hash = "a".repeat(63);
        assert!(!is_real_sha256(&hash));
    }

    #[test]
    fn test_is_real_sha256_non_hex_char() {
        let mut hash = "a".repeat(63);
        hash.push('g'); // 'g' is not a hex digit
        assert!(!is_real_sha256(&hash));
    }

    // ── FileSystemStorage ─────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_put_bytes_and_exists_roundtrip() {
        let (storage, _dir) = make_storage();
        storage.create_bucket("bkt").await.unwrap();

        let data = bytes::Bytes::from("hello storage");
        let sha = hex::encode(Sha256::digest(&data));
        storage.put_bytes("bkt", "key.txt", data.clone(), &sha).await.unwrap();

        assert!(storage.exists("bkt", "key.txt").await.unwrap());
    }

    #[tokio::test]
    async fn test_put_bytes_bad_checksum_returns_error() {
        let (storage, _dir) = make_storage();
        storage.create_bucket("bkt").await.unwrap();

        let data = bytes::Bytes::from("hello");
        let wrong_sha = "a".repeat(64);
        assert!(storage.put_bytes("bkt", "key.txt", data, &wrong_sha).await.is_err());
    }

    #[tokio::test]
    async fn test_put_checksum_mismatch_cleans_up_file() {
        let (storage, _dir) = make_storage();
        storage.create_bucket("bkt").await.unwrap();

        let data = Body::from("hello");
        let wrong_sha = "a".repeat(64);
        assert!(storage.put("bkt", "key.txt", data, &wrong_sha).await.is_err());
        // The partial file must have been cleaned up.
        assert!(!storage.exists("bkt", "key.txt").await.unwrap());
    }

    #[tokio::test]
    async fn test_put_and_get_roundtrip() {
        let (storage, _dir) = make_storage();
        storage.create_bucket("bkt").await.unwrap();

        let payload = b"roundtrip data";
        let sha = hex::encode(Sha256::digest(payload));
        storage.put("bkt", "obj", Body::from(payload.as_ref()), &sha).await.unwrap();

        let body = storage.get("bkt", "obj").await.unwrap();
        let bytes = axum::body::to_bytes(body, usize::MAX).await.unwrap();
        assert_eq!(bytes.as_ref(), payload);
    }

    #[tokio::test]
    async fn test_delete_removes_object() {
        let (storage, _dir) = make_storage();
        storage.create_bucket("bkt").await.unwrap();

        let data = bytes::Bytes::from("bye");
        let sha = hex::encode(Sha256::digest(&data));
        storage.put_bytes("bkt", "gone.txt", data, &sha).await.unwrap();
        assert!(storage.exists("bkt", "gone.txt").await.unwrap());

        storage.delete("bkt", "gone.txt").await.unwrap();
        assert!(!storage.exists("bkt", "gone.txt").await.unwrap());
    }

    #[tokio::test]
    async fn test_directory_marker_key_stored_as_folder_sentinel() {
        let (storage, dir) = make_storage();
        storage.create_bucket("bkt").await.unwrap();

        let data = bytes::Bytes::from("");
        storage.put_bytes("bkt", "prefix/", data, "UNSIGNED-PAYLOAD").await.unwrap();

        // The file should be stored at bkt/prefix/$folder$, not as a directory.
        let sentinel = dir.path().join("bkt").join("prefix").join("$folder$");
        assert!(sentinel.exists(), "expected $folder$ sentinel file at {:?}", sentinel);
    }

    #[tokio::test]
    async fn test_assemble_multipart_concatenates_in_order() {
        let (storage, _dir) = make_storage();
        storage.create_bucket("bkt").await.unwrap();

        let upload_id = "test-upload-001";
        storage.put_part(upload_id, 1, Body::from("part1"), "UNSIGNED-PAYLOAD").await.unwrap();
        storage.put_part(upload_id, 2, Body::from("part2"), "UNSIGNED-PAYLOAD").await.unwrap();
        storage.put_part(upload_id, 3, Body::from("part3"), "UNSIGNED-PAYLOAD").await.unwrap();

        // Pass parts out of order to verify assembly sorts them.
        storage.assemble(upload_id, "bkt", "assembled", vec![3, 1, 2]).await.unwrap();

        let body = storage.get("bkt", "assembled").await.unwrap();
        let bytes = axum::body::to_bytes(body, usize::MAX).await.unwrap();
        assert_eq!(&bytes[..], b"part1part2part3");
    }

    #[tokio::test]
    async fn test_cleanup_multipart_removes_temp_dir() {
        let (storage, dir) = make_storage();
        let upload_id = "cleanup-me";
        storage.put_part(upload_id, 1, Body::from("data"), "UNSIGNED-PAYLOAD").await.unwrap();

        let temp_path = dir.path().join(".temp").join(upload_id);
        assert!(temp_path.exists());

        storage.cleanup_multipart(upload_id).await.unwrap();
        assert!(!temp_path.exists());
    }
}
