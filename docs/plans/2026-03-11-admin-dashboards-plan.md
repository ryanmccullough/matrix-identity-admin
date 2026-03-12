# Admin Dashboards Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Enhance the dashboard with activity metric stat cards (invites, lifecycle, failures across 24h/7d/30d) and system health counts (groups, roles, rooms) in the HTMX status fragment.

**Architecture:** Extend the existing `GET /` handler with new audit DB queries, and extend the `GET /status` HTMX fragment with upstream counts. No new routes, no new DB tables, no background workers.

**Tech Stack:** Rust, Axum, Askama templates, SQLite (sqlx), HTMX

---

### Task 1: Add audit DB queries for dashboard stats

**Files:**
- Modify: `src/db/audit.rs`
- Modify: `src/services/audit_service.rs`

**Step 1: Write failing tests for `count_actions_since`**

Add to the `#[cfg(test)] mod tests` block in `src/db/audit.rs`:

```rust
#[tokio::test]
async fn count_actions_since_filters_by_action_and_time() {
    let pool = setup_db().await;

    // Insert via raw SQL so we control timestamps relative to now
    sqlx::query(
        r#"
        INSERT INTO audit_logs
            (id, timestamp, admin_subject, admin_username,
             target_keycloak_user_id, target_matrix_user_id,
             action, result, metadata_json)
        VALUES
            ('1', datetime('now', '-30 seconds'), 'sub', 'admin', NULL, NULL, 'invite_user', 'success', '{}'),
            ('2', datetime('now', '-30 seconds'), 'sub', 'admin', NULL, NULL, 'disable_identity_account_on_disable', 'success', '{}'),
            ('3', datetime('now', '-2 hours'), 'sub', 'admin', NULL, NULL, 'invite_user', 'success', '{}')
        "#,
    )
    .execute(&pool)
    .await
    .unwrap();

    // Only the recent invite should match
    assert_eq!(
        count_actions_since(&pool, &["invite_user"], 60).await.unwrap(),
        1
    );
    // Both recent entries match when we include both actions
    assert_eq!(
        count_actions_since(&pool, &["invite_user", "disable_identity_account_on_disable"], 60).await.unwrap(),
        2
    );
    // All invites match with a larger window
    assert_eq!(
        count_actions_since(&pool, &["invite_user"], 60 * 60 * 3).await.unwrap(),
        2
    );
}

#[tokio::test]
async fn count_actions_since_returns_zero_on_empty_db() {
    let pool = setup_db().await;
    assert_eq!(
        count_actions_since(&pool, &["invite_user"], 86400).await.unwrap(),
        0
    );
}

#[tokio::test]
async fn count_failures_since_counts_only_failures() {
    let pool = setup_db().await;

    sqlx::query(
        r#"
        INSERT INTO audit_logs
            (id, timestamp, admin_subject, admin_username,
             target_keycloak_user_id, target_matrix_user_id,
             action, result, metadata_json)
        VALUES
            ('1', datetime('now', '-30 seconds'), 'sub', 'admin', NULL, NULL, 'invite_user', 'success', '{}'),
            ('2', datetime('now', '-30 seconds'), 'sub', 'admin', NULL, NULL, 'invite_user', 'failure', '{}'),
            ('3', datetime('now', '-2 hours'), 'sub', 'admin', NULL, NULL, 'invite_user', 'failure', '{}')
        "#,
    )
    .execute(&pool)
    .await
    .unwrap();

    assert_eq!(count_failures_since(&pool, 60).await.unwrap(), 1);
    assert_eq!(count_failures_since(&pool, 60 * 60 * 3).await.unwrap(), 2);
}
```

**Step 2: Run tests to verify they fail**

Run: `flox activate -- cargo test db::audit::tests::count_actions_since -- --nocapture`
Expected: FAIL — `count_actions_since` and `count_failures_since` not found.

**Step 3: Implement `count_actions_since` and `count_failures_since` in `src/db/audit.rs`**

Add after the existing `recent_actions_count` function:

```rust
/// Count audit entries matching any of the given actions within the last `since_seconds` seconds.
pub async fn count_actions_since(
    pool: &SqlitePool,
    actions: &[&str],
    since_seconds: i64,
) -> Result<i64, AppError> {
    if actions.is_empty() {
        return Ok(0);
    }
    let placeholders: Vec<&str> = actions.iter().map(|_| "?").collect();
    let sql = format!(
        "SELECT COUNT(*) FROM audit_logs WHERE action IN ({}) AND unixepoch(timestamp) > unixepoch('now') - ?",
        placeholders.join(", ")
    );
    let mut query = sqlx::query_as::<_, (i64,)>(&sql);
    for action in actions {
        query = query.bind(*action);
    }
    query = query.bind(since_seconds);
    let row = query.fetch_one(pool).await?;
    Ok(row.0)
}

/// Count audit entries with result = 'failure' within the last `since_seconds` seconds.
pub async fn count_failures_since(pool: &SqlitePool, since_seconds: i64) -> Result<i64, AppError> {
    let row: (i64,) = sqlx::query_as(
        r#"
        SELECT COUNT(*)
        FROM audit_logs
        WHERE result = 'failure' AND unixepoch(timestamp) > unixepoch('now') - ?
        "#,
    )
    .bind(since_seconds)
    .fetch_one(pool)
    .await?;
    Ok(row.0)
}
```

**Step 4: Add passthrough methods to `AuditService` in `src/services/audit_service.rs`**

```rust
/// Count audit entries matching any of the given actions within the last `since_seconds` seconds.
pub async fn count_actions_since(
    &self,
    actions: &[&str],
    since_seconds: i64,
) -> Result<i64, AppError> {
    db::audit::count_actions_since(&self.pool, actions, since_seconds).await
}

/// Count audit entries with result = 'failure' within the last `since_seconds` seconds.
pub async fn count_failures_since(&self, since_seconds: i64) -> Result<i64, AppError> {
    db::audit::count_failures_since(&self.pool, since_seconds).await
}
```

**Step 5: Run tests to verify they pass**

Run: `flox activate -- cargo test db::audit::tests::count_actions_since -- --nocapture`
Run: `flox activate -- cargo test db::audit::tests::count_failures_since -- --nocapture`
Expected: PASS

**Step 6: Run full pre-commit gate**

```bash
flox activate -- cargo fmt
flox activate -- cargo clippy --all-targets -- -D warnings
flox activate -- cargo test
```

**Step 7: Commit**

```bash
git add src/db/audit.rs src/services/audit_service.rs
git commit -m "feat(audit): add count_actions_since and count_failures_since queries"
```

---

### Task 2: Extend dashboard handler with activity stats

**Files:**
- Modify: `src/handlers/dashboard.rs`

**Step 1: Update `DashboardTemplate` struct and `dashboard` handler**

Add new fields to `DashboardTemplate`:

```rust
struct DashboardTemplate {
    username: String,
    csrf_token: String,
    total_users: u32,
    actions_24h: i64,
    // New activity stat fields
    invites_24h: i64,
    invites_7d: i64,
    invites_30d: i64,
    lifecycle_24h: i64,
    lifecycle_7d: i64,
    lifecycle_30d: i64,
    failures_24h: i64,
    failures_7d: i64,
    failures_30d: i64,
    recent_actions: Vec<RecentAction>,
    notice: Option<String>,
    error: Option<String>,
    synapse_enabled: bool,
    templates: Vec<crate::models::onboarding_template::OnboardingTemplate>,
}
```

Add constants for the lifecycle actions and time windows at the top of the file:

```rust
const SECS_24H: i64 = 86400;
const SECS_7D: i64 = 86400 * 7;
const SECS_30D: i64 = 86400 * 30;

const INVITE_ACTIONS: &[&str] = &["invite_user"];

const LIFECYCLE_ACTIONS: &[&str] = &[
    "disable_identity_account_on_disable",
    "disable_identity_account_on_offboard",
    "reactivate_auth_account_on_reactivate",
    "delete_keycloak_user",
];
```

Update the `dashboard` handler to fetch the new stats. Add these queries to the existing `tokio::join!`:

```rust
let (
    total_users_res,
    recent_logs_res,
    actions_24h_res,
    inv_24h, inv_7d, inv_30d,
    lc_24h, lc_7d, lc_30d,
    fail_24h, fail_7d, fail_30d,
) = tokio::join!(
    state.keycloak.count_users(""),
    state.audit.recent(5),
    state.audit.recent_actions_count(SECS_24H),
    state.audit.count_actions_since(INVITE_ACTIONS, SECS_24H),
    state.audit.count_actions_since(INVITE_ACTIONS, SECS_7D),
    state.audit.count_actions_since(INVITE_ACTIONS, SECS_30D),
    state.audit.count_actions_since(LIFECYCLE_ACTIONS, SECS_24H),
    state.audit.count_actions_since(LIFECYCLE_ACTIONS, SECS_7D),
    state.audit.count_actions_since(LIFECYCLE_ACTIONS, SECS_30D),
    state.audit.count_failures_since(SECS_24H),
    state.audit.count_failures_since(SECS_7D),
    state.audit.count_failures_since(SECS_30D),
);
```

Unwrap the new results (default to 0 on error for stats — they're non-critical):

```rust
let invites_24h = inv_24h.unwrap_or(0);
let invites_7d = inv_7d.unwrap_or(0);
let invites_30d = inv_30d.unwrap_or(0);
let lifecycle_24h = lc_24h.unwrap_or(0);
let lifecycle_7d = lc_7d.unwrap_or(0);
let lifecycle_30d = lc_30d.unwrap_or(0);
let failures_24h = fail_24h.unwrap_or(0);
let failures_7d = fail_7d.unwrap_or(0);
let failures_30d = fail_30d.unwrap_or(0);
```

Pass them into the template struct.

**Step 2: Run pre-commit gate to verify compilation**

```bash
flox activate -- cargo fmt
flox activate -- cargo clippy --all-targets -- -D warnings
flox activate -- cargo test
```

Expected: Compiles and tests pass (template not yet using new fields, but Askama won't error on unused fields).

**Step 3: Commit**

```bash
git add src/handlers/dashboard.rs
git commit -m "feat(dashboard): fetch activity stats in handler"
```

---

### Task 3: Update dashboard template with new stat cards

**Files:**
- Modify: `templates/dashboard.html`

**Step 1: Replace the stats-grid section**

Replace the existing `<div class="stats-grid">` block with:

```html
<div class="stats-grid">
  <div class="stat-card">
    <div class="stat-value">{{ total_users }}</div>
    <div class="stat-label">Total Users</div>
  </div>
  <div class="stat-card">
    <div class="stat-value">{{ invites_24h }} / {{ invites_7d }} / {{ invites_30d }}</div>
    <div class="stat-label">Invites (24h / 7d / 30d)</div>
  </div>
  <div class="stat-card">
    <div class="stat-value">{{ lifecycle_24h }} / {{ lifecycle_7d }} / {{ lifecycle_30d }}</div>
    <div class="stat-label">Lifecycle (24h / 7d / 30d)</div>
  </div>
  <div class="stat-card">
    <div class="stat-value">{{ failures_24h }} / {{ failures_7d }} / {{ failures_30d }}</div>
    <div class="stat-label">Failures (24h / 7d / 30d)</div>
  </div>
</div>
```

**Step 2: Run pre-commit gate**

```bash
flox activate -- cargo fmt
flox activate -- cargo clippy --all-targets -- -D warnings
flox activate -- cargo test
```

**Step 3: Commit**

```bash
git add templates/dashboard.html
git commit -m "feat(dashboard): add activity stat cards to template"
```

---

### Task 4: Extend status fragment with system counts

**Files:**
- Modify: `src/handlers/dashboard.rs`
- Modify: `templates/status_card.html`

**Step 1: Update `StatusCardTemplate` struct**

Add new fields:

```rust
struct StatusCardTemplate {
    keycloak_ok: bool,
    mas_ok: bool,
    synapse_configured: bool,
    user_count: Option<u32>,
    group_count: Option<usize>,
    role_count: Option<usize>,
    room_count: Option<u64>,
}
```

**Step 2: Update the `status` handler**

Replace the current handler body to add concurrent calls for groups, roles, and rooms:

```rust
pub async fn status(
    AuthenticatedAdmin(_admin): AuthenticatedAdmin,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, AppError> {
    let synapse_configured = state.synapse.is_some();

    let (kc_result, mas_result, groups_result, roles_result) = tokio::join!(
        state.keycloak.count_users(""),
        state.mas.get_user_by_username("__status_check__"),
        state.keycloak.list_groups(),
        state.keycloak.list_realm_roles(),
    );

    let (keycloak_ok, user_count) = match kc_result {
        Ok(n) => (true, Some(n)),
        Err(_) => (false, None),
    };
    let mas_ok = mas_result.is_ok();
    let group_count = groups_result.ok().map(|g| g.len());
    let role_count = roles_result.ok().map(|r| r.len());

    // Synapse room count — only attempt if configured
    let room_count = if let Some(ref synapse) = state.synapse {
        synapse.list_rooms(1, None).await.ok().map(|r| r.total_rooms)
    } else {
        None
    };

    let tmpl = StatusCardTemplate {
        keycloak_ok,
        mas_ok,
        synapse_configured,
        user_count,
        group_count,
        role_count,
        room_count,
    };
    let html = tmpl
        .render()
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Template error: {e}")))?;
    Ok(([(header::CONTENT_TYPE, "text/html; charset=utf-8")], html))
}
```

NOTE: Check the return type of `synapse.list_rooms()` — it returns a struct with a `total_rooms` field. Read `src/clients/synapse.rs` to confirm the exact type and field name before implementing.

**Step 3: Update `templates/status_card.html`**

Add the new counts to the status grid:

```html
<div class="status-grid">
  <div class="status-item {% if keycloak_ok %}status-ok{% else %}status-err{% endif %}">
    <span class="status-label">Keycloak</span>
    <span class="status-value">{% if keycloak_ok %}&#9679;  reachable{% else %}&#9679;  unreachable{% endif %}</span>
    {% if let Some(n) = user_count %}<span class="status-sub">{{ n }} users</span>{% endif %}
  </div>
  <div class="status-item {% if mas_ok %}status-ok{% else %}status-err{% endif %}">
    <span class="status-label">MAS</span>
    <span class="status-value">{% if mas_ok %}&#9679;  reachable{% else %}&#9679;  unreachable{% endif %}</span>
  </div>
  <div class="status-item {% if synapse_configured %}status-ok{% else %}status-warn{% endif %}">
    <span class="status-label">Synapse</span>
    <span class="status-value">{% if synapse_configured %}&#9679;  configured{% else %}&#9679;  not configured{% endif %}</span>
    {% if let Some(n) = room_count %}<span class="status-sub">{{ n }} rooms</span>{% endif %}
  </div>
</div>
<div class="status-grid" style="margin-top:0.5rem">
  <div class="status-item status-ok">
    <span class="status-label">Groups</span>
    <span class="status-value">{% match group_count %}{% when Some with (n) %}{{ n }}{% when None %}&mdash;{% endmatch %}</span>
  </div>
  <div class="status-item status-ok">
    <span class="status-label">Roles</span>
    <span class="status-value">{% match role_count %}{% when Some with (n) %}{{ n }}{% when None %}&mdash;{% endmatch %}</span>
  </div>
</div>
```

**Step 4: Run pre-commit gate**

```bash
flox activate -- cargo fmt
flox activate -- cargo clippy --all-targets -- -D warnings
flox activate -- cargo test
```

**Step 5: Commit**

```bash
git add src/handlers/dashboard.rs templates/status_card.html
git commit -m "feat(dashboard): add system counts to status fragment"
```

---

### Task 5: Add dashboard handler tests for new stats

**Files:**
- Modify: `src/handlers/dashboard.rs` (test module)

**Step 1: Add test for activity stats appearing in dashboard**

Add to the existing `mod tests` block:

```rust
#[tokio::test]
async fn dashboard_shows_invite_stats() {
    let state = build_test_state(MockKeycloak::default(), "secret", None).await;
    state
        .audit
        .log(
            "sub",
            "admin",
            None,
            None,
            "invite_user",
            AuditResult::Success,
            serde_json::json!({}),
        )
        .await
        .unwrap();

    let resp = get_dashboard(state, Some(make_auth_cookie(TEST_CSRF)), "").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_text(resp).await;
    // At least one invite should show in the 24h/7d/30d stat card
    assert!(
        body.contains("Invites"),
        "expected 'Invites' label in dashboard body"
    );
}

#[tokio::test]
async fn dashboard_shows_failure_stats() {
    let state = build_test_state(MockKeycloak::default(), "secret", None).await;
    state
        .audit
        .log(
            "sub",
            "admin",
            None,
            None,
            "invite_user",
            AuditResult::Failure,
            serde_json::json!({}),
        )
        .await
        .unwrap();

    let resp = get_dashboard(state, Some(make_auth_cookie(TEST_CSRF)), "").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_text(resp).await;
    assert!(
        body.contains("Failures"),
        "expected 'Failures' label in dashboard body"
    );
}
```

**Step 2: Add test for status fragment showing group/role counts**

```rust
#[tokio::test]
async fn status_shows_group_and_role_counts() {
    use crate::models::keycloak::{KeycloakGroup, KeycloakRole};
    let state = build_test_state(
        MockKeycloak {
            all_groups: vec![
                KeycloakGroup { id: "g1".into(), name: "staff".into() },
                KeycloakGroup { id: "g2".into(), name: "contractors".into() },
            ],
            all_roles: vec![
                KeycloakRole { id: Some("r1".into()), name: "admin".into() },
            ],
            ..Default::default()
        },
        "secret",
        None,
    )
    .await;
    let cookie = make_auth_cookie(TEST_CSRF);
    let resp = status_router(state)
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/status")
                .header("cookie", cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let html = String::from_utf8(body.to_vec()).unwrap();
    assert!(html.contains("2"), "expected group count '2' in status");
    assert!(html.contains("Groups"), "expected 'Groups' label in status");
    assert!(html.contains("Roles"), "expected 'Roles' label in status");
}
```

**Step 3: Run full pre-commit gate**

```bash
flox activate -- cargo fmt
flox activate -- cargo clippy --all-targets -- -D warnings
flox activate -- cargo test
```

**Step 4: Commit**

```bash
git add src/handlers/dashboard.rs
git commit -m "test(dashboard): add tests for activity stats and system counts"
```

---

### Task 6: Remove old `actions_24h` stat card

**Files:**
- Modify: `src/handlers/dashboard.rs`
- Modify: `templates/dashboard.html`

The old `actions_24h` field is now redundant — the new invite/lifecycle/failure cards give a more detailed breakdown. Remove it from:

1. `DashboardTemplate` struct — remove the `actions_24h` field
2. `dashboard` handler — remove the `recent_actions_count` call from `tokio::join!` and the field assignment
3. `templates/dashboard.html` — already replaced in Task 3 (verify it's gone)

**Step 1: Remove `actions_24h` from handler**

Remove the `actions_24h` field from `DashboardTemplate`, remove `state.audit.recent_actions_count(SECS_24H)` from the `tokio::join!`, and remove the `actions_24h` assignment.

**Step 2: Run pre-commit gate**

```bash
flox activate -- cargo fmt
flox activate -- cargo clippy --all-targets -- -D warnings
flox activate -- cargo test
```

**Step 3: Commit**

```bash
git add src/handlers/dashboard.rs templates/dashboard.html
git commit -m "refactor(dashboard): remove redundant actions_24h stat"
```
