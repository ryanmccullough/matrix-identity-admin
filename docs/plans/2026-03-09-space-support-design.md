# Space Support — Design

## Summary

Extend group→room reconciliation to support Matrix spaces. When a `GROUP_MAPPINGS` entry points to a space, the system auto-detects it, discovers child rooms, and reconciles membership across the space and all its children. No config changes required — spaces are rooms under the hood.

## Goals

1. Auto-detect space vs. regular room in `GROUP_MAPPINGS` entries.
2. Expand space mappings to include all direct child rooms.
3. Join/kick users from space + children during reconciliation.
4. Show expanded space children in preview.
5. Expand spaces during offboard kicks.

## Non-goals

- Recursive/nested space traversal (flat children only).
- Managing space hierarchy (creating spaces, adding/removing child rooms).
- New config fields or env vars.
- Space-specific UI beyond what preview already shows.

---

## Data model & config

**No config changes.** Admins put space room IDs in `matrix_room_id` the same way they put regular room IDs. The system auto-detects which entries are spaces at reconciliation time.

**New Synapse client method:**

```rust
async fn get_space_children(&self, space_id: &str) -> Result<Vec<String>>;
```

Reads `m.space.child` state events from the space room via `GET /_synapse/admin/v1/rooms/{room_id}/state`. Returns child room IDs where content is non-empty (empty content = removed child).

**Reconciliation flow per mapping:**

1. Call `get_space_children(room_id)`.
2. If children found → treat as space. Reconcile the space itself + all child rooms.
3. If empty / error → treat as regular room. Reconcile just that room (current behavior).

---

## Reconciliation logic changes

Current per-mapping logic:
```
check membership → join or kick single room
```

New per-mapping logic:
```
1. get_space_children(room_id)
2. Build target list: [room_id] + child_room_ids (if any)
3. For each target in list:
   - check membership → join or kick
```

**Join order:** Space first, then child rooms. Space appears in sidebar before children populate.

**Kick order:** Child rooms first, then space. Remove contents before container.

**Failure handling:** Per-room failures are non-fatal, collected as warnings in `WorkflowOutcome`. A failure to join one child room does not block joining others.

**Preview:** Same expansion logic. Each space mapping shows the space + individual child rooms with join/kick/skip status.

**Offboard:** `kick_from_all_mapped_rooms` in `lifecycle_steps.rs` uses the same expansion — kick from child rooms then space.

---

## Synapse client & API

**New trait method on `MatrixService`:**

```rust
async fn get_space_children(&self, space_id: &str) -> Result<Vec<String>>;
```

**Endpoint:** `GET /_synapse/admin/v1/rooms/{room_id}/state`

Fetches all state events, filters for `type == "m.space.child"` where `state_key` is the child room ID and `content` is non-empty.

**Why admin API:** Consistent with `get_joined_room_members`. Admin user does not need space membership.

**`RoomManagementApi`:** No changes. `force_join_user` and `kick_user_from_room` already work for spaces (spaces are rooms).

**Error handling:** If `get_space_children` fails for a mapping, treat it as a regular room (single-room reconciliation) and log a warning. Defensive — transient Synapse errors should not break non-space mappings.

---

## Test coverage

### `get_space_children`
- Space with children → returns child room IDs
- Regular room (no `m.space.child` events) → returns empty vec
- Removed child (empty content) → excluded from results
- Synapse error → returns error

### Reconciliation (new tests)
- Space mapping: user joined to space + all child rooms
- Space mapping with kicks: user kicked from child rooms then space
- Mixed mappings: one space + one regular room in same config
- Space child discovery failure: falls back to single-room with warning
- Preview expands space children in output

### Lifecycle steps (updated)
- `kick_from_all_mapped_rooms`: verify child rooms kicked before space

### Handler tests
No changes needed — handlers are space-unaware.

### Mock changes
Add `get_space_children` to `MockSynapse` with configurable return values per room ID.
