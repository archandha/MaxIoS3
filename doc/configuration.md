# Configuration

MaxIOs3 is configured entirely through environment variables. A `.env` file in
the working directory is loaded automatically on startup.

## Environment Variables

| Variable | Description | Default |
|----------|-------------|---------|
| `PORT` | TCP port to listen on | `3000` |
| `HOST` | Bind address | `0.0.0.0` |
| `STORAGE_ROOT` | Directory where object data is stored | `./data` |
| `DATABASE_URL` | Metadata backend connection string (see [Database](#database)) | `sqlite:metadata.db` |
| `LOG_LEVEL` | Tracing log level (`error`, `warn`, `info`, `debug`, `trace`) | `info` |
| `TLS_CERT` | Path to PEM-encoded TLS certificate | *(unset — HTTP)* |
| `TLS_KEY` | Path to PEM-encoded TLS private key | *(unset — HTTP)* |

### SQLite (default)

```bash
STORAGE_ROOT=./data \
DATABASE_URL=sqlite:./metadata.db \
LOG_LEVEL=warn \
./target/debug/maxios3
```

### PostgreSQL

```bash
STORAGE_ROOT=./data \
DATABASE_URL=postgres://maxios3:secret@localhost/maxios3db \
LOG_LEVEL=warn \
./target/debug/maxios3
```

### HTTPS

Set both `TLS_CERT` and `TLS_KEY` to enable HTTPS. If either is absent the
server falls back to plain HTTP.

```bash
TLS_CERT=/etc/ssl/certs/server.pem \
TLS_KEY=/etc/ssl/private/server.key \
./target/debug/maxios3
```

## Database

MaxIOs3 supports two metadata backends. The backend is selected automatically
from the `DATABASE_URL` scheme — no other configuration is required.

| URL scheme                          | Backend    | When to use                                                       |
|-------------------------------------|------------|-------------------------------------------------------------------|
| `sqlite:…`                          | SQLite     | Development, single-node deployments, zero external dependencies  |
| `postgres://…` or `postgresql://…`  | PostgreSQL | Production, concurrent PUT workloads, future multi-node setups    |

The schema is created automatically on first startup — no manual migration step
is needed for either backend.

### SQLite limitations

SQLite serializes all writes through a single connection. Under concurrent PUT
load this limits metadata throughput. It also cannot be shared across multiple
MaxIOs3 nodes, so SQLite is not suitable for clustered deployments.

### Schema

Both backends share the same logical schema:

| Table | Contents |
|-------|----------|
| `buckets` | Bucket names, creation timestamps |
| `objects` | Object keys, sizes, content types, checksums, storage class, and arbitrary metadata JSON |
| `multipart_uploads` | Active upload sessions (bucket, key, upload ID, metadata) |
| `multipart_parts` | Individual parts for each upload session (part number, size, MD5 checksum) |
| `users` | Access key, secret key, and role for each user |
| `bucket_policies` | Per-bucket JSON access control policies |
| `bucket_lifecycle` | Per-bucket lifecycle XML |
| `bucket_versioning` | Per-bucket versioning status |
| `bucket_object_lock` | Per-bucket object lock configuration XML |
| `bucket_configs` | Generic sub-resource blobs (CORS, encryption, ACL, tagging, etc.) |

## Storage Layout

Object data is stored as plain files under `STORAGE_ROOT`:

```
data/
  <bucket-name>/
    <object-key>           # regular objects
    some/prefix/<key>      # keys with slashes become directory paths
  .temp/
    <upload-id>/
      1                    # part files during multipart upload
      2
      ...
```

Temporary part files are removed automatically after `CompleteMultipartUpload`
or `AbortMultipartUpload`. The background garbage collector also cleans up
uploads that have been open for more than 24 hours.

## Default Development User

On startup, if no users exist in the database, the server creates one:

| Field | Value |
|-------|-------|
| Access Key | `AKIAIOSFODNN7EXAMPLE` |
| Secret Key | `wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY` |
| Role | `admin` |

**Remove this user before exposing the server to untrusted networks.**

To add production users, insert rows directly into the `users` table or
implement the user management API (see [road map](roadmap.md)).
