# MaxIOs3

A lightweight, S3-compatible object storage server built with Rust, Axum, and SQLite or PostgreSQL.

## Overview

MaxIOs3 implements a broad subset of the Amazon S3 API, allowing applications
to store and retrieve objects using any standard S3 client. It is designed for
simplicity and compatibility: a single process, a pluggable metadata backend
(SQLite for zero-dependency setups, PostgreSQL for production), and the local
file system for object data.

## Features

- **Broad S3 API coverage** — bucket and object CRUD, multipart uploads,
  CopyObject, UploadPartCopy, DeleteObjects, pre-signed URLs (GET/PUT/POST),
  conditional GET and DELETE, byte-range GET, object tagging, bucket CORS /
  life cycle / encryption / policy, GetObjectAttributes, S3 Select (CSV)
- **SigV4 authentication** — AWS Signature Version 4 request signing
- **Bucket policies** — JSON-based access control (Allow/Deny by action,
  resource, and principal)
- **Additional checksum algorithms** — CRC32, CRC32C, SHA-1, SHA-256 alongside
  the standard MD5 ETag
- **Multipart upload garbage collection** — background task cleans up stale
  uploads after 24 hours
- **TLS/HTTPS** — optional, via PEM certificate and key
- **Pluggable metadata backend** — SQLite (zero-dependency, default) or
  PostgreSQL (production, concurrent writes); selected automatically from the
  `DATABASE_URL` scheme, schema created on first run
- **File system storage** — objects stored as plain files under `STORAGE_ROOT`
- **MinIO compatibility extensions** — health/readiness endpoints, Prometheus
  metrics stubs, `x-minio-force-delete` header, pre-signed POST policies

## Quick Start

### Prerequisites

- Rust (latest stable)

### Running

```bash
cargo build

# SQLite (default, zero-dependency)
STORAGE_ROOT=./data DATABASE_URL=sqlite:./metadata.db ./target/debug/maxios3

# PostgreSQL
STORAGE_ROOT=./data DATABASE_URL=postgres://user:pass@localhost/maxios3db ./target/debug/maxios3
```

The server starts on `http://0.0.0.0:3000` by default. On first run it creates
a default development user automatically (see [Configuration](doc/configuration.md)).

### HTTPS

```bash
TLS_CERT=cert.pem TLS_KEY=key.pem ./target/debug/maxios3
```

When both variables are set the server switches to HTTPS automatically.

## Documentation

- [Configuration](doc/configuration.md) — environment variables, database schema, default user
- [Unit Testing](doc/TESTING.md) — running `cargo test`, writing new tests
- [Integration Testing with Mint](doc/Other_testing.md) — S3 compatibility test suite results
- [Road map](doc/roadmap.md) — planned features and known limitations

## S3 Compatibility

MaxIOs3 passes **11 of 13** MinIO Mint integration test suites (see
[doc/Other_testing.md](doc/Other_testing.md) for the full breakdown). The two
remaining failures are not fixable server-side:

- **minio-java `getBucketCors`** — Java SDK bug: `CORSConfiguration` has no `equals()` method
- **mc `test_admin_users`** — requires the MinIO proprietary admin API, not part of S3
