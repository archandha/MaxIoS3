# Admin API — User Management

MaxIOs3 exposes a minimal REST API for managing user accounts without requiring
direct database access. All endpoints live under `/minio/v1/admin/`.

---

## Authentication

Every request must be signed with **AWS Signature Version 4** using the
credentials of a user whose `userId` is `"admin"`. Any other valid user receives
a `403 AccessDenied` response.

The default development user (`AKIAIOSFODNN7EXAMPLE`) has `userId = "admin"` and
can call all admin endpoints out of the box.

---

## Endpoints

### List users

```
GET /minio/v1/admin/listusers
```

Returns all user accounts as a JSON object keyed by `accessKey`.

**Response `200 OK`:**

```json
{
  "alice": {
    "secretKey": "s3cr3t",
    "status": "enabled",
    "userId": "alice",
    "createdAt": "2026-04-09T10:00:00Z"
  },
  "bob": {
    "secretKey": "hunter2",
    "status": "disabled",
    "userId": "bob",
    "createdAt": "2026-04-01T08:00:00Z"
  }
}
```

---

### Add or replace a user

```
PUT /minio/v1/admin/adduser?accessKey=<access-key>
Content-Type: application/json
```

Creates a new user or replaces an existing one. If the `accessKey` already
exists its `secretKey` and `userId` are overwritten; `is_active` and
`created_at` are preserved by the database upsert.

**Query parameters:**

| Parameter   | Required | Description                      |
| ----------- | -------- | -------------------------------- |
| `accessKey` | yes      | The AWS-style access key for the new user |

**Request body:**

```json
{
  "secretKey": "the-secret-key",
  "userId": "human-readable-name"
}
```

**Response `200 OK`** (empty body on success)

**Error responses:**

| Status | Code              | Cause                              |
| ------ | ----------------- | ---------------------------------- |
| 400    | `InvalidArgument` | Missing `accessKey` parameter      |
| 400    | `InvalidArgument` | Malformed or missing JSON body     |
| 403    | `AccessDenied`    | Caller is not an admin             |

---

### Remove a user

```
DELETE /minio/v1/admin/removeuser?accessKey=<access-key>
```

Permanently deletes the user account. Deleting a non-existent key is a no-op.

**Query parameters:**

| Parameter   | Required | Description                   |
| ----------- | -------- | ----------------------------- |
| `accessKey` | yes      | Access key of the user to remove |

**Response `204 No Content`** (empty body on success)

---

### Enable or disable a user

```
PUT /minio/v1/admin/setuserinfo?accessKey=<access-key>&status=enabled|disabled
```

Toggles the `is_active` flag on a user account. A disabled user's credentials
are rejected at the authentication layer — all S3 requests return `403
AccessDenied`.

**Query parameters:**

| Parameter   | Required | Description                            |
| ----------- | -------- | -------------------------------------- |
| `accessKey` | yes      | Access key of the user to update       |
| `status`    | yes      | `enabled` or `disabled`               |

**Response `200 OK`** (empty body on success)

**Error responses:**

| Status | Code              | Cause                                      |
| ------ | ----------------- | ------------------------------------------ |
| 400    | `InvalidArgument` | Missing `accessKey` or `status` parameter  |
| 400    | `InvalidArgument` | `status` is not `enabled` or `disabled`    |
| 403    | `AccessDenied`    | Caller is not an admin                     |

---

## Example: curl with SigV4 signing

The easiest way to call the admin API from the command line is via the `aws`
CLI's `--endpoint-url` flag and `s3api` (which handles signing automatically)
or via a tool such as [`awscurl`](https://github.com/okigan/awscurl).

```bash
export AWS_ACCESS_KEY_ID=AKIAIOSFODNN7EXAMPLE
export AWS_SECRET_ACCESS_KEY=wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY
export AWS_DEFAULT_REGION=us-east-1
BASE=http://localhost:3000

# Add a user
awscurl --service s3 -X PUT \
  "$BASE/minio/v1/admin/adduser?accessKey=alice" \
  -H "Content-Type: application/json" \
  -d '{"secretKey":"s3cr3t","userId":"alice"}'

# List all users
awscurl --service s3 "$BASE/minio/v1/admin/listusers"

# Disable a user
awscurl --service s3 -X PUT \
  "$BASE/minio/v1/admin/setuserinfo?accessKey=alice&status=disabled"

# Remove a user
awscurl --service s3 -X DELETE \
  "$BASE/minio/v1/admin/removeuser?accessKey=alice"
```

---

## Integration with external tools

Because the endpoints use standard SigV4 authentication and plain JSON bodies
they work with any HTTP client that supports AWS request signing:

| Tool | How to use |
| ---- | ---------- |
| **HashiCorp Vault** AWS secrets engine | Point the engine at the MaxIOs3 endpoint; Vault calls `adduser` / `removeuser` to vend and revoke short-lived credentials |
| **Kubernetes External Secrets** | Use the AWS provider with a custom endpoint URL pointing at MaxIOs3 |
| **`awscurl`** | See example above |
| **boto3 / aws-sdk-go** | Use a custom endpoint URL and call `PUT`/`DELETE`/`GET` directly with the standard signing middleware |

---

## Security notes

- Only users with `userId == "admin"` can call these endpoints. There is
  currently no fine-grained role model — admin is all-or-nothing.
- The `secretKey` is returned in plaintext by `listusers`. Restrict access to
  the admin API accordingly (firewall, reverse proxy, etc.).
- Remove or rotate the default development credentials
  (`AKIAIOSFODNN7EXAMPLE`) before exposing the server to untrusted networks.
