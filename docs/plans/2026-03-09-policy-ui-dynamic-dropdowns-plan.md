# Policy UI Dynamic Dropdowns Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Replace static datalist inputs on the `/policy` page with HTMX-powered `<select>` dropdowns that dynamically load groups, roles, and rooms, and add named power level tiers.

**Architecture:** Three new HTML-fragment endpoints return `<option>` elements that HTMX swaps into `<select>` elements. The subject type toggle switches which endpoint populates the subject value dropdown. Power level changes from a number input to a named-tier select.

**Tech Stack:** Rust, axum, askama, HTMX (`hx-get`, `hx-target`, `hx-trigger`, `hx-swap`)

---

### Task 1: Add fragment handler functions

Add three HTML-fragment endpoints to `src/handlers/policy.rs` and wire routes in `src/lib.rs`.

**Files:**
- Modify: `src/handlers/policy.rs`
- Modify: `src/lib.rs`

**Step 1: Add the three fragment handlers to `src/handlers/policy.rs`**

Add these after the existing `refresh_rooms` handler (before the `#[cfg(test)]` block):

```rust
/// GET /policy/api/groups — HTML fragment of `<option>` elements for Keycloak groups.
pub async fn api_groups(
    AuthenticatedAdmin(_admin): AuthenticatedAdmin,
    State(state): State<AppState>,
) -> Result<Html<String>, AppError> {
    match state.keycloak.list_groups().await {
        Ok(groups) => {
            let mut html = String::from(r#"<option value="">Select a group…</option>"#);
            for g in groups {
                html.push_str(&format!(
                    r#"<option value="{name}">{name}</option>"#,
                    name = g.name
                ));
            }
            Ok(Html(html))
        }
        Err(_) => Ok(Html(
            r#"<option value="" disabled>Failed to load groups — try again</option>"#.to_string(),
        )),
    }
}

/// GET /policy/api/roles — HTML fragment of `<option>` elements for Keycloak realm roles.
pub async fn api_roles(
    AuthenticatedAdmin(_admin): AuthenticatedAdmin,
    State(state): State<AppState>,
) -> Result<Html<String>, AppError> {
    match state.keycloak.list_realm_roles().await {
        Ok(roles) => {
            let mut html = String::from(r#"<option value="">Select a role…</option>"#);
            for r in roles {
                html.push_str(&format!(
                    r#"<option value="{name}">{name}</option>"#,
                    name = r.name
                ));
            }
            Ok(Html(html))
        }
        Err(_) => Ok(Html(
            r#"<option value="" disabled>Failed to load roles — try again</option>"#.to_string(),
        )),
    }
}

/// GET /policy/api/rooms — HTML fragment of `<option>` elements for cached rooms/spaces.
pub async fn api_rooms(
    AuthenticatedAdmin(_admin): AuthenticatedAdmin,
    State(state): State<AppState>,
) -> Result<Html<String>, AppError> {
    let rooms = state.policy_service.list_cached_rooms().await?;
    if rooms.is_empty() {
        return Ok(Html(
            r#"<option value="" disabled>No rooms cached — click Refresh Rooms</option>"#
                .to_string(),
        ));
    }
    let mut html = String::from(r#"<option value="">Select a room…</option>"#);
    for r in rooms {
        let prefix = if r.is_space { "[Space]" } else { "[Room]" };
        let label = match (&r.name, &r.canonical_alias) {
            (Some(name), Some(alias)) => format!("{prefix} {name} ({alias})"),
            (Some(name), None) => format!("{prefix} {name}"),
            (None, Some(alias)) => format!("{prefix} {alias}"),
            (None, None) => format!("{prefix} {}", r.room_id),
        };
        html.push_str(&format!(
            r#"<option value="{room_id}">{label}</option>"#,
            room_id = r.room_id,
            label = label,
        ));
    }
    Ok(Html(html))
}
```

**Step 2: Add routes to `src/lib.rs`**

Find the existing policy routes block and add 3 new GET routes after the POST routes:

```rust
        .route("/policy/api/groups", get(handlers::policy::api_groups))
        .route("/policy/api/roles", get(handlers::policy::api_roles))
        .route("/policy/api/rooms", get(handlers::policy::api_rooms))
```

**Step 3: Remove server-side group/role loading from the `list` handler**

In `src/handlers/policy.rs`, the `list` handler currently fetches groups and roles from Keycloak on every page load. Since the dropdowns now load via HTMX, remove these fetches and the `groups`/`roles` fields from `PolicyTemplate`.

In the `list` handler, remove:
```rust
    // Fetch groups and roles from Keycloak for the dropdowns.
    let groups = state
        .keycloak
        .list_groups()
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|g| g.name)
        .collect();
    let roles = state
        .keycloak
        .list_realm_roles()
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|r| r.name)
        .collect();
```

And remove `groups` and `roles` from the `PolicyTemplate` struct and its construction.

Also remove `rooms` from the template struct — rooms are now loaded via HTMX too. Keep `rooms` only for the room cache count display. Actually, keep `room_count: usize` instead of the full `Vec<CachedRoom>`.

Updated `PolicyTemplate`:
```rust
#[derive(Template)]
#[template(path = "policy.html")]
struct PolicyTemplate {
    username: String,
    csrf_token: String,
    bindings: Vec<PolicyBinding>,
    room_count: usize,
    synapse_enabled: bool,
    notice: String,
    warning: String,
}
```

Updated `list` handler body (after getting bindings):
```rust
    let room_count = state.policy_service.list_cached_rooms().await?.len();
```

Remove the `CachedRoom` import if no longer used in the template struct.

**Step 4: Run tests**

```bash
flox activate -- cargo fmt
flox activate -- cargo clippy --all-targets -- -D warnings
flox activate -- cargo test
```

**Step 5: Commit**

```bash
git add src/handlers/policy.rs src/lib.rs
git commit -m "feat(policy): add HTMX fragment endpoints for groups, roles, rooms"
```

---

### Task 2: Update the template

Replace static datalists and number inputs with HTMX-powered selects and power level dropdown.

**Files:**
- Modify: `templates/policy.html`

**Step 1: Replace the Add Binding form**

Replace the entire `<div class="card"><h2>Add Binding</h2>...</div>` section (lines 75–132) with:

```html
<div class="card">
  <h2>Add Binding</h2>
  <form method="post" action="/policy/bindings">
    <input type="hidden" name="_csrf" value="{{ csrf_token }}">
    <div class="form-row">
      <div class="form-group">
        <label for="subject_type">Subject Type</label>
        <select name="subject_type" id="subject_type"
                hx-get="/policy/api/groups"
                hx-trigger="load, change"
                hx-target="#subject_value"
                hx-swap="innerHTML">
          <option value="group">Group</option>
          <option value="role">Role</option>
        </select>
      </div>
      <div class="form-group">
        <label for="subject_value">Subject</label>
        <select name="subject_value" id="subject_value" required>
          <option value="">Loading…</option>
        </select>
      </div>
    </div>
    <div class="form-row">
      <div class="form-group">
        <label for="target_type">Target Type</label>
        <select name="target_type" id="target_type">
          <option value="room">Room</option>
          <option value="space">Space</option>
        </select>
      </div>
      <div class="form-group">
        <label for="target_room_id">Room / Space</label>
        <select name="target_room_id" id="target_room_id" required
                hx-get="/policy/api/rooms"
                hx-trigger="load"
                hx-swap="innerHTML">
          <option value="">Loading…</option>
        </select>
      </div>
    </div>
    <div class="form-row">
      <div class="form-group">
        <label for="power_level">Power Level</label>
        <select name="power_level" id="power_level">
          <option value="">(None)</option>
          <option value="0">User (0)</option>
          <option value="50">Moderator (50)</option>
          <option value="100">Admin (100)</option>
        </select>
      </div>
      <div class="form-group">
        <label>
          <input type="checkbox" name="allow_remove" value="1">
          Allow remove (kick when user leaves group/role)
        </label>
      </div>
    </div>
    <button type="submit" class="btn btn-primary">Add Binding</button>
  </form>
</div>
```

The key HTMX behavior: `subject_type` has `hx-trigger="load, change"`. On page load it fetches `/policy/api/groups` (since "Group" is selected). When the user switches to "Role", the `change` event fires — but we need to change the URL. Add a small inline script or use `hx-vals` to make the URL dynamic.

Actually, the simplest HTMX approach: use `hx-get` with a computed URL. HTMX doesn't natively support dynamic `hx-get` based on a sibling's value. The cleanest solution is a tiny `<script>` block:

```html
<script>
document.getElementById('subject_type').addEventListener('change', function() {
    var url = this.value === 'role' ? '/policy/api/roles' : '/policy/api/groups';
    this.setAttribute('hx-get', url);
    htmx.trigger(this, 'change');
});
</script>
```

Wait — that creates a loop. Better approach: remove `hx-trigger="change"` from the select and handle it purely in JS:

```html
<select name="subject_type" id="subject_type"
        hx-get="/policy/api/groups"
        hx-trigger="load"
        hx-target="#subject_value"
        hx-swap="innerHTML">
```

Then add this script at the bottom of the `{% block content %}`:

```html
<script>
document.getElementById('subject_type').addEventListener('change', function() {
    var url = this.value === 'role' ? '/policy/api/roles' : '/policy/api/groups';
    htmx.ajax('GET', url, {target: '#subject_value', swap: 'innerHTML'});
});
</script>
```

This is clean — HTMX loads groups on page load, and the JS change handler fetches the right endpoint when toggled.

**Step 2: Update the per-row power level in the bindings table**

Replace the `<input type="number">` in the inline update form (line 56–58) with a power level select:

```html
<select name="power_level" style="width:auto">
  <option value="" {% match b.power_level %}{% when None %}selected{% when Some with (_pl) %}{% endmatch %}>(None)</option>
  <option value="0" {% match b.power_level %}{% when Some with (pl) %}{% if *pl == 0 %}selected{% endif %}{% when None %}{% endmatch %}>User (0)</option>
  <option value="50" {% match b.power_level %}{% when Some with (pl) %}{% if *pl == 50 %}selected{% endif %}{% when None %}{% endmatch %}>Mod (50)</option>
  <option value="100" {% match b.power_level %}{% when Some with (pl) %}{% if *pl == 100 %}selected{% endif %}{% when None %}{% endmatch %}>Admin (100)</option>
</select>
```

**Step 3: Update room cache count display**

Change line 137 from `{{ rooms.len() }}` to `{{ room_count }}`:

```html
<p class="muted">{{ room_count }} rooms cached from Synapse.</p>
```

**Step 4: Run tests**

```bash
flox activate -- cargo fmt
flox activate -- cargo clippy --all-targets -- -D warnings
flox activate -- cargo test
```

**Step 5: Commit**

```bash
git add templates/policy.html
git commit -m "feat(policy): replace datalists with HTMX-powered selects and power level tiers"
```

---

### Task 3: Add tests and update test helpers

Add tests for the 3 new fragment endpoints and update the policy router in test helpers.

**Files:**
- Modify: `src/handlers/policy.rs` (test module)
- Modify: `src/test_helpers.rs`

**Step 1: Add routes to `policy_router` in `src/test_helpers.rs`**

Find the `policy_router` function and add the 3 GET routes:

```rust
pub fn policy_router(state: AppState) -> Router {
    use axum::routing::get;
    Router::new()
        .route("/policy", get(crate::handlers::policy::list))
        .route("/policy/bindings", post(crate::handlers::policy::create))
        .route(
            "/policy/bindings/{id}/update",
            post(crate::handlers::policy::update),
        )
        .route(
            "/policy/bindings/{id}/delete",
            post(crate::handlers::policy::delete),
        )
        .route(
            "/policy/rooms/refresh",
            post(crate::handlers::policy::refresh_rooms),
        )
        .route("/policy/api/groups", get(crate::handlers::policy::api_groups))
        .route("/policy/api/roles", get(crate::handlers::policy::api_roles))
        .route("/policy/api/rooms", get(crate::handlers::policy::api_rooms))
        .with_state(state)
}
```

**Step 2: Add tests to the test module in `src/handlers/policy.rs`**

Add these tests inside the existing `#[cfg(test)] mod tests` block:

```rust
    // ── GET /policy/api/groups ───────────────────────────────────────────

    #[tokio::test]
    async fn api_groups_unauthenticated_redirects() {
        let state = build_test_state(MockKeycloak::default(), "secret", None).await;
        let resp = get_fragment(state, "/policy/api/groups", None).await;
        assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    }

    #[tokio::test]
    async fn api_groups_authenticated_returns_200() {
        let mut kc = MockKeycloak::default();
        kc.all_groups = vec![
            crate::models::keycloak::KeycloakGroup {
                id: "g1".into(),
                name: "staff".into(),
                path: "/staff".into(),
            },
        ];
        let state = build_test_state(kc, "secret", None).await;
        let resp = get_fragment(state, "/policy/api/groups", Some(make_auth_cookie(TEST_CSRF))).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_text(resp).await;
        assert!(body.contains("staff"), "expected 'staff' in groups fragment");
    }

    // ── GET /policy/api/roles ────────────────────────────────────────────

    #[tokio::test]
    async fn api_roles_unauthenticated_redirects() {
        let state = build_test_state(MockKeycloak::default(), "secret", None).await;
        let resp = get_fragment(state, "/policy/api/roles", None).await;
        assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    }

    #[tokio::test]
    async fn api_roles_authenticated_returns_200() {
        let mut kc = MockKeycloak::default();
        kc.all_roles = vec![
            crate::models::keycloak::KeycloakRole {
                id: "r1".into(),
                name: "matrix-admin".into(),
            },
        ];
        let state = build_test_state(kc, "secret", None).await;
        let resp = get_fragment(state, "/policy/api/roles", Some(make_auth_cookie(TEST_CSRF))).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_text(resp).await;
        assert!(body.contains("matrix-admin"), "expected 'matrix-admin' in roles fragment");
    }

    // ── GET /policy/api/rooms ────────────────────────────────────────────

    #[tokio::test]
    async fn api_rooms_unauthenticated_redirects() {
        let state = build_test_state(MockKeycloak::default(), "secret", None).await;
        let resp = get_fragment(state, "/policy/api/rooms", None).await;
        assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    }

    #[tokio::test]
    async fn api_rooms_empty_cache_returns_disabled_option() {
        let state = build_test_state(MockKeycloak::default(), "secret", None).await;
        let resp = get_fragment(state, "/policy/api/rooms", Some(make_auth_cookie(TEST_CSRF))).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_text(resp).await;
        assert!(body.contains("No rooms cached"), "expected empty-cache message");
    }
```

Also add the `get_fragment` and `body_text` helper functions at the top of the test module (if `body_text` doesn't already exist):

```rust
    async fn get_fragment(
        state: crate::state::AppState,
        uri: &str,
        cookie: Option<String>,
    ) -> axum::response::Response {
        let mut builder = Request::builder().method(Method::GET).uri(uri);
        if let Some(c) = cookie {
            builder = builder.header("cookie", c);
        }
        let req = builder.body(Body::empty()).unwrap();
        policy_router(state).oneshot(req).await.unwrap()
    }

    async fn body_text(resp: axum::response::Response) -> String {
        let bytes = http_body_util::BodyExt::collect(resp.into_body())
            .await
            .unwrap()
            .to_bytes();
        String::from_utf8_lossy(&bytes).into_owned()
    }
```

**Step 3: Run all checks**

```bash
flox activate -- cargo fmt
flox activate -- cargo clippy --all-targets -- -D warnings
flox activate -- cargo test
```

**Step 4: Commit**

```bash
git add src/handlers/policy.rs src/test_helpers.rs
git commit -m "test(policy): add tests for HTMX fragment endpoints"
```

---

### Task 4: Add CSS for form layout

The `form-row` and `form-group` classes are used in the policy template but not defined in CSS. Add minimal styles.

**Files:**
- Modify: `static/app.css`

**Step 1: Add form layout styles**

Add to the end of `static/app.css`:

```css
/* ── Form layout ──────────────────────────────────────────────────────────── */

.form-row {
  display: flex;
  gap: 1rem;
  margin-bottom: 1rem;
}

.form-row .form-group {
  flex: 1;
}

.form-group label {
  display: block;
  margin-bottom: 0.25rem;
  font-weight: 600;
  font-size: 0.85rem;
}

.form-group select,
.form-group input[type="text"],
.form-group input[type="number"] {
  width: 100%;
  padding: 0.4rem 0.5rem;
  border: 1px solid #ccc;
  border-radius: 4px;
  font-size: 0.9rem;
}

/* ── Alerts ───────────────────────────────────────────────────────────────── */

.alert {
  padding: 0.75rem 1rem;
  border-radius: 4px;
  margin-bottom: 1rem;
}

.alert-success {
  background: #d4edda;
  border: 1px solid #c3e6cb;
  color: #155724;
}

.alert-warning {
  background: #fff3cd;
  border: 1px solid #ffeeba;
  color: #856404;
}
```

**Step 2: Commit**

```bash
git add static/app.css
git commit -m "style(policy): add form layout and alert CSS classes"
```

---

## Dependencies Between Tasks

```
Task 1 (handlers + routes) ──→ Task 2 (template) ──→ Task 3 (tests)
                                                          │
Task 4 (CSS) ─────────────────────────────────────────────┘
```

Tasks 1–3 are sequential. Task 4 (CSS) is independent and can run in parallel with any task.
