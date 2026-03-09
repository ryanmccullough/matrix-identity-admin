# Phase 2 Finish — Design

## Summary

Two changes to complete Phase 2:

1. Rename provider traits to clarify generic vs. Keycloak-specific roles.
2. Add a reactivate handler to reverse disable/offboard.

---

## Trait Rename

Mechanical rename — no behavior change.

| Current | New | Role |
|---|---|---|
| `IdentityProvider` | `KeycloakIdentityProvider` | Keycloak-specific connector (returns `KeycloakUser`, `KeycloakGroup`, mutations) |
| `IdentityProviderApi` | `IdentityProvider` | Generic trait for pluggable backends (returns `CanonicalUser`, `Vec<String>`) |

Concrete impl stays `KeycloakClient`. `KeycloakClient` implements both traits.

`AppState.keycloak` type changes to `Arc<dyn KeycloakIdentityProvider>`.

`UserService` takes `Arc<dyn IdentityProvider>` (was `IdentityProviderApi`).

File `identity_provider.rs` keeps its name — now holds the generic `IdentityProvider` trait.

All lifecycle steps, handlers, and test mocks update to use `KeycloakIdentityProvider` where they need Keycloak-specific methods, `IdentityProvider` where they use the generic trait.

---

## Reactivate Handler

Reverses a disable — re-enables the Keycloak account and reactivates the MAS account.

### New Keycloak method

`enable_user(id) -> Result<(), AppError>` on `KeycloakIdentityProvider`. PUT to Keycloak admin API with `{"enabled": true}`. Mirror of existing `disable_user`.

### Lifecycle primitives

`src/services/lifecycle_steps.rs`:

- `enable_identity_account(context, ...)` — calls `keycloak.enable_user()`. Fatal on failure. Audit action: `enable_identity_account_on_{context}`.
- `reactivate_auth_account(context, ...)` — calls `mas.reactivate_user()`. Non-fatal — failure adds warning (MAS account may not exist or may already be active). Audit action: `reactivate_auth_account_on_{context}`.

### Workflow

`src/services/reactivate_user.rs`:

```
1. Fetch Keycloak user → username, matrix_user_id
2. enable_identity_account("reactivate", ...) — fatal
3. Look up MAS user by username
4. If found and deactivated: reactivate_auth_account("reactivate", ...) — non-fatal
5. If not found or already active: skip with no warning
```

### Handler

`src/handlers/reactivate.rs` — `POST /users/{id}/reactivate`

- CSRF validated
- Calls `reactivate_user` workflow
- Redirects to `/users/{id}?notice=User+reactivated` or `?warning=...`

### UI

Button on user detail page, visible when user lifecycle state is `Disabled`:

```html
<form method="post" action="/users/{{ user.keycloak_id }}/reactivate"
      onsubmit="return confirm('Reactivate {{ user.username }}?')">
  <input type="hidden" name="_csrf" value="{{ csrf_token }}">
  <button type="submit" class="btn btn-success">Reactivate User</button>
</form>
```

### Tests

Handler: success, unauth, bad CSRF, 404, 502, audit logs.
Service: happy path, no MAS user (skip), MAS reactivate failure (warning), enable failure (error).
Lifecycle steps: enable success/failure, reactivate success/failure.
