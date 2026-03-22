/// Bucket policy evaluation for the MaxIOs3 server.
/// This file implements the logic for parsing and evaluating S3-compatible JSON bucket policies.
/// It supports matching principals, actions, and resources with wildcard support.
/// The `PolicyEvaluator` determines if a request should be allowed or denied based on the defined policies.

use serde_json::Value;
use crate::metadata::bucket::BucketPolicy;

pub struct PolicyEvaluator;

impl PolicyEvaluator {
    pub fn evaluate(
        policy: &BucketPolicy,
        principal_arn: &str,
        action: &str,
        resource_arn: &str,
    ) -> PolicyResult {
        let mut result = PolicyResult::Default;

        for statement in &policy.statements {
            if Self::matches_principal(&statement.principal, principal_arn)
                && Self::matches_action(&statement.action, action)
                && Self::matches_resource(&statement.resource, resource_arn)
            {
                if statement.effect == "Deny" {
                    return PolicyResult::Deny;
                } else if statement.effect == "Allow" {
                    result = PolicyResult::Allow;
                }
            }
        }

        result
    }

    fn matches_principal(principal: &Value, principal_arn: &str) -> bool {
        match principal {
            Value::String(s) => s == "*" || s == principal_arn,
            Value::Object(obj) => {
                if let Some(aws) = obj.get("AWS") {
                    match aws {
                        Value::String(s) => s == "*" || s == principal_arn,
                        Value::Array(arr) => arr.iter().any(|v| v.as_str() == Some("*") || v.as_str() == Some(principal_arn)),
                        _ => false,
                    }
                } else {
                    false
                }
            }
            _ => false,
        }
    }

    fn matches_action(action: &Value, requested_action: &str) -> bool {
        match action {
            Value::String(s) => Self::wildcard_match(s, requested_action),
            Value::Array(arr) => arr.iter().any(|v| {
                if let Some(s) = v.as_str() {
                    Self::wildcard_match(s, requested_action)
                } else {
                    false
                }
            }),
            _ => false,
        }
    }

    fn matches_resource(resource: &Value, requested_resource: &str) -> bool {
        match resource {
            Value::String(s) => Self::wildcard_match(s, requested_resource),
            Value::Array(arr) => arr.iter().any(|v| {
                if let Some(s) = v.as_str() {
                    Self::wildcard_match(s, requested_resource)
                } else {
                    false
                }
            }),
            _ => false,
        }
    }

    fn wildcard_match(pattern: &str, text: &str) -> bool {
        if pattern == "*" {
            return true;
        }
        // Simple wildcard match: handle "*" at the end
        if pattern.ends_with('*') {
            let prefix = &pattern[..pattern.len() - 1];
            return text.starts_with(prefix);
        }
        pattern == text
    }
}

#[derive(Debug, PartialEq)]
pub enum PolicyResult {
    Allow,
    Deny,
    Default,
}

#[cfg(test)]
#[path = "tests/policy.rs"]
mod tests;
