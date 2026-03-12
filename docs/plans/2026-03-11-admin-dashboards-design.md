# Admin Dashboards Design

## Goal

Enhance the existing dashboard page with activity metrics and system health counts, giving admins at-a-glance operational visibility without leaving the landing page.

## Architecture

Extend the existing `GET /` handler and HTMX `/status` fragment. No new routes, no background workers, no new DB tables. All data computed on page load.

- Audit-derived stats load synchronously (fast indexed SQLite queries)
- System counts load async via the existing HTMX status fragment (avoids blocking initial render on upstream calls)

## Activity Metrics (Stat Cards)

Derived from audit log queries across fixed time windows (24h / 7d / 30d).

| Card | Filter | Source |
|------|--------|--------|
| Total Users | — | `count_users("")` (already exists) |
| Invites | `action = 'invite_user'` | Audit DB |
| Lifecycle Actions | `action IN (disable, offboard, reactivate, delete)` | Audit DB |
| Failures | `result = 'failure'` | Audit DB |

Each multi-window card shows three numbers inline (e.g. "3 / 12 / 45") with "24h / 7d / 30d" labels beneath.

### New audit query

Add `count_by_action_since(actions: &[&str], since_seconds: i64) -> Result<i64>` to `AuditService`. Single SQL query with `WHERE action IN (...) AND timestamp > datetime(...)`. Nine calls total (3 cards x 3 windows), all fast on indexed columns.

For the failures card, add `count_failures_since(since_seconds: i64) -> Result<i64>` — `WHERE result = 'failure' AND timestamp > datetime(...)`.

## System Health Counts (Status Fragment)

Added to the existing HTMX `/status` fragment alongside current health indicators.

| Stat | Source | Call |
|------|--------|------|
| Keycloak status | Keycloak | `count_users("")` (exists) |
| MAS status | MAS | sentinel lookup (exists) |
| Keycloak groups | Keycloak | `list_groups().len()` |
| Keycloak roles | Keycloak | `list_realm_roles().len()` |
| Synapse rooms | Synapse | `list_rooms(1, None)` total_rooms field |

All calls run concurrently via `tokio::join!`. Individual failures render as "—" with `.status-err` styling, same as existing pattern.

## Error Handling

- Audit queries: use existing `AppError::Database` path
- Upstream count failures in status fragment: render "—" with error indicator (existing pattern)
- No new error types needed

## Testing

- `count_by_action_since` / `count_failures_since` — unit tests with seeded audit entries
- Dashboard handler — existing test pattern (authenticated 200, unauthenticated redirect)
- Status handler — verify new counts appear in response body
- No new mock traits needed — `MockKeycloak` already implements `list_groups`/`list_realm_roles`

## Out of Scope

- Selectable time ranges (audit page already has this)
- Charts or graphs
- Background stat caching
- MAS session counts
