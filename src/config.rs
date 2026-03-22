/// Configuration management for the MaxIOs3 server.
/// This file defines the `Config` struct and its methods for loading settings from environment variables.
/// It supports loading from a `.env` file and provides sensible defaults for all configuration options.

use std::env;

#[derive(Debug, Clone)]
pub struct Config {
    pub database_url: String,
    pub storage_root: String,
    pub host: String,
    pub port: u16,
    pub log_level: String,
    pub tls_cert: Option<String>,
    pub tls_key: Option<String>,
}

impl Config {
    pub fn from_env() -> Self {
        // Load .env file if it exists
        let _ = dotenvy::dotenv();

        Self {
            database_url: env::var("DATABASE_URL")
                .unwrap_or_else(|_| "sqlite:metadata.db".to_string()),
            storage_root: env::var("STORAGE_ROOT")
                .unwrap_or_else(|_| "./data".to_string()),
            host: env::var("HOST")
                .unwrap_or_else(|_| "0.0.0.0".to_string()),
            port: env::var("PORT")
                .ok()
                .and_then(|p| p.parse().ok())
                .unwrap_or(3000),
            log_level: env::var("LOG_LEVEL")
                .unwrap_or_else(|_| "info".to_string()),
            tls_cert: env::var("TLS_CERT").ok(),
            tls_key: env::var("TLS_KEY").ok(),
        }
    }
}
