# Road map

This document tracks planned improvements, known limitations, and intentional
non-goals for MaxIOs3.

## Currently Implemented

The following S3 operations are fully working and pass the MinIO Mint
integration test suite:

**Bucket operations**
- `ListBuckets`
- `CreateBucket` / `DeleteBucket` (with `BucketNotEmpty` guard)
- `HeadBucket`
- `GetBucketLocation`
- `GetBucketPolicy` / `PutBucketPolicy` / `DeleteBucketPolicy`
- `GetBucketCors` / `PutBucketCors` / `DeleteBucketCors`
- `GetBucketEncryption` / `PutBucketEncryption` / `DeleteBucketEncryption`
- `GetBucketLifecycleConfiguration` / `PutBucketLifecycleConfiguration` / `DeleteBucketLifecycleConfiguration`
- `ListMultipartUploads`
- Bucket config sub-resources stored and returned verbatim: `acl`, `tagging`,
  `website`, `logging`, `replication`, `notification`, `accelerate`,
  `requestPayment`, `intelligent-tiering`, `inventory`, `metrics`,
  `ownershipControls`, `publicAccessBlock`

**Object operations**
- `GetObject` / `HeadObject` — including byte-range requests (both `bytes=start-end` and suffix `bytes=-N`)
- `PutObject` — including `x-amz-copy-source` (CopyObject), streaming SigV4, chunked encoding, and additional checksum verification (CRC32, CRC32C, SHA-1, SHA-256)
- `DeleteObject` — including conditional delete via `If-Match`
- `DeleteObjects` (multi-delete)
- `GetObjectTagging` / `PutObjectTagging` — with validation (max 10 tags, no duplicates, key/value length limits)
- `GetObjectAttributes` — returns ETag, ObjectSize, StorageClass, Checksum, ObjectParts
- `SelectObjectContent` — basic CSV input with `SELECT *` / `SELECT col1, col2` and `WHERE col = 'val'`; supports `FileHeaderInfo` USE / IGNORE / NONE

**Multipart upload**
- `CreateMultipartUpload`
- `UploadPart` — including streaming SigV4 and chunked encoding
- `UploadPartCopy` — copies an existing object into a multipart part
- `CompleteMultipartUpload` — enforces 5 MB minimum part size (except last), computes multipart ETag
- `AbortMultipartUpload`
- `ListParts`

**Presigned requests**
- Presigned GET and PUT (query-string authentication)
- Presigned POST (form-based upload with policy document)

**Authentication & authorization**
- AWS Signature Version 4 (SigV4) — header and query-string variants
- Bucket policies — Allow/Deny by action, resource ARN pattern, and principal

**MinIO compatibility extensions**
- Health endpoints: `GET /minio/health/live`, `GET /minio/health/ready`
- Prometheus metrics stubs: `/minio/v2/metrics/{cluster,node,bucket,resource}`
- Force-delete header: `x-minio-force-delete: true` on `DELETE /{bucket}` or `DELETE /{bucket}/{prefix}` deletes all matching objects recursively
- Snowball auto-extract: `x-amz-meta-snowball-auto-extract: true` on `PUT` unpacks a tar archive into individual objects

---

## Near-Term Improvements

These are concrete, bounded tasks that would meaningfully increase compatibility
or operational usability:

### S3 Select — extended SQL support
The current CSV S3 Select implementation handles simple `SELECT` and `WHERE` equality filters. Missing:
- Aggregate functions (`COUNT`, `SUM`, `AVG`, `MIN`, `MAX`)
- `LIMIT` clause
- JSON input format
- Custom quote and escape characters in CSV
- Proper `NULL` handling

This would allow the `s3select` Mint suite to pass.

### Bucket versioning
Currently the server returns `NotImplemented` for `PutBucketVersioning`, which
causes the SDKs to skip all versioning tests gracefully. A real implementation
would require storing multiple versions of each object key and returning the
correct `x-amz-version-id` headers.

### Object locking (WORM)
`PutObjectLockConfiguration` is accepted and stored but has no enforcement.
Implementing it requires checking retention dates and legal holds before
allowing deletes or overwrites.

### Presigned URL expiry enforcement
Presigned URLs are currently validated for signature correctness but `X-Amz-Expires` is not checked against the current time. Expired URLs are accepted.

### User management API
Adding a minimal `/minio/v1/admin/` API (user add/list/remove, policy attach)
would allow the `mc test_admin_users` test to pass and make the server
self-contained for development environments that use `mc` as an admin tool.

### ETag for CopyObject
When copying an object, the destination ETag should reflect the copied content.
Currently it inherits the source ETag, which is correct for single-part objects
but may differ for objects that were originally uploaded via multipart.

---

## Medium-Term Ideas

These require more design work but address real operational gaps:

### Persistent user management via API
A proper REST API for creating and managing users and access policies, rather
than requiring direct database writes.

### Object lifecycle enforcement
The lifecycle XML is stored and returned correctly but expiration rules are not
evaluated. A background task could delete objects that have passed their
configured expiration date.

### Multi-node / clustered storage backend
The `StorageEngine` trait is designed to be pluggable. A distributed backend
(e.g., backed by a networked file system or an object store like Ceph) could
replace `FileSystemStorage` without changes to the handlers.

### Prometheus metrics with real data
The metrics endpoints currently return placeholder values. Wiring them to actual
counters (request rates, storage used, error counts) would make the server
observable in production.

### Audit logging
Per-request structured logs that record who accessed what, suitable for
compliance use cases.

---

## Non-Goals

The following are deliberately out of scope for MaxIOs3:

- **S3 versioning with full delete-marker semantics** — the complexity is high
  relative to the use cases MaxIOs3 targets
- **Multi-region replication** — MaxIOs3 is a single-site server
- **IAM-compatible policy evaluation** — bucket policies cover the common cases;
  full IAM with roles, groups, and session tokens is not planned
- **The MinIO admin API in full** — only the subset needed for basic `mc` and
  tooling compatibility is considered
- **S3 Glacier / storage tiering** — storage class values are stored and
  returned but no actual tiering is performed
