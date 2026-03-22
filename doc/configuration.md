# Configuration

Currently, the server uses a mix of hardcoded defaults and simple configuration. Future versions will move these to environment variables or a config file.

## Environment Variables

| Variable | Description | Default |
|----------|-------------|---------|
| `PORT` | Port to listen on | `3000` |
| `HOST` | Bind address | `0.0.0.0` |
| `STORAGE_ROOT` | Path for file storage | `./data` |
| `DATABASE_URL` | SQLite connection string | `sqlite:metadata.db` |
| `LOG_LEVEL` | Tracing log level | `info` |
| `TLS_CERT` | Path to TLS certificate (PEM) | (None) |
| `TLS_KEY` | Path to TLS private key (PEM) | (None) |

### HTTPS Setup

To enable HTTPS, you must set both `TLS_CERT` and `TLS_KEY`. If either is missing, the server will default to HTTP.

```env
TLS_CERT=cert.pem
TLS_KEY=key.pem
```

## Database Setup

The server uses SQLite. On the first run, it will attempt to connect to `metadata.db`. If the database doesn't exist, you may need to run migrations or the server will initialize the schema (depending on implementation).

Currently, the schema includes:
- `buckets`: Stores bucket names and creation dates.
- `objects`: Stores object keys, sizes, and metadata.
- `multipart_uploads`: Tracks active multipart upload sessions.
- `multipart_parts`: Stores individual parts of a multipart upload.
- `users`: Stores access keys, secret keys, and user status.

## Default User (Development)

On startup, if no users exist, the server creates a default development user:
- **Access Key**: `AKIAIOSFODNN7EXAMPLE`
- **Secret Key**: `wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY`
