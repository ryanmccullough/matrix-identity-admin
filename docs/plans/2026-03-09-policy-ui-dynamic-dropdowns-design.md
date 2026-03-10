# Policy UI Dynamic Dropdowns — Design Document

**Date:** 2026-03-09
**Phase:** 4 (Polished)
**Status:** Approved

## Goal

Replace static datalist inputs on the `/policy` page with HTMX-powered `<select>` dropdowns that dynamically load groups, roles, and rooms from upstream services. Add named power level tiers instead of free-form number input.

## Changes

### New endpoints

Three HTML-fragment endpoints, all requiring `AuthenticatedAdmin`:

| Method | Path | Source | Returns |
|--------|------|--------|---------|
| GET | `/policy/api/groups` | `keycloak.list_groups()` | `<option>` elements |
| GET | `/policy/api/roles` | `keycloak.list_realm_roles()` | `<option>` elements |
| GET | `/policy/api/rooms` | `policy_service.list_cached_rooms()` | `<option>` elements with `[Room]`/`[Space]` prefix |

Endpoints return raw HTML fragments (not JSON). HTMX swaps the fragment directly into a `<select>` element.

### Subject selection

- Subject type `<select>` (Group/Role) uses `hx-get` to fetch the appropriate list on change
- `hx-target="#subject_value"` replaces the subject value dropdown's inner `<option>` elements
- `hx-trigger="change"` on the type selector, plus `hx-trigger="load"` for initial population
- Default loads groups (since "Group" is the first option)

### Room/space selection

- `<select>` populated via `hx-get="/policy/api/rooms"` with `hx-trigger="load"`
- Options display: `[Space] Engineering` or `[Room] #general (example.com)` using cached name with fallback to room ID
- Empty cache shows: `<option disabled>No rooms cached — click Refresh Rooms</option>`

### Power level dropdown

Replace `<input type="number">` with a `<select>` in both the "Add Binding" form and per-row inline update:

| Label | Value |
|-------|-------|
| (None) | empty |
| User (0) | 0 |
| Moderator (50) | 50 |
| Admin (100) | 100 |

### Error handling

- If Keycloak is unreachable, group/role endpoints return `<option disabled>Failed to load — try again</option>`
- If room cache is empty, rooms endpoint returns `<option disabled>No rooms cached — click Refresh Rooms</option>`
- No manual-entry fallback — if upstream is down, admin retries later

### Files affected

- Modify: `src/handlers/policy.rs` — add 3 fragment handler functions
- Modify: `src/lib.rs` — add 3 routes
- Modify: `templates/policy.html` — replace datalists/inputs with HTMX-powered selects, power level dropdown
- Modify: `src/test_helpers.rs` — add routes to `policy_router`

### Testing

- One test per fragment endpoint: authenticated returns 200
- One test per fragment endpoint: unauthenticated redirects to login
- Existing handler tests remain unchanged
