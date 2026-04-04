# Other Testing Measures

## MinIO Mint Integration Test Suite

[MinIO Mint](https://github.com/minio/mint) is an official S3 compatibility test suite maintained by MinIO. It runs a battery of functional tests against an S3-compatible endpoint using multiple client SDKs simultaneously, making it the most comprehensive third-party compatibility benchmark available.

### Setup

The project uses a local Docker image built from the mint repository (`mein-mint`). A helper script wraps the Docker invocation:

```bash
./run-mint-test.sh <suite-name>
```

The script passes the server endpoint, credentials, and a log volume mount to the container. Test results are written to `/tmp/mint/<suite-name>/log.json` in JSON Lines format, one entry per test function.

To build the local mint image:

```bash
# Clone https://github.com/minio/mint and run docker build -t mein-mint .
# (or pull minio/mint:edge and tag it)
```

### Available Suites

Mint ships with the following test suites, each exercising the server through a different S3 client library:

| Suite | Client Library |
|-------|---------------|
| `awscli` | AWS CLI v2 |
| `aws-sdk-go-v2` | AWS SDK for Go v2 |
| `aws-sdk-java-v2` | AWS SDK for Java v2 |
| `aws-sdk-php` | AWS SDK for PHP |
| `aws-sdk-ruby` | AWS SDK for Ruby |
| `minio-go` | MinIO Go client |
| `minio-java` | MinIO Java client |
| `minio-js` | MinIO JavaScript client |
| `minio-py` | MinIO Python client |
| `mc` | MinIO Client (`mc`) CLI |
| `s3cmd` | s3cmd CLI |
| `healthcheck` | MinIO health and metrics endpoints |
| `versioning` | S3 bucket versioning API |
| `s3select` | S3 Select (SQL queries on objects) |

### Current Results (as of 2026-04-04)

11 out of 13 runnable suites pass completely. The two remaining failures are not caused by server bugs — see the notes below.

| Suite | Status | Notes |
|-------|--------|-------|
| `healthcheck` | PASS | |
| `awscli` | PASS | |
| `aws-sdk-go-v2` | PASS | |
| `aws-sdk-java-v2` | PASS | |
| `aws-sdk-php` | PASS | |
| `aws-sdk-ruby` | PASS | |
| `minio-go` | PASS | |
| `minio-js` | PASS | |
| `minio-py` | PASS | |
| `s3cmd` | PASS | |
| `versioning` | PASS | All versioning tests skipped (`NA`) — server returns `NotImplemented`; the SDK sets `isVersioningSupported=false` and skips all versioning tests gracefully |
| `mc` | FAIL (1 test) | `test_admin_users` fails — see note below |
| `minio-java` | FAIL (1 test) | `getBucketCors` fails — see note below |
| `s3select` | not run | Requires a full SQL engine (aggregates, JSON input, date functions). Too complex for the current scope. |

### Known Unfixable Failures

#### `minio-java` — `getBucketCors`

**Root cause: MinIO Java SDK bug, not a server bug.**

The test calls `setBucketCors()` (which passes) and then `getBucketCors()`, expecting to round-trip the configuration. The SDK's `CORSConfiguration` class has no `equals()` implementation, so `assertEquals(expected, actual)` always throws `AssertionFailedError` — regardless of what the server returns. This error extends `Error` (not `Exception`), so it bypasses the SDK's normal error-handling path and surfaces as an unrecoverable failure.

Returning the correct CORS XML from the server makes no difference: the test cannot pass until the MinIO Java SDK adds an `equals()` method to `CORSConfiguration`. This is confirmed by the fact that `setBucketCors()` passes consistently and the server correctly stores and retrieves the CORS config (verified via `awscli` and `aws-sdk-go-v2` tests, both of which test CORS and pass).

#### `mc` — `test_admin_users`

**Root cause: MinIO proprietary admin API, not part of the S3 standard.**

The `test_admin_users` test uses the MinIO-specific admin API (`/minio/v1/admin/user/add`, `/minio/v1/admin/user/list`, etc.) to create a user named `foo` and then verify it appears in the user list. This API is a MinIO extension with no S3 equivalent and no public specification. Implementing it would require adding a separate admin authentication layer and user management subsystem outside the core S3 protocol scope.

All other `mc` tests pass, covering object operations, pre-signed URLs, mirroring, directory copies, and more.
