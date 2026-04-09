// Tests for bucket policy evaluation logic, covering principal matching, effect
// precedence (explicit Deny overrides Allow), wildcard resources, and multi-action statements.

use super::*;
use crate::metadata::bucket::PolicyStatement;
use serde_json::json;

fn create_test_policy(statements: Vec<PolicyStatement>) -> BucketPolicy {
    BucketPolicy {
        version: "2012-10-17".to_string(),
        statements,
    }
}

#[test]
fn test_allow_all_principal() {
    let policy = create_test_policy(vec![PolicyStatement {
        sid: None,
        effect: "Allow".to_string(),
        principal: json!("*"),
        action: json!("s3:GetObject"),
        resource: json!("arn:aws:s3:::mybucket/*"),
    }]);

    let result = PolicyEvaluator::evaluate(
        &policy,
        "arn:aws:iam:::user/someuser",
        "s3:GetObject",
        "arn:aws:s3:::mybucket/photo.jpg",
    );
    assert_eq!(result, PolicyResult::Allow);
}

#[test]
fn test_deny_specific_user() {
    let policy = create_test_policy(vec![
        PolicyStatement {
            sid: Some("AllowAll".to_string()),
            effect: "Allow".to_string(),
            principal: json!("*"),
            action: json!("s3:*"),
            resource: json!("*"),
        },
        PolicyStatement {
            sid: Some("DenyBadUser".to_string()),
            effect: "Deny".to_string(),
            principal: json!("arn:aws:iam:::user/baduser"),
            action: json!("s3:GetObject"),
            resource: json!("arn:aws:s3:::mybucket/*"),
        },
    ]);

    let result = PolicyEvaluator::evaluate(
        &policy,
        "arn:aws:iam:::user/baduser",
        "s3:GetObject",
        "arn:aws:s3:::mybucket/secret.txt",
    );
    assert_eq!(result, PolicyResult::Deny);

    let result_other = PolicyEvaluator::evaluate(
        &policy,
        "arn:aws:iam:::user/gooduser",
        "s3:GetObject",
        "arn:aws:s3:::mybucket/secret.txt",
    );
    assert_eq!(result_other, PolicyResult::Allow);
}

#[test]
fn test_wildcard_resource() {
    let policy = create_test_policy(vec![PolicyStatement {
        sid: None,
        effect: "Allow".to_string(),
        principal: json!("*"),
        action: json!("s3:GetObject"),
        resource: json!("arn:aws:s3:::mybucket/public/*"),
    }]);

    assert_eq!(
        PolicyEvaluator::evaluate(
            &policy,
            "any",
            "s3:GetObject",
            "arn:aws:s3:::mybucket/public/file.txt"
        ),
        PolicyResult::Allow
    );

    assert_eq!(
        PolicyEvaluator::evaluate(
            &policy,
            "any",
            "s3:GetObject",
            "arn:aws:s3:::mybucket/private/file.txt"
        ),
        PolicyResult::Default
    );
}

#[test]
fn test_multiple_actions() {
    let policy = create_test_policy(vec![PolicyStatement {
        sid: None,
        effect: "Allow".to_string(),
        principal: json!("*"),
        action: json!(["s3:GetObject", "s3:PutObject"]),
        resource: json!("*"),
    }]);

    assert_eq!(
        PolicyEvaluator::evaluate(&policy, "any", "s3:GetObject", "any"),
        PolicyResult::Allow
    );
    assert_eq!(
        PolicyEvaluator::evaluate(&policy, "any", "s3:PutObject", "any"),
        PolicyResult::Allow
    );
    assert_eq!(
        PolicyEvaluator::evaluate(&policy, "any", "s3:DeleteObject", "any"),
        PolicyResult::Default
    );
}
