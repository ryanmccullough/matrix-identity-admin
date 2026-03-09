# Phase 2 Finish — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Rename provider traits to clarify generic vs. Keycloak-specific roles, then add a reactivate handler to reverse disable/offboard.

**Architecture:** Two independent changes. First, a mechanical rename (`IdentityProvider` → `KeycloakIdentityProvider`, `IdentityProviderApi` → `IdentityProvider`) so the generic trait gets the clean name for Phase 3 pluggability. Second, a new `POST /users/{id}/reactivate` endpoint composing two new lifecycle primitives (`enable_identity_account` + `reactivate_auth_account`).

**Tech Stack:** Rust, Axum, async_trait, sqlx (SQLite), Askama templates

---

### Task 1: Rename `IdentityProvider` → `KeycloakIdentityProvider`

Mechanical find-and-replace of the Keycloak-specific trait name. No behaviour change.

**Files:**
- Modify: `src/clients/keycloak.rs` (trait definition line 17, impl blocks)
- Modify: `src/clients/mod.rs` (re-export)
- Modify: `src/state.rs` (field type)
- Modify: `src/lib.rs` (type annotation)
- Modify: `src/test_helpers.rs` (`impl IdentityProvider for MockKeycloak` → `impl KeycloakIdentityProvider`)
- Modify: `src/services/disable_user.rs` (imports, function signature, mock impl)
- Modify: `src/services/offboard_user.rs` (imports, function signature, mock impl)
- Modify: `src/services/delete_user.rs` (imports, function signature, mock impl)
- Modify: `src/services/invite_user.rs` (imports, function signature, mock impl)
- Modify: `src/services/lifecycle_steps.rs` (imports, function signatures, mock impl)

**Step 1: Rename the trait definition in `src/clients/keycloak.rs`**

Change line 17:
```rust
// Before
pub trait IdentityProvider: Send + Sync {
// After
pub trait KeycloakIdentityProvider: Send + Sync {
```

**Step 2: Update the `impl` block for `KeycloakClient`**

In `src/clients/keycloak.rs`, the `impl IdentityProvider for KeycloakClient` block (around line 75) becomes:
```rust
impl KeycloakIdentityProvider for KeycloakClient {
```

**Step 3: Update the `IdentityProviderApi` impl disambiguation calls**

In `src/clients/keycloak.rs` lines 388-445, every `IdentityProvider::method(self, ...)` call becomes `KeycloakIdentityProvider::method(self, ...)`. There are 6 calls:
- Line 396: `IdentityProvider::search_users` → `KeycloakIdentityProvider::search_users`
- Line 414: `IdentityProvider::get_user` → `KeycloakIdentityProvider::get_user`
- Line 429: `IdentityProvider::get_user_groups` → `KeycloakIdentityProvider::get_user_groups`
- Line 434: `IdentityProvider::get_user_roles` → `KeycloakIdentityProvider::get_user_roles`
- Line 439: `IdentityProvider::logout_user` → `KeycloakIdentityProvider::logout_user`
- Line 443: `IdentityProvider::count_users` → `KeycloakIdentityProvider::count_users`

**Step 4: Update `src/clients/mod.rs`**

```rust
// Before
pub use keycloak::{IdentityProvider, KeycloakClient};
// After
pub use keycloak::{KeycloakIdentityProvider, KeycloakClient};
```

**Step 5: Update `src/state.rs`**

Line 9 import and line 20 field type:
```rust
// Before
use crate::clients::{AuthService, IdentityProvider, MatrixService};
pub keycloak: Arc<dyn IdentityProvider>,
// After
use crate::clients::{AuthService, KeycloakIdentityProvider, MatrixService};
pub keycloak: Arc<dyn KeycloakIdentityProvider>,
```

**Step 6: Update `src/lib.rs`**

Line 40:
```rust
// Before
let keycloak: Arc<dyn clients::IdentityProvider> =
// After
let keycloak: Arc<dyn clients::KeycloakIdentityProvider> =
```

**Step 7: Update `src/test_helpers.rs`**

- Line 11 import: `IdentityProvider` → `KeycloakIdentityProvider`
- Line 83: `impl IdentityProvider for MockKeycloak` → `impl KeycloakIdentityProvider for MockKeycloak`
- Line 448: `Arc<dyn IdentityProvider>` → `Arc<dyn KeycloakIdentityProvider>`

**Step 8: Update all service files**

In each of these files, rename `IdentityProvider` → `KeycloakIdentityProvider` in imports, function parameter types, and test mock `impl` blocks:

- `src/services/disable_user.rs`: import (line 2), param type (line 16), mock impl (line 127)
- `src/services/offboard_user.rs`: import (line 9), param type (line 26), mock impl (line 201)
- `src/services/delete_user.rs`: import (line 4), param type (line 20), mock impl (line 156)
- `src/services/invite_user.rs`: import (line 4), param type (line 27), mock impl (line 222)
- `src/services/lifecycle_steps.rs`: import (line 11), param types (lines 103, 147), mock impl (line 375)

**Step 9: Run tests to verify**

Run: `flox activate -- cargo test`
Expected: All tests pass — no behaviour change, just a rename.

**Step 10: Commit**

```bash
git add src/clients/keycloak.rs src/clients/mod.rs src/state.rs src/lib.rs \
  src/test_helpers.rs src/services/disable_user.rs src/services/offboard_user.rs \
  src/services/delete_user.rs src/services/invite_user.rs src/services/lifecycle_steps.rs
git commit -m "$(cat <<'EOF'
refactor(keycloak): rename IdentityProvider to KeycloakIdentityProvider

Clarify that this trait is Keycloak-specific (returns KeycloakUser,
KeycloakGroup, has mutations like create_user and disable_user).
Prepares the clean name for the generic pluggable trait in Task 2.
EOF
)"
```

---

### Task 2: Rename `IdentityProviderApi` → `IdentityProvider`

Give the generic trait the clean name for Phase 3 pluggability.

**Files:**
- Modify: `src/clients/identity_provider.rs` (trait definition line 11)
- Modify: `src/clients/keycloak.rs` (import line 7, impl block line 389)
- Modify: `src/clients/mod.rs` (re-export line 6)
- Modify: `src/lib.rs` (import line 25, type annotation line 44)
- Modify: `src/test_helpers.rs` (import line 11, impl block line 179, state builder lines 449-450)
- Modify: `src/services/user_service.rs` (import line 4, field type line 32, constructor line 39, mock impl line 193)

**Step 1: Rename the trait in `src/clients/identity_provider.rs`**

Line 11:
```rust
// Before
pub trait IdentityProviderApi: Send + Sync {
// After
pub trait IdentityProvider: Send + Sync {
```

**Step 2: Update `src/clients/keycloak.rs`**

- Line 7: `use crate::clients::identity_provider::IdentityProviderApi;` → `use crate::clients::identity_provider::IdentityProvider;`
- Line 389: `impl IdentityProviderApi for KeycloakClient` → `impl IdentityProvider for KeycloakClient`

**Step 3: Update `src/clients/mod.rs`**

Line 6:
```rust
// Before
pub use identity_provider::IdentityProviderApi;
// After
pub use identity_provider::IdentityProvider;
```

**Step 4: Update `src/lib.rs`**

- Line 25: `use clients::{IdentityProviderApi, KeycloakClient, MasClient, SynapseClient};` → `use clients::{IdentityProvider, KeycloakClient, MasClient, SynapseClient};`
- Line 44: `let identity_provider: Arc<dyn IdentityProviderApi> =` → `let identity_provider: Arc<dyn IdentityProvider> =`

**Step 5: Update `src/test_helpers.rs`**

- Line 11 import: `IdentityProviderApi` → `IdentityProvider` (note: now both `IdentityProvider` and `KeycloakIdentityProvider` are imported — `IdentityProvider` from `identity_provider.rs`, `KeycloakIdentityProvider` from `keycloak.rs`)
- Line 172 doc comment: update `IdentityProviderApi` → `IdentityProvider`
- Line 179: `impl IdentityProviderApi for MockKeycloak` → `impl IdentityProvider for MockKeycloak`
- Line 449: `Arc<dyn IdentityProviderApi>` → `Arc<dyn IdentityProvider>`
- Line 444 comment: `implements both IdentityProvider and IdentityProviderApi` → `implements both KeycloakIdentityProvider and IdentityProvider`

**Step 6: Update `src/services/user_service.rs`**

- Line 4: `use crate::clients::{AuthService, IdentityProviderApi};` → `use crate::clients::{AuthService, IdentityProvider};`
- Line 32: `identity_provider: Arc<dyn IdentityProviderApi>,` → `identity_provider: Arc<dyn IdentityProvider>,`
- Line 39: `identity_provider: Arc<dyn IdentityProviderApi>,` → `identity_provider: Arc<dyn IdentityProvider>,`
- Line ~193 (mock): `impl IdentityProviderApi for MockIdP` → `impl IdentityProvider for MockIdP`

**Step 7: Run tests to verify**

Run: `flox activate -- cargo test`
Expected: All tests pass — no behaviour change, just a rename.

**Step 8: Commit**

```bash
git add src/clients/identity_provider.rs src/clients/keycloak.rs src/clients/mod.rs \
  src/lib.rs src/test_helpers.rs src/services/user_service.rs
git commit -m "$(cat <<'EOF'
refactor: rename IdentityProviderApi to IdentityProvider

The generic pluggable-backend trait gets the clean name. UserService
depends on IdentityProvider (not a Keycloak-specific type), enabling
future backends like Authentik or LDAP without touching the service
layer.
EOF
)"
```

---

### Task 3: Add `enable_user` to `KeycloakIdentityProvider` trait + impl

Mirror of `disable_user` — PUT to Keycloak admin API with `{"enabled": true}`.

**Files:**
- Modify: `src/clients/keycloak.rs` (trait + impl)
- Modify: `src/test_helpers.rs` (add `fail_enable` flag + mock method)
- Modify: `src/services/lifecycle_steps.rs` (mock — add `enable_user` stub)
- Modify: `src/services/disable_user.rs` (mock — add `enable_user` stub)
- Modify: `src/services/offboard_user.rs` (mock — add `enable_user` stub)
- Modify: `src/services/delete_user.rs` (mock — add `enable_user` stub)
- Modify: `src/services/invite_user.rs` (mock — add `enable_user` stub)

**Step 1: Add method to trait in `src/clients/keycloak.rs`**

After `disable_user` (line 38), add:
```rust
    /// Enable a user account in Keycloak (sets enabled = true).
    async fn enable_user(&self, user_id: &str) -> Result<(), AppError>;
```

**Step 2: Add implementation in `KeycloakClient`**

After the `disable_user` impl (after line 363), add:
```rust
    async fn enable_user(&self, user_id: &str) -> Result<(), AppError> {
        let token = self.admin_token().await?;
        let url = self.admin_url(&format!("/users/{user_id}"));

        #[derive(Serialize)]
        struct EnableBody {
            enabled: bool,
        }

        self.http
            .put(&url)
            .bearer_auth(&token)
            .json(&EnableBody { enabled: true })
            .send()
            .await
            .map_err(|e| upstream_error("keycloak", e))?
            .error_for_status()
            .map_err(|e| upstream_error("keycloak", e))?;

        Ok(())
    }
```

**Step 3: Add `fail_enable` to `MockKeycloak` in `src/test_helpers.rs`**

Add field:
```rust
    pub fail_enable: bool,
```

Add to `Default`:
```rust
    fail_enable: false,
```

Add to `impl KeycloakIdentityProvider for MockKeycloak`:
```rust
    async fn enable_user(&self, _user_id: &str) -> Result<(), AppError> {
        if self.fail_enable {
            Err(AppError::Upstream {
                service: "keycloak".into(),
                message: "mock enable_user failure".into(),
            })
        } else {
            Ok(())
        }
    }
```

**Step 4: Add `enable_user` stubs to all test mock `impl KeycloakIdentityProvider` blocks**

In each of these files, add to the mock impl:
```rust
        async fn enable_user(&self, _: &str) -> Result<(), AppError> {
            Ok(())
        }
```

Files:
- `src/services/lifecycle_steps.rs` (MockKeycloak impl, around line 375)
- `src/services/disable_user.rs` (MockKc impl, around line 127)
- `src/services/offboard_user.rs` (MockKc impl, around line 201)
- `src/services/delete_user.rs` (MockKc impl, around line 156)
- `src/services/invite_user.rs` (MockKc impl, around line 222)

**Step 5: Run tests**

Run: `flox activate -- cargo test`
Expected: All tests pass.

**Step 6: Commit**

```bash
git add src/clients/keycloak.rs src/test_helpers.rs \
  src/services/lifecycle_steps.rs src/services/disable_user.rs \
  src/services/offboard_user.rs src/services/delete_user.rs \
  src/services/invite_user.rs
git commit -m "$(cat <<'EOF'
feat(keycloak): add enable_user to KeycloakIdentityProvider

Mirror of disable_user — PUT to Keycloak admin API with
{"enabled": true}. Needed by the reactivate workflow.
EOF
)"
```

---

### Task 4: Add `enable_identity_account` and `reactivate_auth_account` lifecycle primitives

Two new composable steps in `src/services/lifecycle_steps.rs`.

**Files:**
- Modify: `src/services/lifecycle_steps.rs` (add functions + tests)

**Step 1: Write failing tests**

Add to the test module in `src/services/lifecycle_steps.rs`:

Add `fail_enable: bool` to the test-local `MockKeycloak` struct (around line 370) and wire it:
```rust
    struct MockKeycloak {
        fail_logout: bool,
        fail_disable: bool,
        fail_enable: bool,
    }
```

Add `enable_user` to the test-local mock impl:
```rust
        async fn enable_user(&self, _: &str) -> Result<(), AppError> {
            if self.fail_enable {
                Err(AppError::Upstream {
                    service: "keycloak".into(),
                    message: "mock enable failure".into(),
                })
            } else {
                Ok(())
            }
        }
```

Add `fail_reactivate: bool` to the test-local `MockMas` struct and wire it:
```rust
    struct MockMas {
        user: Option<MasUser>,
        sessions: Vec<MasSession>,
        fail_finish: bool,
        fail_delete: bool,
        fail_reactivate: bool,
    }
```

Add to the test-local `MockMas` impl:
```rust
        async fn reactivate_user(&self, _: &str) -> Result<(), AppError> {
            if self.fail_reactivate {
                Err(AppError::Upstream {
                    service: "mas".into(),
                    message: "mock reactivate failure".into(),
                })
            } else {
                Ok(())
            }
        }
```

Then add tests:
```rust
    // ── enable_identity_account ─────────────────────────────────────────────────

    #[tokio::test]
    async fn enable_account_success() {
        let audit = audit_svc().await;
        let keycloak = MockKeycloak {
            fail_logout: false,
            fail_disable: false,
            fail_enable: false,
        };

        let result = enable_identity_account(
            "reactivate",
            "kc-1",
            "alice",
            "@alice:example.com",
            &keycloak,
            &audit,
            "sub",
            "admin",
        )
        .await;

        assert!(result.is_ok());
        let logs = audit.for_user("kc-1", 10).await.unwrap();
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].action, "enable_identity_account_on_reactivate");
        assert_eq!(logs[0].result, "success");
    }

    #[tokio::test]
    async fn enable_account_failure_returns_error() {
        let audit = audit_svc().await;
        let keycloak = MockKeycloak {
            fail_logout: false,
            fail_disable: false,
            fail_enable: true,
        };

        let result = enable_identity_account(
            "reactivate",
            "kc-1",
            "alice",
            "@alice:example.com",
            &keycloak,
            &audit,
            "sub",
            "admin",
        )
        .await;

        assert!(result.is_err());
        let logs = audit.for_user("kc-1", 10).await.unwrap();
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].action, "enable_identity_account_on_reactivate");
        assert_eq!(logs[0].result, "failure");
    }

    // ── reactivate_auth_account ─────────────────────────────────────────────────

    #[tokio::test]
    async fn reactivate_auth_account_success() {
        let audit = audit_svc().await;
        let mas = MockMas {
            user: None,
            sessions: vec![],
            fail_finish: false,
            fail_delete: false,
            fail_reactivate: false,
        };

        let result = reactivate_auth_account(
            "reactivate",
            "kc-1",
            "mas-001",
            "alice",
            "@alice:example.com",
            &mas,
            &audit,
            "sub",
            "admin",
        )
        .await;

        assert!(!result.has_warnings());
        let logs = audit.for_user("kc-1", 10).await.unwrap();
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].action, "reactivate_auth_account_on_reactivate");
        assert_eq!(logs[0].result, "success");
    }

    #[tokio::test]
    async fn reactivate_auth_account_failure_is_warning() {
        let audit = audit_svc().await;
        let mas = MockMas {
            user: None,
            sessions: vec![],
            fail_finish: false,
            fail_delete: false,
            fail_reactivate: true,
        };

        let result = reactivate_auth_account(
            "reactivate",
            "kc-1",
            "mas-001",
            "alice",
            "@alice:example.com",
            &mas,
            &audit,
            "sub",
            "admin",
        )
        .await;

        assert!(result.has_warnings());
        assert!(result.warnings[0].contains("reactivate"));
        let logs = audit.for_user("kc-1", 10).await.unwrap();
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].action, "reactivate_auth_account_on_reactivate");
        assert_eq!(logs[0].result, "failure");
    }
```

**Step 2: Run tests to verify they fail**

Run: `flox activate -- cargo test lifecycle_steps`
Expected: Compilation error — `enable_identity_account` and `reactivate_auth_account` don't exist yet.

**Step 3: Implement the primitives**

Add after `disable_identity_account` in `src/services/lifecycle_steps.rs` (after line 177):

```rust
/// Enable a user account in Keycloak.
///
/// Fatal: returns `Err` on failure (after audit logging the failure).
#[allow(clippy::too_many_arguments)]
pub(crate) async fn enable_identity_account(
    context: &str,
    keycloak_id: &str,
    username: &str,
    matrix_user_id: &str,
    keycloak: &dyn KeycloakIdentityProvider,
    audit: &AuditService,
    admin_subject: &str,
    admin_username: &str,
) -> Result<(), AppError> {
    let action = format!("enable_identity_account_on_{context}");

    let result = keycloak.enable_user(keycloak_id).await;
    let audit_result = if result.is_ok() {
        AuditResult::Success
    } else {
        AuditResult::Failure
    };

    let _ = audit
        .log(
            admin_subject,
            admin_username,
            Some(keycloak_id),
            Some(matrix_user_id),
            &action,
            audit_result,
            json!({
                "keycloak_user_id": keycloak_id,
                "username": username,
            }),
        )
        .await;

    result
}

/// Reactivate a previously deactivated MAS user account.
///
/// Non-fatal: failure adds a warning to the outcome rather than returning an
/// error. The MAS account may not exist or may already be active.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn reactivate_auth_account(
    context: &str,
    keycloak_id: &str,
    auth_user_id: &str,
    username: &str,
    matrix_user_id: &str,
    mas: &dyn AuthService,
    audit: &AuditService,
    admin_subject: &str,
    admin_username: &str,
) -> WorkflowOutcome {
    let mut outcome = WorkflowOutcome::ok();
    let action = format!("reactivate_auth_account_on_{context}");

    let result = mas.reactivate_user(auth_user_id).await;
    let audit_result = if result.is_ok() {
        AuditResult::Success
    } else {
        AuditResult::Failure
    };

    let _ = audit
        .log(
            admin_subject,
            admin_username,
            Some(keycloak_id),
            Some(matrix_user_id),
            &action,
            audit_result,
            json!({
                "auth_user_id": auth_user_id,
                "username": username,
            }),
        )
        .await;

    if let Err(e) = result {
        tracing::warn!(error = %e, "Auth account reactivation failed during {context}");
        outcome.add_warning(format!("Auth account reactivate failed: {e}"));
    }

    outcome
}
```

Note: `enable_identity_account` takes `&dyn KeycloakIdentityProvider` (not `&dyn IdentityProvider`) because `enable_user` is a Keycloak-specific mutation. The existing `disable_identity_account` also needs updating to use `&dyn KeycloakIdentityProvider` — it currently takes `&dyn IdentityProvider` which was the old name for the Keycloak-specific trait. After Task 1, `IdentityProvider` refers to the generic trait which does NOT have `disable_user`. This is a compile error that Task 1 should have caught.

**IMPORTANT**: In Task 1 step 8, when renaming `IdentityProvider` → `KeycloakIdentityProvider` in `lifecycle_steps.rs`, the import on line 11 must change from `IdentityProvider` to `KeycloakIdentityProvider`, AND the param types on lines 103 and 147 must also change. Double-check that this was done correctly — if `IdentityProvider` still refers to the generic trait after Task 2, `disable_user` and `logout_user` won't be on it.

**Step 4: Run tests to verify they pass**

Run: `flox activate -- cargo test lifecycle_steps`
Expected: All tests pass, including the 4 new tests.

**Step 5: Commit**

```bash
git add src/services/lifecycle_steps.rs
git commit -m "$(cat <<'EOF'
feat(lifecycle): add enable and reactivate primitives

enable_identity_account — fatal, calls keycloak.enable_user()
reactivate_auth_account — non-fatal, calls mas.reactivate_user()
Both follow the existing lifecycle_steps pattern with audit logging.
EOF
)"
```

---

### Task 5: Create `reactivate_user` workflow service

Composes lifecycle primitives into a reactivate sequence (mirror of `disable_user`).

**Files:**
- Create: `src/services/reactivate_user.rs`
- Modify: `src/services/mod.rs` (add module)

**Step 1: Write failing tests**

Create `src/services/reactivate_user.rs` with full test module. The workflow:
1. Fetch Keycloak user → username, matrix_user_id
2. `enable_identity_account("reactivate", ...)` — fatal
3. Look up MAS user by username
4. If found and deactivated: `reactivate_auth_account("reactivate", ...)` — non-fatal
5. If not found or already active: skip with no warning

```rust
use crate::{
    clients::{AuthService, KeycloakIdentityProvider},
    error::AppError,
    models::workflow::WorkflowOutcome,
    services::{lifecycle_steps, AuditService},
};

/// Reactivate a previously disabled/offboarded user account.
///
/// Composes lifecycle primitives into a reactivate sequence:
///   1. Fetch the Keycloak user to resolve the username and Matrix ID.
///   2. Enable the identity account (fatal — error returned to caller).
///   3. Look up the MAS user by username.
///   4. If found and deactivated, reactivate the auth account (non-fatal).
pub async fn reactivate_user(
    keycloak_id: &str,
    keycloak: &dyn KeycloakIdentityProvider,
    mas: &dyn AuthService,
    audit: &AuditService,
    admin_subject: &str,
    admin_username: &str,
    homeserver_domain: &str,
) -> Result<WorkflowOutcome, AppError> {
    let kc_user = keycloak.get_user(keycloak_id).await?;
    let username = &kc_user.username;
    let matrix_user_id = format!("@{}:{}", username, homeserver_domain);

    lifecycle_steps::enable_identity_account(
        "reactivate",
        keycloak_id,
        username,
        &matrix_user_id,
        keycloak,
        audit,
        admin_subject,
        admin_username,
    )
    .await?;

    let mut outcome = WorkflowOutcome::ok();

    let auth_user = match mas.get_user_by_username(username).await {
        Ok(Some(u)) => Some(u),
        Ok(None) => None,
        Err(e) => {
            tracing::warn!(error = %e, "Auth user lookup failed during reactivate; skipping auth reactivation");
            outcome.add_warning(format!("Auth user lookup failed: {e}"));
            None
        }
    };

    if let Some(ref u) = auth_user {
        if u.deactivated_at.is_some() {
            let reactivate_outcome = lifecycle_steps::reactivate_auth_account(
                "reactivate",
                keycloak_id,
                &u.id,
                username,
                &matrix_user_id,
                mas,
                audit,
                admin_subject,
                admin_username,
            )
            .await;
            outcome.warnings.extend(reactivate_outcome.warnings);
        }
    }

    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use async_trait::async_trait;
    use sqlx::sqlite::SqlitePoolOptions;

    use super::*;
    use crate::{
        clients::{AuthService, KeycloakIdentityProvider},
        models::keycloak::{KeycloakGroup, KeycloakRole, KeycloakUser},
        models::mas::{MasSession, MasUser},
        services::AuditService,
    };

    // ── Test helpers ──────────────────────────────────────────────────────────

    fn kc_user(id: &str, username: &str) -> KeycloakUser {
        KeycloakUser {
            id: id.to_string(),
            username: username.to_string(),
            email: Some(format!("{username}@example.com")),
            first_name: None,
            last_name: None,
            enabled: false,
            email_verified: true,
            created_timestamp: None,
            required_actions: vec![],
        }
    }

    async fn audit_svc() -> AuditService {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();
        AuditService::new(pool)
    }

    // ── Mock Keycloak ─────────────────────────────────────────────────────────

    struct MockKc {
        user: Option<KeycloakUser>,
        fail_enable: bool,
    }

    #[async_trait]
    impl KeycloakIdentityProvider for MockKc {
        async fn search_users(
            &self,
            _: &str,
            _: u32,
            _: u32,
        ) -> Result<Vec<KeycloakUser>, AppError> {
            Ok(vec![])
        }
        async fn count_users(&self, _: &str) -> Result<u32, AppError> {
            Ok(0)
        }
        async fn get_user(&self, _: &str) -> Result<KeycloakUser, AppError> {
            self.user
                .clone()
                .ok_or_else(|| AppError::NotFound("not found".into()))
        }
        async fn get_user_by_email(&self, _: &str) -> Result<Option<KeycloakUser>, AppError> {
            Ok(None)
        }
        async fn get_user_groups(&self, _: &str) -> Result<Vec<KeycloakGroup>, AppError> {
            Ok(vec![])
        }
        async fn get_user_roles(&self, _: &str) -> Result<Vec<KeycloakRole>, AppError> {
            Ok(vec![])
        }
        async fn logout_user(&self, _: &str) -> Result<(), AppError> {
            Ok(())
        }
        async fn create_user(&self, _: &str, _: &str) -> Result<String, AppError> {
            Ok("id".into())
        }
        async fn send_invite_email(&self, _: &str) -> Result<(), AppError> {
            Ok(())
        }
        async fn delete_user(&self, _: &str) -> Result<(), AppError> {
            Ok(())
        }
        async fn disable_user(&self, _: &str) -> Result<(), AppError> {
            Ok(())
        }
        async fn enable_user(&self, _: &str) -> Result<(), AppError> {
            if self.fail_enable {
                Err(AppError::Upstream {
                    service: "keycloak".into(),
                    message: "mock enable failure".into(),
                })
            } else {
                Ok(())
            }
        }
    }

    // ── Mock MAS ──────────────────────────────────────────────────────────────

    struct MockMs {
        user: Option<MasUser>,
        fail_reactivate: bool,
    }

    #[async_trait]
    impl AuthService for MockMs {
        async fn get_user_by_username(&self, _: &str) -> Result<Option<MasUser>, AppError> {
            Ok(self.user.clone())
        }
        async fn list_sessions(&self, _: &str) -> Result<Vec<MasSession>, AppError> {
            Ok(vec![])
        }
        async fn finish_session(&self, _: &str, _: &str) -> Result<(), AppError> {
            Ok(())
        }
        async fn delete_user(&self, _: &str) -> Result<(), AppError> {
            Ok(())
        }
        async fn reactivate_user(&self, _: &str) -> Result<(), AppError> {
            if self.fail_reactivate {
                Err(AppError::Upstream {
                    service: "mas".into(),
                    message: "mock reactivate failure".into(),
                })
            } else {
                Ok(())
            }
        }
    }

    fn deactivated_mas_user(username: &str) -> MasUser {
        MasUser {
            id: "mas-001".to_string(),
            username: username.to_string(),
            deactivated_at: Some("2026-01-01T00:00:00Z".to_string()),
        }
    }

    fn active_mas_user(username: &str) -> MasUser {
        MasUser {
            id: "mas-001".to_string(),
            username: username.to_string(),
            deactivated_at: None,
        }
    }

    // ── Tests ─────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn reactivate_succeeds_with_no_mas_user() {
        let audit = audit_svc().await;
        let kc = Arc::new(MockKc {
            user: Some(kc_user("kc-1", "alice")),
            fail_enable: false,
        });
        let mas = Arc::new(MockMs {
            user: None,
            fail_reactivate: false,
        });

        let outcome = reactivate_user(
            "kc-1",
            kc.as_ref(),
            mas.as_ref(),
            &audit,
            "sub",
            "admin",
            "example.com",
        )
        .await
        .unwrap();

        assert!(!outcome.has_warnings());
        let logs = audit.for_user("kc-1", 10).await.unwrap();
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].action, "enable_identity_account_on_reactivate");
        assert_eq!(logs[0].result, "success");
    }

    #[tokio::test]
    async fn reactivate_enables_keycloak_and_reactivates_mas() {
        let audit = audit_svc().await;
        let kc = Arc::new(MockKc {
            user: Some(kc_user("kc-1", "alice")),
            fail_enable: false,
        });
        let mas = Arc::new(MockMs {
            user: Some(deactivated_mas_user("alice")),
            fail_reactivate: false,
        });

        let outcome = reactivate_user(
            "kc-1",
            kc.as_ref(),
            mas.as_ref(),
            &audit,
            "sub",
            "admin",
            "example.com",
        )
        .await
        .unwrap();

        assert!(!outcome.has_warnings());
        let logs = audit.for_user("kc-1", 10).await.unwrap();
        assert_eq!(logs.len(), 2);
        let actions: Vec<&str> = logs.iter().map(|l| l.action.as_str()).collect();
        assert!(actions.contains(&"enable_identity_account_on_reactivate"));
        assert!(actions.contains(&"reactivate_auth_account_on_reactivate"));
        assert!(logs.iter().all(|l| l.result == "success"));
    }

    #[tokio::test]
    async fn reactivate_skips_active_mas_user() {
        let audit = audit_svc().await;
        let kc = Arc::new(MockKc {
            user: Some(kc_user("kc-1", "alice")),
            fail_enable: false,
        });
        let mas = Arc::new(MockMs {
            user: Some(active_mas_user("alice")),
            fail_reactivate: false,
        });

        let outcome = reactivate_user(
            "kc-1",
            kc.as_ref(),
            mas.as_ref(),
            &audit,
            "sub",
            "admin",
            "example.com",
        )
        .await
        .unwrap();

        assert!(!outcome.has_warnings());
        let logs = audit.for_user("kc-1", 10).await.unwrap();
        // Only the enable, no reactivate (MAS user already active)
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].action, "enable_identity_account_on_reactivate");
    }

    #[tokio::test]
    async fn reactivate_mas_failure_is_warning() {
        let audit = audit_svc().await;
        let kc = Arc::new(MockKc {
            user: Some(kc_user("kc-1", "alice")),
            fail_enable: false,
        });
        let mas = Arc::new(MockMs {
            user: Some(deactivated_mas_user("alice")),
            fail_reactivate: true,
        });

        let outcome = reactivate_user(
            "kc-1",
            kc.as_ref(),
            mas.as_ref(),
            &audit,
            "sub",
            "admin",
            "example.com",
        )
        .await
        .unwrap();

        assert!(outcome.has_warnings());
        assert!(outcome.warnings[0].contains("reactivate"));
    }

    #[tokio::test]
    async fn reactivate_enable_failure_returns_error() {
        let audit = audit_svc().await;
        let kc = Arc::new(MockKc {
            user: Some(kc_user("kc-1", "alice")),
            fail_enable: true,
        });
        let mas = Arc::new(MockMs {
            user: None,
            fail_reactivate: false,
        });

        let result = reactivate_user(
            "kc-1",
            kc.as_ref(),
            mas.as_ref(),
            &audit,
            "sub",
            "admin",
            "example.com",
        )
        .await;
        assert!(result.is_err());

        let logs = audit.for_user("kc-1", 10).await.unwrap();
        assert_eq!(logs[0].action, "enable_identity_account_on_reactivate");
        assert_eq!(logs[0].result, "failure");
    }

    #[tokio::test]
    async fn reactivate_user_not_found_returns_error() {
        let audit = audit_svc().await;
        let kc = Arc::new(MockKc {
            user: None,
            fail_enable: false,
        });
        let mas = Arc::new(MockMs {
            user: None,
            fail_reactivate: false,
        });

        let result = reactivate_user(
            "kc-1",
            kc.as_ref(),
            mas.as_ref(),
            &audit,
            "sub",
            "admin",
            "example.com",
        )
        .await;
        assert!(result.is_err());
        let logs = audit.for_user("kc-1", 10).await.unwrap();
        assert!(logs.is_empty());
    }
}
```

**Step 2: Register the module in `src/services/mod.rs`**

Add:
```rust
pub mod reactivate_user;
```

**Step 3: Run tests**

Run: `flox activate -- cargo test reactivate_user`
Expected: All 6 tests pass.

**Step 4: Commit**

```bash
git add src/services/reactivate_user.rs src/services/mod.rs
git commit -m "$(cat <<'EOF'
feat(lifecycle): add reactivate_user workflow

Reverses disable/offboard — enables the Keycloak account and
reactivates the MAS account if deactivated. MAS reactivation is
non-fatal (account may not exist or may already be active).
EOF
)"
```

---

### Task 6: Create reactivate handler + route + UI button

**Files:**
- Create: `src/handlers/reactivate.rs`
- Modify: `src/handlers/mod.rs` (add module)
- Modify: `src/lib.rs` (add route)
- Modify: `src/test_helpers.rs` (add route to `mutations_router`)
- Modify: `templates/user_detail.html` (add Reactivate button)

**Step 1: Create `src/handlers/reactivate.rs`**

Mirror of `src/handlers/disable.rs`:

```rust
use axum::{
    extract::{Path, State},
    response::{IntoResponse, Redirect},
    Form,
};
use serde::Deserialize;

use crate::{
    auth::{csrf::validate, session::AuthenticatedAdmin},
    error::AppError,
    services::reactivate_user::reactivate_user,
    state::AppState,
    utils::pct_encode,
};

#[derive(Deserialize)]
pub struct ReactivateForm {
    pub _csrf: String,
}

/// POST /users/{id}/reactivate
///
/// Enables the Keycloak account and reactivates the MAS account if
/// deactivated. Both operations are audit-logged. On success, redirects
/// back to the user detail page.
pub async fn reactivate(
    AuthenticatedAdmin(admin): AuthenticatedAdmin,
    State(state): State<AppState>,
    Path(keycloak_id): Path<String>,
    Form(form): Form<ReactivateForm>,
) -> Result<impl IntoResponse, AppError> {
    validate(&admin.csrf_token, &form._csrf)?;

    let outcome = reactivate_user(
        &keycloak_id,
        state.keycloak.as_ref(),
        state.mas.as_ref(),
        &state.audit,
        &admin.subject,
        &admin.username,
        &state.config.homeserver_domain,
    )
    .await?;

    let redirect = if outcome.has_warnings() {
        let mut warning = pct_encode(&outcome.warning_summary());
        if warning.len() > 400 {
            warning.truncate(400);
            warning.push_str("%E2%80%A6"); // …
        }
        format!("/users/{keycloak_id}?warning={warning}")
    } else {
        format!("/users/{keycloak_id}?notice=User+reactivated")
    };

    Ok(Redirect::to(&redirect))
}

#[cfg(test)]
mod tests {
    use axum::{
        body::Body,
        http::{Method, Request, StatusCode},
    };
    use tower::ServiceExt;

    use crate::{
        models::{keycloak::KeycloakUser, mas::MasUser},
        test_helpers::{
            build_test_state_full, make_auth_cookie, mutations_router, MockKeycloak, MockMas,
            TEST_CSRF,
        },
    };

    fn test_kc_user() -> KeycloakUser {
        KeycloakUser {
            id: "kc-123".to_string(),
            username: "testuser".to_string(),
            email: Some("test@example.com".to_string()),
            first_name: None,
            last_name: None,
            enabled: false,
            email_verified: true,
            created_timestamp: None,
            required_actions: vec![],
        }
    }

    fn test_mas_user() -> MasUser {
        MasUser {
            id: "mas-456".to_string(),
            username: "testuser".to_string(),
            deactivated_at: Some("2026-01-01T00:00:00Z".to_string()),
        }
    }

    async fn post_reactivate(
        state: crate::state::AppState,
        user_id: &str,
        csrf: &str,
        auth_cookie: Option<&str>,
    ) -> axum::response::Response {
        let body = format!("_csrf={csrf}");
        let mut builder = Request::builder()
            .method(Method::POST)
            .uri(format!("/users/{user_id}/reactivate"))
            .header("content-type", "application/x-www-form-urlencoded");
        if let Some(cookie) = auth_cookie {
            builder = builder.header("cookie", cookie);
        }
        mutations_router(state)
            .oneshot(builder.body(Body::from(body)).unwrap())
            .await
            .unwrap()
    }

    // ── Tests ─────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn reactivate_success_redirects_with_notice() {
        let state = build_test_state_full(
            MockKeycloak {
                users: vec![test_kc_user()],
                ..Default::default()
            },
            MockMas::default(),
            "secret",
            None,
        )
        .await;
        let cookie = make_auth_cookie(TEST_CSRF);
        let resp = post_reactivate(state, "kc-123", TEST_CSRF, Some(&cookie)).await;
        assert_eq!(resp.status(), StatusCode::SEE_OTHER);
        let location = resp.headers().get("location").unwrap().to_str().unwrap();
        assert!(location.contains("/users/kc-123"));
        assert!(location.contains("notice=User+reactivated"));
    }

    #[tokio::test]
    async fn reactivate_unauthenticated_redirects_to_login() {
        let state = build_test_state_full(
            MockKeycloak {
                users: vec![test_kc_user()],
                ..Default::default()
            },
            MockMas::default(),
            "secret",
            None,
        )
        .await;
        let resp = post_reactivate(state, "kc-123", TEST_CSRF, None).await;
        assert_eq!(resp.status(), StatusCode::SEE_OTHER);
        assert_eq!(resp.headers().get("location").unwrap(), "/auth/login");
    }

    #[tokio::test]
    async fn reactivate_invalid_csrf_returns_400() {
        let state = build_test_state_full(
            MockKeycloak {
                users: vec![test_kc_user()],
                ..Default::default()
            },
            MockMas::default(),
            "secret",
            None,
        )
        .await;
        let cookie = make_auth_cookie(TEST_CSRF);
        let resp = post_reactivate(state, "kc-123", "wrong-csrf", Some(&cookie)).await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn reactivate_keycloak_failure_returns_502() {
        let state = build_test_state_full(
            MockKeycloak {
                users: vec![test_kc_user()],
                fail_enable: true,
                ..Default::default()
            },
            MockMas::default(),
            "secret",
            None,
        )
        .await;
        let cookie = make_auth_cookie(TEST_CSRF);
        let resp = post_reactivate(state, "kc-123", TEST_CSRF, Some(&cookie)).await;
        assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
    }

    #[tokio::test]
    async fn reactivate_user_not_found_returns_404() {
        let state =
            build_test_state_full(MockKeycloak::default(), MockMas::default(), "secret", None)
                .await;
        let cookie = make_auth_cookie(TEST_CSRF);
        let resp = post_reactivate(state, "nonexistent", TEST_CSRF, Some(&cookie)).await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn reactivate_with_mas_account_writes_audit_logs() {
        let state = build_test_state_full(
            MockKeycloak {
                users: vec![test_kc_user()],
                ..Default::default()
            },
            MockMas {
                user: Some(test_mas_user()),
                ..Default::default()
            },
            "secret",
            None,
        )
        .await;
        let audit = std::sync::Arc::clone(&state.audit);
        let cookie = make_auth_cookie(TEST_CSRF);
        post_reactivate(state, "kc-123", TEST_CSRF, Some(&cookie)).await;
        let logs = audit.for_user("kc-123", 10).await.unwrap();
        let actions: Vec<&str> = logs.iter().map(|l| l.action.as_str()).collect();
        assert!(actions.contains(&"enable_identity_account_on_reactivate"));
        assert!(actions.contains(&"reactivate_auth_account_on_reactivate"));
        assert!(logs.iter().all(|l| l.result == "success"));
    }
}
```

**Step 2: Register the handler module in `src/handlers/mod.rs`**

Add:
```rust
pub mod reactivate;
```

**Step 3: Add route in `src/lib.rs`**

After the disable route (line 109), add:
```rust
        .route("/users/{id}/reactivate", post(handlers::reactivate::reactivate))
```

**Step 4: Add route to `mutations_router` in `src/test_helpers.rs`**

After the disable route in `mutations_router` (around line 597), add:
```rust
        .route(
            "/users/{id}/reactivate",
            post(crate::handlers::reactivate::reactivate),
        )
```

**Step 5: Add Reactivate button to `templates/user_detail.html`**

After the Disable User button form (around line 59), add:
```html
  <!-- Reactivate user -->
  <form method="post" action="/users/{{ user.keycloak_id }}/reactivate" style="margin-top:0.5rem"
        onsubmit="return confirm('Reactivate {{ user.username }}?')">
    <input type="hidden" name="_csrf" value="{{ csrf_token }}">
    <button type="submit" class="btn btn-success">Reactivate User</button>
  </form>
```

**Step 6: Run tests**

Run: `flox activate -- cargo test`
Expected: All tests pass (including 6 new handler tests).

**Step 7: Commit**

```bash
git add src/handlers/reactivate.rs src/handlers/mod.rs src/lib.rs \
  src/test_helpers.rs templates/user_detail.html
git commit -m "$(cat <<'EOF'
feat(lifecycle): add POST /users/{id}/reactivate handler

Reverses disable/offboard — re-enables Keycloak account and
reactivates MAS account. CSRF-protected, audit-logged, with
Reactivate button on user detail page.
EOF
)"
```

---

### Task 7: Update CLAUDE.md + final verification

Update documentation to reflect the renamed traits and new endpoint.

**Files:**
- Modify: `CLAUDE.md` (trait names, route list, roadmap)

**Step 1: Update Client Traits section in CLAUDE.md**

Replace the `KeycloakApi` trait block with updated names showing both traits:
```rust
#[async_trait]
pub trait KeycloakIdentityProvider: Send + Sync {
    async fn search_users(&self, query: &str, max: u32, first: u32) -> Result<Vec<KeycloakUser>>;
    async fn count_users(&self, query: &str) -> Result<u32>;
    async fn get_user(&self, user_id: &str) -> Result<KeycloakUser>;
    async fn get_user_groups(&self, user_id: &str) -> Result<Vec<KeycloakGroup>>;
    async fn get_user_roles(&self, user_id: &str) -> Result<Vec<KeycloakRole>>;
    async fn logout_user(&self, user_id: &str) -> Result<()>;
    async fn create_user(&self, username: &str, email: &str) -> Result<String>;
    async fn disable_user(&self, user_id: &str) -> Result<()>;
    async fn enable_user(&self, user_id: &str) -> Result<()>;
    // ... more Keycloak-specific methods
}

#[async_trait]
pub trait IdentityProvider: Send + Sync {
    async fn search_users(&self, query: &str, max: u32, first: u32) -> Result<Vec<CanonicalUser>>;
    async fn get_user(&self, id: &str) -> Result<CanonicalUser>;
    async fn get_user_groups(&self, id: &str) -> Result<Vec<String>>;
    async fn get_user_roles(&self, id: &str) -> Result<Vec<String>>;
    async fn logout_user(&self, id: &str) -> Result<()>;
    async fn count_users(&self, query: &str) -> Result<u32>;
}
```

**Step 2: Add reactivate to the handler list**

Add to the Interface layer handlers section:
```
  reactivate.rs  # POST /users/{id}/reactivate
```

**Step 3: Add reactivate_user to the Workflow layer**

Update the services listing to include `reactivate_user`.

**Step 4: Update Roadmap Phase 2 to mark complete**

Ensure Phase 2 shows as done.

**Step 5: Run full pre-commit gate**

```bash
flox activate -- cargo fmt
flox activate -- cargo clippy --all-targets -- -D warnings
flox activate -- cargo test
```

All three must pass.

**Step 6: Commit**

```bash
git add CLAUDE.md
git commit -m "$(cat <<'EOF'
docs: update CLAUDE.md for trait rename and reactivate

Reflect KeycloakIdentityProvider / IdentityProvider rename and new
POST /users/{id}/reactivate endpoint. Phase 2 complete.
EOF
)"
```
