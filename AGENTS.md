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
| Group access | DB-backed policy bindings, `/policy` admin UI | Group → Space → Room policy enforcement |
| Reconciliation | Per-user + bulk reconcile from DB policy | Periodic drift detection and correction |
| Synapse | Active — room reconciliation, device listing, user lookup | Group → Space → Room policy enforcement |

The move from current to target happens **incrementally**. Do not rewrite working code.

---

## Four architectural layers

Every piece of code belongs to one layer. When adding or modifying code, identify which layer it belongs to before writing.

### Layer 1: Domain (`src/models/`)
Internal concepts that represent organizational state — not upstream API shapes.

**Lives here:**
- `User` (canonical internal model with lifecycle state, external IDs)
- `LifecycleState` (invited, active, suspended, disabled, offboarded)
- `GroupMapping` (legacy bootstrap config: group → rooms)
- `PolicyBinding` (DB-backed policy: group → room with per-binding options)
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
- `clients/synapse.rs` — Matrix client API + Synapse admin API (`mas-cli`-provisioned compat token with `urn:synapse:admin:*` scope); used for room membership reconciliation, device listing, user lookup

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
- `services/policy_service.rs` — CRUD for policy bindings, effective binding resolution, bootstrap from legacy config

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

## Synapse / MAS integration note

Synapse delegates auth entirely to MAS via `matrix_authentication_service` config. All tokens are validated by MAS introspection using a shared secret.

`SynapseClient` requires `SYNAPSE_ADMIN_TOKEN` — a `mas-cli`-provisioned compat token with `urn:synapse:admin:*` scope. Regular `mct_` tokens from `m.login.password` do not get admin scope and cannot access `/_synapse/admin/*`.

Provision admin tokens with: `mas-cli manage issue-compatibility-token <user> --yes-i-want-to-grant-synapse-admin-privileges`

Admin API endpoints are used where no client API equivalent exists (e.g. force-joining a user to a room, listing room members). Client API endpoints are used where they suffice (e.g. kicking a user).

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

### PR workflow

All non-trivial changes (features, fixes, refactors, CI) go through a branch + PR. **Never push directly to `main`** except for documentation-only or config-only changes with no code impact.

When starting any task:
1. Check which branch you are on — if on `main`, create a branch first
2. Name the branch `type/short-description` in kebab-case (see below)
3. Do all work on the branch
4. Run the pre-commit gate before committing
5. Commit using the `/commit` skill
6. Push and open a PR: `git push -u origin <branch>` then `gh pr create`
7. CI and e2e must be green before merging

If the user asks to commit and you are on `main`, stop and create a branch first.

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

## Automated Codex review workflow

`.github/workflows/codex-review-open-issues.yml` runs on a weekly schedule (Monday 09:00 UTC) and on `workflow_dispatch`. It sends all Rust source files and Askama templates to GPT-4o for a security and correctness review, then opens GitHub issues for any `medium`, `high`, or `critical` findings.

### Required secret

`OPENROUTER_API_KEY` must be set in the repository secrets. The workflow exits cleanly if the API call fails (non-200), so a missing or invalid key does not break CI — it only produces a failed step in the workflow run.

### Cost gating

The request payload includes `max_cost` (USD), which OpenRouter enforces server-side — if the estimated cost exceeds the cap the request is rejected with a 400 before any tokens are consumed. The default cap is **$0.50 per run**. Both the model and the cap can be overridden at dispatch time via `workflow_dispatch` inputs:

| Input | Default | Description |
|-------|---------|-------------|
| `model` | `openai/gpt-4o` | Any OpenRouter model ID |
| `max_cost_usd` | `0.50` | Hard spend cap in USD |

After a successful run the step logs the actual model, token counts, and reported cost from the OpenRouter `usage` response field.

### JSON output contract

The model is instructed to return a JSON object with a single key `findings`. Each element must conform to:

```json
{
  "title":       "Short one-line description (≤72 chars)",
  "severity":    "critical | high | medium | low | info",
  "file":        "src/handlers/users.rs",
  "line":        42,
  "description": "Detailed explanation (markdown OK)",
  "suggestion":  "Concrete fix or next step",
  "ai_fixable":  true
}
```

- `severity` is constrained to exactly these five values. The workflow only opens issues for `medium` and above.
- `ai_fixable: true` adds the `ai-fixable` label so that automated agents can self-select fixable work.
- `line: 0` and `file: ""` are valid for cross-cutting or non-file-specific findings.

### Labels created by this workflow

| Label | Meaning |
|-------|---------|
| `codex` | All issues opened by the workflow |
| `triage` | Needs human review before acting |
| `critical` / `high` / `medium` | Severity from model output |
| `ai-fixable` | Model assessed this as autonomously fixable |

Create these labels once with:

```bash
gh label create codex      --color "0075ca" --description "Opened by Codex review workflow"
gh label create triage     --color "e4e669" --description "Needs human review"
gh label create critical   --color "b60205" --description "Critical severity finding"
gh label create high       --color "d93f0b" --description "High severity finding"
gh label create medium     --color "fbca04" --description "Medium severity finding"
gh label create ai-fixable --color "0e8a16" --description "Can be fixed autonomously by an AI agent"
```

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
- [x] Explicit `LifecycleState` model
- [x] Unified disable/offboard workflow

### Phase 2 — Structurally sound (done)
- [x] Extract explicit workflow modules (`invite_user`, `disable_user`, `delete_user`)
- [x] Better error handling across multi-step operations (`WorkflowOutcome` for partial failures)
- [x] Group membership reconciliation (Keycloak groups → Matrix room membership via `reconcile_membership`)
- [x] Dry-run / preview support — HTMX inline preview panel on user detail page (`preview_membership` + `POST /users/{id}/reconcile/preview`)

### Phase 3 — Extensible (done)
- [x] Provider interface for pluggable identity backends (provider-agnostic `IdentityProvider` trait)
- [x] Dynamic policy engine — DB-backed policy bindings with `/policy` admin UI, replacing static `GROUP_MAPPINGS` config
- [x] Per-binding `allow_remove` and power level support in reconciliation
- [x] New connector methods: `list_rooms`, `get_room_details`, `set_power_level` (Synapse); `list_groups`, `list_realm_roles` (Keycloak)
- [x] Bootstrap from legacy `GROUP_MAPPINGS` env var on first run
- [ ] Swappable notification backends (email, Matrix message)
- [ ] Support for more deployment patterns

### Phase 4 — Polished (in progress)
- [x] Pico CSS adoption and UX polish (collapsible sections, ARIA, responsive tables, consistent dialogs)
- [x] Dashboard activity stats and system counts
- [x] Tag-triggered release pipeline (CI)
- [ ] Bulk actions (invite many, disable many)
- [ ] Onboarding templates

---

## Environment and build requirements

- Must run inside Flox: `flox activate -- cargo run`
- `libiconv` is provided by Flox — do not add `.cargo/config.toml` linker overrides
- SQLite DB path must exist; `data/` directory is created with `create_if_missing(true)`
- Keycloak realm name is lowercase and case-sensitive

See `CLAUDE.md` for full environment variable reference and external service details.
