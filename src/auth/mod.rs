/// Authentication and request signing for the MaxIOs3 server.
/// This file implements the AWS Signature Version 4 (SigV4) validation logic.
/// It includes parsing of authorization headers and query parameters,
/// as well as the construction of canonical requests and string-to-sign components.

use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
use anyhow::{Result, anyhow};
use std::collections::HashMap;
use percent_encoding::percent_decode_str;
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};

pub mod policy;

type HmacSha256 = Hmac<Sha256>;

/// Represents the components of an S3 Signature V4 authorization header
#[derive(Debug, Clone)]
pub struct SigV4Params {
    pub access_key: String,
    pub credential_scope: String,
    pub signed_headers: Vec<String>,
    pub signature: String,
    pub is_presigned: bool,
    pub expires: Option<u64>,
    pub amz_date: Option<String>,
}

impl SigV4Params {
    /// Parses the Authorization header string into SigV4Params
    pub fn parse(header: &str) -> Result<Self> {
        if !header.starts_with("AWS4-HMAC-SHA256 ") {
            return Err(anyhow!("Invalid authorization algorithm"));
        }

        let parts: Vec<&str> = header[17..].split(',').collect();
        let mut params = HashMap::new();

        for part in parts {
            let kv: Vec<&str> = part.trim().splitn(2, '=').collect();
            if kv.len() == 2 {
                params.insert(kv[0], kv[1]);
            }
        }

        let credential = params.get("Credential").ok_or_else(|| anyhow!("Missing Credential"))?;
        let cred_parts: Vec<&str> = credential.splitn(2, '/').collect();

        let access_key = cred_parts[0].to_string();
        let credential_scope = cred_parts[1].to_string();

        let signed_headers = params.get("SignedHeaders")
            .ok_or_else(|| anyhow!("Missing SignedHeaders"))?
            .split(';')
            .map(|s| s.to_string())
            .collect();

        let signature = params.get("Signature")
            .ok_or_else(|| anyhow!("Missing Signature"))?
            .to_string();

        Ok(Self {
            access_key,
            credential_scope,
            signed_headers,
            signature,
            is_presigned: false,
            expires: None,
            amz_date: None,
        })
    }

    /// Parses SigV4 params from query string (for pre-signed URLs)
    pub fn parse_query(query: &HashMap<String, String>) -> Result<Self> {
        let algorithm = query.get("X-Amz-Algorithm").ok_or_else(|| anyhow!("Missing X-Amz-Algorithm"))?;
        if algorithm != "AWS4-HMAC-SHA256" {
            return Err(anyhow!("Unsupported algorithm: {}", algorithm));
        }

        let credential = query.get("X-Amz-Credential").ok_or_else(|| anyhow!("Missing X-Amz-Credential"))?;
        let cred_parts: Vec<&str> = credential.splitn(2, '/').collect();
        let access_key = cred_parts[0].to_string();
        let credential_scope = cred_parts[1].to_string();

        let amz_date = query.get("X-Amz-Date").ok_or_else(|| anyhow!("Missing X-Amz-Date"))?.to_string();
        let expires = query.get("X-Amz-Expires").ok_or_else(|| anyhow!("Missing X-Amz-Expires"))?
            .parse::<u64>().map_err(|_| anyhow!("Invalid X-Amz-Expires"))?;

        let signed_headers = query.get("X-Amz-SignedHeaders")
            .ok_or_else(|| anyhow!("Missing X-Amz-SignedHeaders"))?
            .split(';')
            .map(|s| s.to_string())
            .collect();

        let signature = query.get("X-Amz-Signature").ok_or_else(|| anyhow!("Missing X-Amz-Signature"))?.to_string();

        Ok(Self {
            access_key,
            credential_scope,
            signed_headers,
            signature,
            is_presigned: true,
            expires: Some(expires),
            amz_date: Some(amz_date),
        })
    }
}

pub struct SignatureValidator;

impl SignatureValidator {
    pub fn new() -> Self {
        Self
    }

    /// Validates the request signature
    pub fn validate(
        &self,
        method: &str,
        uri: &str,
        query_params: &[(String, String)],
        headers: &HashMap<String, String>,
        params: &SigV4Params,
        payload_hash: &str,
        secret_key: &str,
    ) -> Result<bool> {
        // 1. Check Expiration for Pre-signed URLs
        if params.is_presigned {
            if let (Some(amz_date), Some(expires)) = (&params.amz_date, params.expires) {
                let request_time = chrono::DateTime::parse_from_str(&format!("{} +0000", amz_date), "%Y%m%dT%H%M%SZ %z")
                    .map_err(|_| anyhow!("Invalid X-Amz-Date format"))?;
                let now = chrono::Utc::now();
                let duration = now.signed_duration_since(request_time);

                if duration.num_seconds() < 0 || duration.num_seconds() > expires as i64 {
                    return Ok(false); // Expired
                }
            }
        }

        let canonical_request = self.create_canonical_request(method, uri, query_params, headers, params, payload_hash);
        let string_to_sign = self.create_string_to_sign(&canonical_request, headers, params)?;
        let signing_key = self.get_signing_key(&params.credential_scope, secret_key)?;

        let mut hmac = HmacSha256::new_from_slice(&signing_key)?;
        hmac.update(string_to_sign.as_bytes());
        let calculated_signature = hex::encode(hmac.finalize().into_bytes());

        if calculated_signature != params.signature {
            tracing::debug!("Signature mismatch!");
            tracing::debug!("Canonical Request:\n{}", canonical_request);
            tracing::debug!("String to Sign:\n{}", string_to_sign);
            tracing::debug!("Expected Signature: {}", params.signature);
            tracing::debug!("Calculated Signature: {}", calculated_signature);
        }

        Ok(calculated_signature == params.signature)
    }

    fn create_canonical_request(
        &self,
        method: &str,
        uri: &str,
        query_params: &[(String, String)],
        headers: &HashMap<String, String>,
        params: &SigV4Params,
        payload_hash: &str,
    ) -> String {
        let canonical_uri = if uri.is_empty() || uri == "/" {
            "/".to_string()
        } else {
            // S3 requires each path segment to be URI-encoded, but not the slashes
            let encoded_path: String = uri.split('/')
                .map(|s| {
                    let decoded = percent_decode_str(s).decode_utf8_lossy();
                    self.url_encode(&decoded)
                })
                .collect::<Vec<_>>()
                .join("/");

            // Ensure it starts with /
            let mut result = if !encoded_path.starts_with('/') {
                format!("/{}", encoded_path)
            } else {
                encoded_path
            };

            // If the original uri ended with a slash, ensure the canonical one does too
            if uri.ends_with('/') && !result.ends_with('/') {
                result.push('/');
            }
            result
        };

        // 2. Canonical Query String
        let mut sorted_params: Vec<_> = query_params.iter()
            .map(|(k, v)| (self.url_encode(k), self.url_encode(v)))
            .collect();
        sorted_params.retain(|(k, _)| k != "X-Amz-Signature");

        // Sort by key, then by value
        sorted_params.sort_by(|a, b| {
            match a.0.cmp(&b.0) {
                std::cmp::Ordering::Equal => a.1.cmp(&b.1),
                other => other,
            }
        });

        let canonical_query_string = sorted_params
            .iter()
            .map(|(k, v)| format!("{}={}", k, v))
            .collect::<Vec<_>>()
            .join("&");

        // 3. Canonical Headers
        let mut header_list: Vec<(String, String)> = Vec::new();
        for header_name in &params.signed_headers {
            let hn = header_name.to_lowercase();
            if let Some(v) = headers.get(&hn) {
                // S3 requires converting sequential spaces to a single space in header values
                let normalized_v = v.split_whitespace().collect::<Vec<_>>().join(" ");
                header_list.push((hn, normalized_v));
            }
        }
        header_list.sort_by_key(|(k, _)| k.clone());

        let mut canonical_headers = String::new();
        for (k, v) in &header_list {
            canonical_headers.push_str(&format!("{}:{}\n", k, v));
        }

        let signed_headers_str = header_list
            .iter()
            .map(|(k, _)| k.clone())
            .collect::<Vec<_>>()
            .join(";");

        format!(
            "{}\n{}\n{}\n{}\n{}\n{}",
            method,
            canonical_uri,
            canonical_query_string,
            canonical_headers,
            signed_headers_str,
            payload_hash
        )
    }

    fn url_encode(&self, s: &str) -> String {
        let mut encoded = String::new();
        for b in s.as_bytes() {
            match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                    encoded.push(*b as char);
                }
                _ => {
                    encoded.push_str(&format!("%{:02X}", b));
                }
            }
        }
        encoded
    }

    fn create_string_to_sign(
        &self,
        canonical_request: &str,
        headers: &HashMap<String, String>,
        params: &SigV4Params,
    ) -> Result<String> {
        let amz_date = if params.is_presigned {
            params.amz_date.as_ref().ok_or_else(|| anyhow!("Missing amz_date in pre-signed params"))?
        } else {
            headers.get("x-amz-date")
                .ok_or_else(|| anyhow!("Missing x-amz-date header"))?
        };

        let mut hasher = Sha256::new();
        hasher.update(canonical_request.as_bytes());
        let hashed_request = hex::encode(hasher.finalize());

        Ok(format!(
            "AWS4-HMAC-SHA256\n{}\n{}\n{}",
            amz_date,
            params.credential_scope,
            hashed_request
        ))
    }

    fn get_signing_key(&self, credential_scope: &str, secret_key: &str) -> Result<Vec<u8>> {
        let scope_parts: Vec<&str> = credential_scope.split('/').collect();
        if scope_parts.len() < 4 {
            return Err(anyhow!("Invalid credential scope"));
        }

        let date = scope_parts[0];
        let region = scope_parts[1];
        let service = scope_parts[2];

        let k_date = self.hmac_sign(format!("AWS4{}", secret_key).as_bytes(), date.as_bytes())?;
        let k_region = self.hmac_sign(&k_date, region.as_bytes())?;
        let k_service = self.hmac_sign(&k_region, service.as_bytes())?;
        let k_signing = self.hmac_sign(&k_service, "aws4_request".as_bytes())?;

        Ok(k_signing)
    }

    fn hmac_sign(&self, key: &[u8], data: &[u8]) -> Result<Vec<u8>> {
        let mut hmac = HmacSha256::new_from_slice(key)?;
        hmac.update(data);
        Ok(hmac.finalize().into_bytes().to_vec())
    }

    /// Validates a SigV2 presigned URL (legacy AWS Signature Version 2)
    /// String-to-sign: METHOD\n\n\nExpires\n/path
    pub fn validate_sigv2_presigned(
        &self,
        method: &str,
        uri: &str,
        expires: &str,
        signature: &str,
        secret_key: &str,
    ) -> Result<bool> {
        use sha1::Sha1;
        type HmacSha1 = Hmac<Sha1>;

        // SigV2 Expires is an absolute Unix timestamp
        let expires_ts: i64 = expires.parse().map_err(|_| anyhow!("Invalid Expires timestamp"))?;
        let now = chrono::Utc::now().timestamp();
        if now > expires_ts {
            tracing::debug!("SigV2 presigned URL expired: now={} > expires={}", now, expires_ts);
            return Ok(false);
        }

        // String-to-sign: METHOD\n\n\nExpires\n/path
        let string_to_sign = format!("{}\n\n\n{}\n{}", method, expires, uri);
        tracing::debug!("SigV2 string-to-sign: {:?}", string_to_sign);

        let mut hmac = HmacSha1::new_from_slice(secret_key.as_bytes())?;
        hmac.update(string_to_sign.as_bytes());
        let calculated = BASE64_STANDARD.encode(hmac.finalize().into_bytes());

        // The provided signature may be URL-encoded (e.g., '+' → '%2B', '=' → '%3D')
        let decoded_sig = percent_decode_str(signature).decode_utf8_lossy().to_string();

        if calculated != decoded_sig {
            tracing::debug!("SigV2 mismatch: calculated={}, expected={}", calculated, decoded_sig);
        }

        Ok(calculated == decoded_sig)
    }

    /// Validates a SigV2 header-based auth (Authorization: AWS access_key:signature)
    /// String-to-sign: METHOD\nContent-MD5\nContent-Type\nDate\n{amz-headers}\n{resource}
    pub fn validate_sigv2_header(
        &self,
        method: &str,
        uri: &str,
        headers: &HashMap<String, String>,
        signature: &str,
        secret_key: &str,
    ) -> Result<bool> {
        use sha1::Sha1;
        type HmacSha1 = Hmac<Sha1>;

        let content_md5 = headers.get("content-md5").map(|s| s.as_str()).unwrap_or("");
        let content_type = headers.get("content-type").map(|s| s.as_str()).unwrap_or("");
        // If x-amz-date is present, the Date line is empty (x-amz-date goes into AmzHeaders)
        let date = if headers.contains_key("x-amz-date") {
            ""
        } else {
            headers.get("date").map(|s| s.as_str()).unwrap_or("")
        };

        // Canonicalized AMZ headers: x-amz-* sorted alphabetically, each ending with \n
        let mut amz_headers: Vec<(String, String)> = headers.iter()
            .filter(|(k, _)| k.starts_with("x-amz-"))
            .map(|(k, v)| (k.clone(), v.trim().to_string()))
            .collect();
        amz_headers.sort_by(|a, b| a.0.cmp(&b.0));
        // Each amz header contributes "key:value\n" — note trailing newline
        let canonicalized_amz = amz_headers.iter()
            .map(|(k, v)| format!("{}:{}\n", k, v))
            .collect::<String>();

        // string-to-sign: METHOD\nMD5\nType\nDate\n{amz headers (each with \n)}Resource
        let string_to_sign = format!("{}\n{}\n{}\n{}\n{}{}", method, content_md5, content_type, date, canonicalized_amz, uri);

        tracing::debug!("SigV2 header string-to-sign: {:?}", string_to_sign);

        let mut hmac = HmacSha1::new_from_slice(secret_key.as_bytes())?;
        hmac.update(string_to_sign.as_bytes());
        let calculated = BASE64_STANDARD.encode(hmac.finalize().into_bytes());

        let decoded_sig = percent_decode_str(signature).decode_utf8_lossy().to_string();

        if calculated != decoded_sig {
            tracing::debug!("SigV2 header mismatch: calculated={}, expected={}", calculated, decoded_sig);
        }

        Ok(calculated == decoded_sig)
    }
}

#[cfg(test)]
#[path = "tests/sigv4.rs"]
mod tests;
