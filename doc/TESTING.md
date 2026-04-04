# Unit Testing in MaxIOs3

This document explains how to run and write unit tests for the MaxIOs3 project.

## Running Tests

To run all unit tests in the project, use the standard Cargo command:

```bash
cargo test
```

This will compile the project in test mode and execute all functions marked
with `#[test]`.

## Test Structure

Unit tests are located in a `tests/` subdirectory relative to the code they
test. This keeps the main source files clean while allowing tests to access
the module's internal logic.

### Example: Bucket Policy Enforcement

- **Logic**: `src/auth/policy.rs`
- **Tests**: `src/auth/tests/policy.rs`

In the logic file, the tests are included using the `#[path]` attribute:

```rust
#[cfg(test)]
#[path = "tests/policy.rs"]
mod tests;
```

These tests verify:

- **Allow/Deny Logic**: Ensuring that explicit `Deny` statements override
  `Allow` statements.
- **Principal Matching**: Verifying that policies correctly identify users by
  their ARN or handle the wildcard `*`.
- **Action Matching**: Checking that requested actions (e.g., `s3:GetObject`)
  match the actions defined in the policy.
- **Resource Wildcards**: Ensuring that resource patterns like
  `arn:aws:s3:::mybucket/*` correctly match objects within the bucket.

## Writing New Tests

When adding new features, especially those related to security or data
integrity, please add corresponding unit tests.

1. Create a `tests` module at the end of your file:
   ```rust
   #[cfg(test)]
   mod tests {
       use super::*;

       #[test]
       fn my_new_feature_test() {
           // ... test logic ...
       }
   }
   ```
2. Use `assert_eq!`, `assert!`, and other standard macros to verify behavior.
3. For complex structures like JSON, use the `serde_json::json!` macro for easy
   setup.

## Continuous Integration

It is recommended to run `cargo test` before every commit to ensure that no regressions are introduced into the security or core logic of the server.

## Integration Testing

For end-to-end S3 compatibility testing against real client SDKs, see
[Other_testing.md](Other_testing.md). That document describes the MinIO Mint
test suite setup and current pass/fail results across 13 client libraries.
