# MaxIOs3 - Enterprise Storage Server

A high-performance, S3-compatible storage server built with Rust, Axum, and SQLite.

## Overview

MaxIOs3 implements a robust subset of the Amazon S3 API, allowing applications to store and retrieve objects using standard S3 clients. It is designed for enterprise-grade performance and security, with a modular architecture that supports pluggable storage and metadata backends.

## Features

- **S3 API Compatibility**: Supports core S3 operations (ListBuckets, CreateBucket, PutObject, GetObject, DeleteObject).
- **Multipart Uploads**: Efficiently handle large files with multi-part upload support.
- **SigV4 Authentication**: Secure request signing using AWS Signature Version 4.
- **Bucket Policies**: Control access to buckets and objects using JSON policies.
- **TLS/HTTPS Support**: Optional encryption for secure data transfer.
- **SQLite Metadata**: Fast and reliable metadata storage using SQLite.
- **Pluggable Storage**: Abstracted storage engine (defaults to FileSystem).
- **Garbage Collection**: Automatic cleanup of stale multipart uploads.
- **Environment Configuration**: Flexible setup via environment variables and `.env` files.

## Quick Start

### Prerequisites

- Rust (latest stable)
- SQLite

### Running the Server

```bash
cd maxios3
cargo run
```

The server will start on `http://0.0.0.0:3000` by default (configurable via `HOST` and `PORT` environment variables).

### HTTPS Configuration

To enable HTTPS, provide paths to your PEM-encoded certificate and private key:

```bash
export TLS_CERT=cert.pem
export TLS_KEY=key.pem
cargo run
```

When both variables are set, the server will automatically switch to HTTPS mode.

## Documentation

Detailed documentation can be found in the [doc/](./doc/) folder:

- [Design Goals & Architecture](./doc/design.md)
- [Configuration](./doc/configuration.md)
- [Unit Testing](./doc/TESTING.md)
- [Current Status & Roadmap](./doc/status.md)
