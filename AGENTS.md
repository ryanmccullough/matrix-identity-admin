# AGENTS.md

Guidance for AI coding agents (Claude Code, Codex, etc.) working in this repository.

Read this file before starting any task. It explains the project's direction, decision rules, and where things belong.

---

## One-sentence direction

`matrix-identity-admin` is evolving into the **identity and lifecycle control plane for self-hosted Matrix infrastructure.**

It is not a thin read-only console. It is not a Synapse wrapper. It is the system that manages users, sessions, groups, and access across Keycloak, MAS, and eventually Synapse — the equivalent of Slack Admin or Google Workspace Admin for self-hosted Matrix.

---

## Current state vs target state

| Concern | Current | Target |
|---------|---------|--------|
| Architecture | Handlers → services → clients | Domain → Workflows → Connectors → Interface |
| User model | External API structs + thin unified view | Canonical internal `User` with `LifecycleState` |
| Operations | Discrete admin actions | Explicit lifecycle workflows |
| Group access | Not implemented | Group → Space → Room policy enforcement |
| Reconciliation | Not implemented | Periodic drift detection and correction |
| Synapse | Preserved but unused | Used via Matrix client API for room management |

The move from current to target happens **incrementally**. Do not rewrite working code.

---

## Four architectural layers

Every piece of code belongs to one layer. When adding or modifying code, identify which layer it belongs to before writing.

### Layer 1: Domain (`src/models/`)
Internal concepts that represent organizational state — not upstream API shapes.

**Lives here:**
- `User` (canonical internal model with lifecycle state, external IDs)
- `LifecycleState` (invited, active, suspended, disabled, offboarded)
- `GroupMapping` (policy: group → spaces/rooms)
- `AuditEvent`
- `Invite`

**Does not live here:** raw Keycloak/MAS response structs (those go in connector-specific models).

**Rule:** Domain models must not import connector types. They represent what the app knows, not what an upstream returned.

---

### Layer 2: Connectors (`src/clients/`)
Everything that talks to an external system.

**Lives here:**
- HTTP requests, auth headers, token management
- Typed request/response structs per upstream
- Error conversion from upstream errors to `AppError`
- Retry logic, timeouts

**Current connectors:**
- `clients/keycloak.rs` — Keycloak admin API
- `clients/mas.rs` — MAS admin API (OAuth2 client credentials, token cache)
- `clients/synapse.rs` — NOT compiled; preserved for Matrix client API

**Rule:** Connectors must not contain business logic. They return typed results. Callers decide what to do with them.

---

### Layer 3: Workflows (`src/services/`)
Multi-step business logic coordinating connectors and domain state.

**Lives here:**
- `invite_user` — create Keycloak user, set required actions, audit log
- `disable_user` — revoke MAS sessions, force Keycloak logout, audit log
- `offboard_user` — disable + remove room memberships + deactivate MAS account
- `reconcile_membership` — check group membership drift, correct it

**Current services (being evolved into workflows):**
- `services/user_service.rs` — aggregates Keycloak + MAS into unified models
- `services/identity_mapper.rs` — derives Keycloak → MAS → Matrix ID correlation
- `services/audit_service.rs` — writes audit log entries

**Rule:** Workflows must not leak connector types into their return values. Return domain types. Do not put workflow logic in handlers.

---

### Layer 4: Interface (`src/handlers/`, `templates/`)
Thin HTTP handlers, API routes, and templates.

**Lives here:**
- Route registration and HTTP method enforcement
- Input parsing, CSRF validation
- Calling the right workflow
- Rendering templates or returning JSON

**Rule:** No business logic in handlers. If a handler is doing more than "parse input → call workflow → render output", extract the logic into a workflow.

---

## Decision checklist — before starting any task

Answer these questions before writing code:

1. **Which layer does this belong to?** Domain / Connector / Workflow / Interface?
2. **Does this fit the scope?** Identity, lifecycle, access, or administration?
3. **Am I duplicating logic?** If yes, extract a workflow or connector instead.
4. **Should I refactor first?** See the build vs refactor rule below.
5. **Can this be a small increment?** Prefer narrow changes over broad ones.

If a feature touches all four layers at once, stop and identify the one layer boundary to clean up first.

---

## Build vs refactor rule

### Build directly when:
- The feature fits scope cleanly and you can explain where it belongs
- It touches only a few files
- The existing code is imperfect but understandable
- It doesn't require redefining core state

### Do a small refactor first when:
- You're about to duplicate logic that belongs in a shared workflow or connector
- Vendor API calls are mixed into handlers
- The feature is multi-step and failure handling is unclear
- User/account state is represented inconsistently across files

### Consider a larger redesign only when:
- Every feature requires invasive cross-cutting changes
- There is no usable separation between layers
- State is fundamentally contradictory

Default: **keep building, refactor only at the boundary that the next feature stresses.**

---

## Identity correlation model

```
Keycloak User (keycloak_user.id = stable OIDC subject)
   ↓
MAS account (correlated via subject claim in OIDC token)
   ↓
Matrix user (@{keycloak_user.username}:{homeserver_domain})
```

- `Confirmed` — Keycloak user + MAS account both found
- `Inferred` — Keycloak only; Matrix ID derived by convention

Never silently assert a correlation is valid when it isn't. Always surface the status.

---

## Lifecycle states

When adding user state management, use these states:

| State | Meaning |
|-------|---------|
| `invited` | Invite sent; user has not logged in yet |
| `active` | User has logged in; sessions exist |
| `suspended` | Temporarily blocked; sessions revoked |
| `disabled` | Account disabled in Keycloak and MAS |
| `offboarded` | Fully removed; room memberships cleared |

Transitions between states are implemented as **workflows**, not ad-hoc handler logic.

---

## Audit logging — required for every mutation

Every state-changing operation must write an audit log entry:

```
id, timestamp, admin_subject, admin_username,
target_keycloak_user_id, target_matrix_user_id,
action, result (success/failure), metadata_json
```

Write the audit entry regardless of whether the upstream operation succeeded or failed. Record `result: failure` with the error in `metadata_json`.

---

## Security — non-negotiable rules

- All mutating endpoints: POST-only + CSRF validation
- All protected routes: require `APP_REQUIRED_ADMIN_ROLE`
- All upstream tokens: server-side only, never sent to browser
- All `reqwest` calls: must have explicit timeouts
- Never log secrets, tokens, or credentials

---

## MSC3861 — Synapse integration note

In MSC3861 mode, Synapse delegates auth to MAS. MAS-issued compat tokens (`mct_`) cannot access the Synapse admin API.

**Current approach:** MAS is the session/device source of truth. Revoking a MAS compat session invalidates the corresponding Matrix device.

**Future:** Synapse will be integrated via the **Matrix client API** (not admin API) — for room joins, invites, and space management. The connector stub is at `src/clients/synapse.rs`.

Do not wire in Synapse admin API calls. Do not use `mct_` tokens against `/admin`.

---

## What belongs in this project vs what doesn't

### In scope
- Identity lifecycle management (invite → active → disabled → offboarded)
- Group → Space → Room access policy
- Session and device management via MAS
- Keycloak user and group management
- Audit logging of all admin actions
- Reconciliation of group membership drift

### Out of scope (not now, may never be)
- Full moderation platform
- Full observability/metrics suite
- Federation governance or reputation
- SCIM
- Multi-realm support
- User self-service portal
- Encryption key management
- Generic room management unrelated to identity

---

## Standards — non-negotiable

These apply to every change, every time. There are no exceptions for small changes or "just a quick fix."

### Pre-commit gate

Run all three before committing:

```bash
flox activate -- cargo fmt
flox activate -- cargo clippy --all-targets -- -D warnings
flox activate -- cargo test
```

If any fail, fix them. Do not commit with a failing check. Do not use `--no-verify`.

### Branch naming

Format: `type/short-description` in kebab-case. Match the commit type.

```
feat/lifecycle-state-model      ← new feature
fix/mas-token-refresh           ← bug fix
refactor/extract-disable-workflow
test/disable-handler-coverage
ci/add-deny-check
chore/update-pr-template
docs/update-agents-standards
```

### Commit format (Conventional Commits)

```
type(scope): short imperative description   ← 50 chars max, no period
                                            ← blank line
Why this change was made.                   ← body at 72 chars, explain why not what
```

Types: `feat`, `fix`, `refactor`, `perf`, `test`, `docs`, `ci`, `build`, `chore`
Scope: the affected area, lowercase — `feat(invite)`, `fix(mas)`, `refactor(auth)`

Subject rules:
- Imperative mood — "add", "fix", "remove" not "added", "fixes", "removed"
- Lowercase after the colon
- No period at the end
- Completes: "If applied, this commit will: ___"

Use the `/commit` skill when creating commits.

### Coding style

**Comments — when to write them:**
- `///` doc comments on all public types, traits, and non-trivial public functions
- `//` inline comments only where the code is non-obvious — never narrate what the code clearly shows
- `// NOTE:` for critical non-obvious behavior (ordering requirements, upstream API quirks)
- `// TODO:` for known limitations to address later
- Section dividers `// ── Label ───────────────────────────────────────────────` only inside `#[cfg(test)]` blocks; never in production code

**Comments — what not to write:**

```rust
// Bad — narrates what the code already shows
let token = self.admin_token().await?;  // get the admin token

// Good — explains a non-obvious constraint
// Subtract 30 s from expiry as a safety margin to avoid using a token
// that expires mid-request.
let expires_at = now + Duration::from_secs(expires_in.saturating_sub(30));
```

**Naming conventions:**
- Types: `PascalCase`; functions and variables: `snake_case`; constants: `SCREAMING_SNAKE_CASE`
- Concrete client implementations: `XxxClient` (e.g. `KeycloakClient`, `MasClient`)
- Test functions: `{what}_{condition}_{expected_result}` — e.g. `revoke_invalid_csrf_returns_400`

**Error handling:**
- No `unwrap()` or `expect()` in production code paths — use `?` or explicit handling
- Use `upstream_error(service, err)` for all reqwest errors — never construct `AppError::Upstream` manually
- Use `?` propagation where the error type converts cleanly; explicit `match` only when branching on the error variant matters

**Imports:**
- Grouped: `std` → external crates → `crate::` — blank line between groups
- `use super::*` only inside `#[cfg(test)]` modules, nowhere else
- `rustfmt` enforces ordering automatically — run `cargo fmt` to fix

### Testing requirements

**Every new handler** must have tests covering:
1. Success — correct redirect or response body
2. Unauthenticated — redirects to `/auth/login`
3. Invalid CSRF — returns 400
4. Upstream failure — returns 502
5. Audit log written on success (for mutating handlers)

**Every new service/workflow function** must have unit tests with mock implementations covering:
1. Happy path returns the correct result
2. Each upstream failure mode — test graceful degradation vs hard error per the contract

**Every new model** with `Display` or non-trivial derived behavior must have basic assertion tests.

**Coverage must not regress.** CI reports coverage via Codecov. If coverage drops on a PR, investigate before merging. Do not exclude new files from coverage without justification.

### CI gates — all must be green before merging

| Check | What it runs | Blocks merge |
|-------|-------------|-------------|
| Formatting | `cargo fmt --check` | Yes |
| Compilation | `cargo check --all-targets` | Yes |
| Lint | `cargo clippy --all-targets -- -D warnings` | Yes |
| Tests | `cargo test` | Yes |
| Coverage | `cargo llvm-cov` + Codecov | Review on regression |
| Security | `cargo audit` | Yes (push to main + weekly) |

Do not push to a branch with known CI failures. Do not ask to merge a PR with failing checks. If CI is red, diagnose and fix it — do not work around it.

---

## Good prompt patterns for this codebase

Use small, focused prompts with a single clear objective.

**Good examples:**
- "Extract the MAS API calls from `handlers/sessions.rs` into a method on `MasClient`."
- "Create a `disable_user` workflow in `services/` that revokes MAS sessions and forces Keycloak logout."
- "Add a `LifecycleState` enum to `models/unified.rs` and update `UnifiedUserDetail` to include it."
- "Refactor `user_service.rs` to return domain types instead of raw `KeycloakUser`."
- "Add a `reconcile_group_membership` workflow stub that compares Keycloak groups to Matrix room memberships."

**Avoid:**
- "Rewrite the architecture."
- "Clean up the whole codebase."
- "Redesign everything to match the vision."
- Prompts that span all four layers at once

**Why:** Small prompts produce better code, waste fewer tokens, and are easier to review and revert.

---

## Roadmap — what comes next

### Phase 1 — Trustworthy (mostly done)
- [x] Reliable invite flow with Keycloak required actions
- [x] MAS session revocation
- [x] Force Keycloak logout
- [x] Audit logging
- [x] OIDC login, admin role enforcement
- [ ] Explicit `LifecycleState` model
- [ ] Unified disable/offboard workflow

### Phase 2 — Structurally sound (next)
- Extract explicit workflow modules from services
- Group membership reconciliation (Keycloak groups → Matrix room membership)
- Dry-run / preview support for admin actions
- Better error handling across multi-step operations (partial failure tracking)

### Phase 3 — Extensible
- Provider interface for pluggable identity backends
- Policy configuration (group → room mapping as config, not hardcode)
- Swappable notification backends (email, Matrix message)
- Support for more deployment patterns

### Phase 4 — Polished
- Improved admin UI with HTMX interactions
- Bulk actions (invite many, disable many)
- Dashboards and system status views
- Onboarding templates

---

## Environment and build requirements

- Must run inside Flox: `flox activate -- cargo run`
- `libiconv` is provided by Flox — do not add `.cargo/config.toml` linker overrides
- SQLite DB path must exist; `data/` directory is created with `create_if_missing(true)`
- Keycloak realm name is lowercase and case-sensitive

See `CLAUDE.md` for full environment variable reference and external service details.
