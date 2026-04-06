# Road map

This document tracks planned improvements, known limitations, and intentional
non-goals for MaxIOs3.

## Currently Implemented

The following S3 operations are fully working and pass the MinIO Mint
integration test suite:

### Bucket operations

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

### Object operations

- `GetObject` / `HeadObject` — including byte-range requests (both `bytes=start-end` and suffix `bytes=-N`)
- `PutObject` — including `x-amz-copy-source` (CopyObject), streaming SigV4, chunked encoding, and additional checksum verification (CRC32, CRC32C, SHA-1, SHA-256)
- `DeleteObject` — including conditional delete via `If-Match`
- `DeleteObjects` (multi-delete)
- `GetObjectTagging` / `PutObjectTagging` — with validation (max 10 tags, no duplicates, key/value length limits)
- `GetObjectAttributes` — returns ETag, ObjectSize, StorageClass, Checksum, ObjectParts
- `SelectObjectContent` — basic CSV input with `SELECT *` / `SELECT col1, col2` and `WHERE col = 'val'`; supports `FileHeaderInfo` USE / IGNORE / NONE

### Multipart upload

- `CreateMultipartUpload`
- `UploadPart` — including streaming SigV4 and chunked encoding
- `UploadPartCopy` — copies an existing object into a multipart part
- `CompleteMultipartUpload` — enforces 5 MB minimum part size (except last), computes multipart ETag
- `AbortMultipartUpload`
- `ListParts`

### Presigned requests

- Presigned GET and PUT (query-string authentication)
- Presigned POST (form-based upload with policy document)

### Authentication & authorization

- AWS Signature Version 4 (SigV4) — header and query-string variants
- Bucket policies — Allow/Deny by action, resource ARN pattern, and principal

### MinIO compatibility extensions

- Health endpoints: `GET /minio/health/live`, `GET /minio/health/ready`
- Prometheus metrics stubs: `/minio/v2/metrics/{cluster,node,bucket,resource}`
- Force-delete header: `x-minio-force-delete: true` on `DELETE /{bucket}` or `DELETE /{bucket}/{prefix}`
  deletes all matching objects recursively
- Snowball auto-extract: `x-amz-meta-snowball-auto-extract: true` on `PUT` unpacks a tar archive into individual objects

### Metadata backends

- **SQLite** — default, zero-dependency; schema created on first run; suitable
  for single-node and development setups. Note: writes are serialized through a
  single connection, limiting throughput under heavy concurrent PUT load, and
  SQLite cannot be shared across multiple nodes.
- **PostgreSQL** — production backend with concurrent writes and connection
  pooling; selected automatically when `DATABASE_URL` starts with `postgres://`
  or `postgresql://`; schema created on first run.

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

Presigned URLs are currently validated for signature correctness but
`X-Amz-Expires` is not checked against the current time. Expired URLs are
accepted.

### User management API

Adding a minimal `/minio/v1/admin/` API (user add/list/remove, policy attach)
would allow the `mc test_admin_users` test to pass and make the server
self-contained for development environments that use `mc` as an admin tool.

This API also acts as the integration point for external secrets management
tooling. HashiCorp Vault's AWS secrets engine, Kubernetes External Secrets, and
similar systems can call these endpoints to create and revoke short-lived
credentials on demand without requiring any direct database access or knowledge
of MaxIOS3 internals. The access-key / secret-key model maps directly to what
these tools expect.

### ETag for CopyObject

When copying an object, the destination ETag should reflect the copied content.
Currently it inherits the source ETag, which is correct for single-part objects
but may differ for objects that were originally uploaded via multipart.

---

## Medium-Term Ideas

These require more design work but address real operational gaps:

### OIDC / STS federated authentication

For Kubernetes and cloud-native environments, the right authentication model is
OIDC federation via the S3 STS API (`AssumeRoleWithWebIdentity`). The flow:

1. A workload authenticates with an OIDC provider (Keycloak, Dex, a Kubernetes
   service account token issuer, etc.) and receives a JWT.
2. The workload calls `STS AssumeRoleWithWebIdentity` with that JWT.
3. MaxIOS3 validates the JWT against the provider's JWKS endpoint, extracts
   claims, and maps them to a local role or policy.
4. MaxIOS3 returns short-lived `AccessKeyId` / `SecretAccessKey` /
   `SessionToken` credentials.
5. The workload uses those temporary credentials for normal S3 operations.

All major S3 SDKs (aws-sdk-go, boto3, aws-sdk-java, s3fs-fuse, etc.) support
this flow natively. MinIO implements it. It is the standard path for pod-level
storage access in Kubernetes without distributing long-lived secrets.

PAM-based authentication is explicitly not planned: it couples authentication to
the host OS, breaks in container environments, and does not map cleanly to the
access-key / secret-key model.

This feature builds on the user management API (see near-term section) and
requires a roles/policies model to map OIDC claims to access permissions.

### Object lifecycle enforcement

The lifecycle XML is stored and returned correctly but expiration rules are not
evaluated. A background task could delete objects that have passed their
configured expiration date.

### Multi-node / clustered storage backend with intelligent sharding

The `StorageEngine` and metadata traits are designed to be pluggable. A
multi-node deployment could use consistent hashing to assign objects to nodes,
with any node able to handle any request by either issuing a 307 redirect to the
owning node (transparent to all S3 clients) or proxying the request internally.

Key design areas:

- Cluster membership and hash-ring management (gossip or etcd-backed)
- A `ShardedStorageEngine` that routes or proxies based on `hash(bucket/key)`
- PostgreSQL (or a distributed KV store such as TiKV) as the shared metadata
  layer — a prerequisite for any multi-node setup
- Fan-out `ListObjects` that merges results across shards
- Cross-shard `CopyObject` via a saga / two-phase approach
- Multipart uploads pinned to a single node for the lifetime of the upload

Shared-filesystem approaches (NFS, SMB) are explicitly not the target here;
the goal is an architecture where each node owns local storage for its shard.

### Content-addressable object storage

Store object data under the SHA-256 content hash rather than the bucket/key
path. The canonical key mapping lives in metadata. This enables transparent
deduplication at the storage layer — identical content uploaded to different
keys (or different buckets) is stored once. The SHA-256 is already computed on
every `PutObject`, so the storage-layer change is primarily a path-layout
decision.

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
