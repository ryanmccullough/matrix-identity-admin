# Space Support Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Auto-detect Matrix spaces in GROUP_MAPPINGS and expand reconciliation to cover the space plus all its child rooms.

**Architecture:** Add `get_space_children` to the `MatrixService` trait, then modify `reconcile_membership`, `preview_membership`, and `kick_from_all_mapped_rooms` to expand space mappings before processing. No config changes — spaces are detected at reconciliation time by querying room state.

**Tech Stack:** Rust, axum, async_trait, reqwest, serde, sqlx (tests)

**Design doc:** `docs/plans/2026-03-09-space-support-design.md`

---

### Task 1: Add `get_space_children` to `MatrixService` trait

**Files:**
- Modify: `src/clients/synapse.rs:14-38` (trait definition)
- Modify: `src/test_helpers.rs:327-372` (MockSynapse MatrixService impl)
- Modify: `src/services/lifecycle_steps.rs:467-508` (local MockSynapse impl)

This task adds the trait method and stubs all mock implementations so the project compiles. No behavior change yet.

**Step 1: Add trait method to `MatrixService`**

In `src/clients/synapse.rs`, add this method to the `MatrixService` trait (after `kick_user_from_room`):

```rust
    /// Return the room IDs of all direct children of a Matrix space.
    /// If the room is not a space (no `m.space.child` events), returns an empty vec.
    async fn get_space_children(&self, space_id: &str) -> Result<Vec<String>, AppError>;
```

**Step 2: Add `SynapseClient` implementation**

In `src/clients/synapse.rs`, add this method to the `impl MatrixService for SynapseClient` block (after `kick_user_from_room`):

```rust
    async fn get_space_children(&self, space_id: &str) -> Result<Vec<String>, AppError> {
        #[derive(Deserialize)]
        struct StateEvent {
            #[serde(rename = "type")]
            event_type: String,
            state_key: String,
            content: serde_json::Value,
        }

        #[derive(Deserialize)]
        struct StateResponse {
            state: Vec<StateEvent>,
        }

        let token = self.admin_token().await?;
        let encoded = urlencoded(space_id);
        let url = self.url(&format!("/_synapse/admin/v1/rooms/{encoded}/state"));

        let resp: StateResponse = self
            .http
            .get(&url)
            .bearer_auth(&token)
            .send()
            .await
            .map_err(|e| upstream_error("synapse", e))?
            .error_for_status()
            .map_err(|e| upstream_error("synapse", e))?
            .json()
            .await
            .map_err(|e| upstream_error("synapse", e))?;

        let children: Vec<String> = resp
            .state
            .into_iter()
            .filter(|e| {
                e.event_type == "m.space.child"
                    && !e.state_key.is_empty()
                    && e.content != serde_json::Value::Object(serde_json::Map::new())
            })
            .map(|e| e.state_key)
            .collect();

        Ok(children)
    }
```

Note: `serde_json` is already imported (`use serde_json::json;` not present here, but `serde_json::Value` is used inline). Add `use serde_json;` if needed, or use the fully qualified path as shown.

**Step 3: Stub `get_space_children` in `MockSynapse` (test_helpers.rs)**

Add a new field to `MockSynapse` in `src/test_helpers.rs`:

```rust
pub struct MockSynapse {
    pub members: Vec<String>,
    pub fail_get_members: bool,
    pub fail_force_join: bool,
    pub fail_kick: bool,
    /// Child room IDs returned by `get_space_children`. Keyed by space ID.
    pub space_children: std::collections::HashMap<String, Vec<String>>,
    pub fail_get_space_children: bool,
}
```

Update the `Default` impl — `MockSynapse` uses `#[derive(Default)]`, which still works since `HashMap` and `bool` both implement `Default`.

Add the method to the `impl MatrixService for MockSynapse` block:

```rust
    async fn get_space_children(&self, space_id: &str) -> Result<Vec<String>, AppError> {
        if self.fail_get_space_children {
            return Err(AppError::Upstream {
                service: "synapse".into(),
                message: "mock get_space_children failure".into(),
            });
        }
        Ok(self.space_children.get(space_id).cloned().unwrap_or_default())
    }
```

**Step 4: Stub `get_space_children` in lifecycle_steps.rs local mock**

In `src/services/lifecycle_steps.rs`, add to the local `MockSynapse` impl of `MatrixService`:

```rust
        async fn get_space_children(&self, _: &str) -> Result<Vec<String>, AppError> {
            Ok(vec![])
        }
```

**Step 5: Run tests to verify compilation**

Run: `flox activate -- cargo test`
Expected: All existing tests pass. No behavior change — the new method is defined but not called yet.

**Step 6: Commit**

```bash
git add src/clients/synapse.rs src/test_helpers.rs src/services/lifecycle_steps.rs
git commit -m "feat(synapse): add get_space_children to MatrixService trait

Queries room state for m.space.child events to discover child rooms.
Returns empty vec for non-space rooms. No callers yet — behavior
unchanged."
```

---

### Task 2: Extract space expansion helper and wire into `reconcile_membership`

**Files:**
- Modify: `src/services/reconcile_membership.rs`

This task adds a helper function to expand a room ID into [room_id] + child rooms (if it's a space), then modifies `reconcile_membership` to use it.

**Step 1: Write the failing test**

Add to the `#[cfg(test)]` module in `src/services/reconcile_membership.rs`. First, update the local `MockSynapse` to support `get_space_children`:

The test module uses its own `MockSynapse` that implements `RoomManagementApi` (not `MatrixService`). The space expansion helper needs `MatrixService` (which has `get_space_children`). So we need to change the reconcile functions to accept a `MatrixService` instead of `RoomManagementApi`, OR pass `MatrixService` as an additional parameter for space discovery.

**Design decision:** Change `reconcile_membership` and `preview_membership` to accept `&dyn MatrixService` instead of `&dyn RoomManagementApi`. `MatrixService` already has `get_joined_room_members`, `force_join_user`, and `kick_user_from_room` — the same methods as `RoomManagementApi`. This avoids passing two separate trait objects.

Update function signatures:

```rust
pub async fn reconcile_membership(
    keycloak_id: &str,
    matrix_user_id: &str,
    policy: &PolicyEngine,
    keycloak_groups: &[String],
    synapse: &dyn MatrixService,  // was: &dyn RoomManagementApi
    audit: &AuditService,
    actor_subject: &str,
    actor_username: &str,
    remove_from_rooms: bool,
) -> Result<WorkflowOutcome, AppError>
```

```rust
pub async fn preview_membership(
    matrix_user_id: &str,
    policy: &PolicyEngine,
    keycloak_groups: &[String],
    synapse: &dyn MatrixService,  // was: &dyn RoomManagementApi
    remove_from_rooms: bool,
) -> Result<ReconcilePreview, AppError>
```

Update imports at top of file:

```rust
use crate::{
    clients::MatrixService,  // was: clients::RoomManagementApi
    // ... rest unchanged
};
```

Update the handler in `src/handlers/reconcile.rs` — change `room_mgmt` to use `state.synapse`:

```rust
    let synapse = state.synapse.as_ref().ok_or_else(|| {
        AppError::NotFound("Synapse is not configured — reconciliation is unavailable".into())
    })?;
    // ...
    let outcome = reconcile_membership(
        &keycloak_id,
        &matrix_user_id,
        &state.policy,
        &group_names,
        synapse.as_ref(),  // was: room_mgmt.as_ref()
        // ... rest unchanged
    )
```

And the preview handler similarly.

Update the test module's mock to implement `MatrixService` instead of `RoomManagementApi`:

```rust
    use crate::clients::MatrixService;
    use crate::models::synapse::{SynapseDevice, SynapseUser};

    #[derive(Default)]
    struct MockSynapse {
        pub members: Vec<String>,
        pub fail_get_members: bool,
        pub fail_force_join: bool,
        pub fail_kick: bool,
        pub space_children: std::collections::HashMap<String, Vec<String>>,
        pub fail_get_space_children: bool,
        pub joined: std::sync::Mutex<Vec<(String, String)>>,  // (user_id, room_id) for ordering verification
        pub kicked: std::sync::Mutex<Vec<(String, String)>>,  // (user_id, room_id)
    }

    #[async_trait]
    impl MatrixService for MockSynapse {
        async fn get_user(&self, _: &str) -> Result<Option<SynapseUser>, AppError> {
            unimplemented!()
        }
        async fn list_devices(&self, _: &str) -> Result<Vec<SynapseDevice>, AppError> {
            unimplemented!()
        }
        async fn delete_device(&self, _: &str, _: &str) -> Result<(), AppError> {
            unimplemented!()
        }
        async fn get_joined_room_members(&self, _room_id: &str) -> Result<Vec<String>, AppError> {
            if self.fail_get_members {
                return Err(AppError::Upstream {
                    service: "synapse".into(),
                    message: "mock member fetch failure".into(),
                });
            }
            Ok(self.members.clone())
        }
        async fn force_join_user(&self, user_id: &str, room_id: &str) -> Result<(), AppError> {
            if self.fail_force_join {
                return Err(AppError::Upstream {
                    service: "synapse".into(),
                    message: "mock force_join failure".into(),
                });
            }
            self.joined.lock().unwrap().push((user_id.to_string(), room_id.to_string()));
            Ok(())
        }
        async fn kick_user_from_room(
            &self,
            user_id: &str,
            room_id: &str,
            _reason: &str,
        ) -> Result<(), AppError> {
            if self.fail_kick {
                return Err(AppError::Upstream {
                    service: "synapse".into(),
                    message: "mock kick failure".into(),
                });
            }
            self.kicked.lock().unwrap().push((user_id.to_string(), room_id.to_string()));
            Ok(())
        }
        async fn get_space_children(&self, space_id: &str) -> Result<Vec<String>, AppError> {
            if self.fail_get_space_children {
                return Err(AppError::Upstream {
                    service: "synapse".into(),
                    message: "mock get_space_children failure".into(),
                });
            }
            Ok(self.space_children.get(space_id).cloned().unwrap_or_default())
        }
    }
```

Now add the space test:

```rust
    #[tokio::test]
    async fn space_mapping_joins_space_and_child_rooms() {
        let mut space_children = std::collections::HashMap::new();
        space_children.insert(
            "!space1:test.com".to_string(),
            vec!["!child1:test.com".to_string(), "!child2:test.com".to_string()],
        );
        let synapse = MockSynapse {
            space_children,
            ..Default::default()
        };
        let audit = audit().await;
        let policy = policy(vec![mapping("staff", "!space1:test.com")]);
        let groups = vec!["staff".to_string()];

        let outcome = reconcile_membership(
            "kc-1",
            "@alice:test.com",
            &policy,
            &groups,
            &synapse,
            &audit,
            "sub",
            "admin",
            false,
        )
        .await
        .unwrap();

        assert!(!outcome.has_warnings());
        let joined = synapse.joined.lock().unwrap();
        // Space joined first, then children
        assert_eq!(joined.len(), 3);
        assert_eq!(joined[0].1, "!space1:test.com");
        assert_eq!(joined[1].1, "!child1:test.com");
        assert_eq!(joined[2].1, "!child2:test.com");
    }
```

**Step 2: Run test to verify it fails**

Run: `flox activate -- cargo test space_mapping_joins_space_and_child_rooms`
Expected: FAIL — reconcile doesn't expand spaces yet.

**Step 3: Implement space expansion in reconcile_membership**

Add this helper function above `reconcile_membership`:

```rust
/// Expand a mapping's room ID into a list of target room IDs.
///
/// If the room is a space (has `m.space.child` state events), returns the
/// space ID followed by all child room IDs. Otherwise returns just the
/// room ID. If space child discovery fails, falls back to treating the
/// room as a single room and adds a warning.
async fn expand_targets(
    room_id: &str,
    synapse: &dyn MatrixService,
    warnings: &mut Vec<String>,
) -> Vec<String> {
    match synapse.get_space_children(room_id).await {
        Ok(children) if !children.is_empty() => {
            let mut targets = vec![room_id.to_string()];
            targets.extend(children);
            targets
        }
        Ok(_) => vec![room_id.to_string()],
        Err(e) => {
            warnings.push(format!(
                "Could not check space children for {room_id}: {e}"
            ));
            vec![room_id.to_string()]
        }
    }
}
```

Modify `reconcile_membership` to use it. Replace the loop body:

```rust
    for mapping in policy.all_mappings() {
        let in_group = keycloak_groups.contains(&mapping.keycloak_group);

        let mut expansion_warnings = Vec::new();
        let targets = expand_targets(&mapping.matrix_room_id, synapse, &mut expansion_warnings).await;
        for w in expansion_warnings {
            outcome.add_warning(w);
        }

        for target_room_id in &targets {
            let members = match synapse.get_joined_room_members(target_room_id).await {
                Ok(m) => m,
                Err(e) => {
                    outcome.add_warning(format!(
                        "Could not fetch members of {}: {}",
                        target_room_id, e
                    ));
                    continue;
                }
            };

            let in_room = members.contains(&matrix_user_id.to_string());

            if in_group && !in_room {
                let result = synapse
                    .force_join_user(matrix_user_id, target_room_id)
                    .await;
                let audit_result = if result.is_ok() {
                    AuditResult::Success
                } else {
                    AuditResult::Failure
                };
                let _ = audit
                    .log(
                        actor_subject,
                        actor_username,
                        Some(keycloak_id),
                        Some(matrix_user_id),
                        "join_room_on_reconcile",
                        audit_result,
                        serde_json::json!({
                            "room_id": target_room_id,
                            "keycloak_group": mapping.keycloak_group,
                        }),
                    )
                    .await;
                if let Err(e) = result {
                    outcome.add_warning(format!(
                        "Could not join {} to {}: {}",
                        matrix_user_id, target_room_id, e
                    ));
                }
            } else if remove_from_rooms && !in_group && in_room {
                // Kicks are handled in a separate pass below for correct ordering
            }
        }

        // Kick pass: child rooms first, then the space (reverse of join order)
        if remove_from_rooms && !in_group {
            for target_room_id in targets.iter().rev() {
                let members = match synapse.get_joined_room_members(target_room_id).await {
                    Ok(m) => m,
                    Err(e) => {
                        outcome.add_warning(format!(
                            "Could not fetch members of {}: {}",
                            target_room_id, e
                        ));
                        continue;
                    }
                };

                let in_room = members.contains(&matrix_user_id.to_string());

                if in_room {
                    let result = synapse
                        .kick_user_from_room(
                            matrix_user_id,
                            target_room_id,
                            "Removed from Keycloak group",
                        )
                        .await;
                    let audit_result = if result.is_ok() {
                        AuditResult::Success
                    } else {
                        AuditResult::Failure
                    };
                    let _ = audit
                        .log(
                            actor_subject,
                            actor_username,
                            Some(keycloak_id),
                            Some(matrix_user_id),
                            "kick_room_on_reconcile",
                            audit_result,
                            serde_json::json!({
                                "room_id": target_room_id,
                                "keycloak_group": mapping.keycloak_group,
                            }),
                        )
                        .await;
                    if let Err(e) = result {
                        outcome.add_warning(format!(
                            "Could not kick {} from {}: {}",
                            matrix_user_id, target_room_id, e
                        ));
                    }
                }
            }
        }
    }
```

Wait — the above restructuring duplicates the member-fetch for kick targets. A cleaner approach: do a single pass over targets for joins, then a reverse pass for kicks. But we need to avoid fetching members twice. Let me simplify.

Actually, the cleanest approach is: for each mapping, expand targets. Then for joins (in_group), iterate targets forward. For kicks (!in_group), iterate targets in reverse. Each iteration fetches members for that specific room. The double-fetch only happens in the case where `in_group` is false AND kicks are enabled — in that case we only do the kick pass (no join pass), so there's no duplication.

Revised logic — replace the entire loop:

```rust
    for mapping in policy.all_mappings() {
        let in_group = keycloak_groups.contains(&mapping.keycloak_group);

        let mut expansion_warnings = Vec::new();
        let targets = expand_targets(&mapping.matrix_room_id, synapse, &mut expansion_warnings).await;
        for w in expansion_warnings {
            outcome.add_warning(w);
        }

        if in_group {
            // Join pass: space first, then children
            for target_room_id in &targets {
                let members = match synapse.get_joined_room_members(target_room_id).await {
                    Ok(m) => m,
                    Err(e) => {
                        outcome.add_warning(format!(
                            "Could not fetch members of {target_room_id}: {e}"
                        ));
                        continue;
                    }
                };

                if !members.contains(&matrix_user_id.to_string()) {
                    let result = synapse
                        .force_join_user(matrix_user_id, target_room_id)
                        .await;
                    let audit_result = if result.is_ok() {
                        AuditResult::Success
                    } else {
                        AuditResult::Failure
                    };
                    let _ = audit
                        .log(
                            actor_subject,
                            actor_username,
                            Some(keycloak_id),
                            Some(matrix_user_id),
                            "join_room_on_reconcile",
                            audit_result,
                            serde_json::json!({
                                "room_id": target_room_id,
                                "keycloak_group": mapping.keycloak_group,
                            }),
                        )
                        .await;
                    if let Err(e) = result {
                        outcome.add_warning(format!(
                            "Could not join {matrix_user_id} to {target_room_id}: {e}"
                        ));
                    }
                }
            }
        } else if remove_from_rooms {
            // Kick pass: children first, then space (reverse order)
            for target_room_id in targets.iter().rev() {
                let members = match synapse.get_joined_room_members(target_room_id).await {
                    Ok(m) => m,
                    Err(e) => {
                        outcome.add_warning(format!(
                            "Could not fetch members of {target_room_id}: {e}"
                        ));
                        continue;
                    }
                };

                if members.contains(&matrix_user_id.to_string()) {
                    let result = synapse
                        .kick_user_from_room(
                            matrix_user_id,
                            target_room_id,
                            "Removed from Keycloak group",
                        )
                        .await;
                    let audit_result = if result.is_ok() {
                        AuditResult::Success
                    } else {
                        AuditResult::Failure
                    };
                    let _ = audit
                        .log(
                            actor_subject,
                            actor_username,
                            Some(keycloak_id),
                            Some(matrix_user_id),
                            "kick_room_on_reconcile",
                            audit_result,
                            serde_json::json!({
                                "room_id": target_room_id,
                                "keycloak_group": mapping.keycloak_group,
                            }),
                        )
                        .await;
                    if let Err(e) = result {
                        outcome.add_warning(format!(
                            "Could not kick {matrix_user_id} from {target_room_id}: {e}"
                        ));
                    }
                }
            }
        }
    }
```

**Step 4: Update existing test assertions**

The mock's `joined` and `kicked` fields changed from `Vec<String>` to `Vec<(String, String)>`. Update existing test assertions. For example:

```rust
// Before:
assert_eq!(*synapse.joined.lock().unwrap(), vec!["@alice:test.com"]);
// After:
let joined = synapse.joined.lock().unwrap();
assert_eq!(joined.len(), 1);
assert_eq!(joined[0], ("@alice:test.com".to_string(), "!room1:test.com".to_string()));
```

Actually — to minimize churn on existing tests, keep the mock simpler. Use `Vec<String>` for the user IDs (existing behavior), and add a separate `joined_rooms: Mutex<Vec<(String, String)>>` field. But that's over-engineered.

**Simpler approach:** Change `joined` and `kicked` to track `(user_id, room_id)` tuples. Update the ~4 existing tests that check these fields. The assertions just need the user_id check updated to check the first element of the tuple:

```rust
// Old: assert_eq!(*synapse.joined.lock().unwrap(), vec!["@alice:test.com"]);
// New:
let joined = synapse.joined.lock().unwrap();
assert_eq!(joined.len(), 1);
assert_eq!(joined[0].0, "@alice:test.com");
```

**Step 5: Update handler to use `state.synapse` instead of `state.room_mgmt`**

In `src/handlers/reconcile.rs`, update both handlers:

```rust
// reconcile handler
    let synapse = state.synapse.as_ref().ok_or_else(|| {
        AppError::NotFound("Synapse is not configured — reconciliation is unavailable".into())
    })?;
```

Replace `room_mgmt.as_ref()` with `synapse.as_ref()` in the call to `reconcile_membership`.

Same change in `reconcile_preview`.

Update the handler's import:
```rust
use crate::{
    // remove: services::reconcile_membership::{preview_membership, reconcile_membership, RoomAction},
    // keep RoomAction but update the rest
    services::reconcile_membership::{preview_membership, reconcile_membership, RoomAction},
    // ... no actual import change needed, just the function signatures changed
};
```

**Step 6: Run tests to verify the new test passes**

Run: `flox activate -- cargo test`
Expected: All tests pass, including `space_mapping_joins_space_and_child_rooms`.

**Step 7: Commit**

```bash
git add src/services/reconcile_membership.rs src/handlers/reconcile.rs
git commit -m "feat(reconcile): expand space mappings to include child rooms

Reconciliation now auto-detects spaces by querying m.space.child
state events. Space mappings expand to space + all children. Join
order: space first, then children. Kick order: children first, then
space."
```

---

### Task 3: Add space expansion tests for reconcile edge cases

**Files:**
- Modify: `src/services/reconcile_membership.rs` (test module)

**Step 1: Write additional space tests**

Add these tests to the `#[cfg(test)]` module:

```rust
    #[tokio::test]
    async fn space_mapping_kicks_children_before_space() {
        let mut space_children = std::collections::HashMap::new();
        space_children.insert(
            "!space1:test.com".to_string(),
            vec!["!child1:test.com".to_string(), "!child2:test.com".to_string()],
        );
        let synapse = MockSynapse {
            members: vec!["@alice:test.com".to_string()],
            space_children,
            ..Default::default()
        };
        let audit = audit().await;
        let policy = policy(vec![mapping("staff", "!space1:test.com")]);
        let groups: Vec<String> = vec![]; // not in group

        let outcome = reconcile_membership(
            "kc-1",
            "@alice:test.com",
            &policy,
            &groups,
            &synapse,
            &audit,
            "sub",
            "admin",
            true, // remove_from_rooms
        )
        .await
        .unwrap();

        assert!(!outcome.has_warnings());
        let kicked = synapse.kicked.lock().unwrap();
        assert_eq!(kicked.len(), 3);
        // Children kicked first (reverse order), then space
        assert_eq!(kicked[0].1, "!child2:test.com");
        assert_eq!(kicked[1].1, "!child1:test.com");
        assert_eq!(kicked[2].1, "!space1:test.com");
    }

    #[tokio::test]
    async fn mixed_space_and_room_mappings() {
        let mut space_children = std::collections::HashMap::new();
        space_children.insert(
            "!space1:test.com".to_string(),
            vec!["!child1:test.com".to_string()],
        );
        // !room2 has no children — treated as regular room
        let synapse = MockSynapse {
            space_children,
            ..Default::default()
        };
        let audit = audit().await;
        let policy = policy(vec![
            mapping("staff", "!space1:test.com"),
            mapping("staff", "!room2:test.com"),
        ]);
        let groups = vec!["staff".to_string()];

        let outcome = reconcile_membership(
            "kc-1",
            "@alice:test.com",
            &policy,
            &groups,
            &synapse,
            &audit,
            "sub",
            "admin",
            false,
        )
        .await
        .unwrap();

        assert!(!outcome.has_warnings());
        let joined = synapse.joined.lock().unwrap();
        assert_eq!(joined.len(), 3); // space + child + room
    }

    #[tokio::test]
    async fn space_child_discovery_failure_falls_back_to_single_room() {
        let synapse = MockSynapse {
            fail_get_space_children: true,
            ..Default::default()
        };
        let audit = audit().await;
        let policy = policy(vec![mapping("staff", "!space1:test.com")]);
        let groups = vec!["staff".to_string()];

        let outcome = reconcile_membership(
            "kc-1",
            "@alice:test.com",
            &policy,
            &groups,
            &synapse,
            &audit,
            "sub",
            "admin",
            false,
        )
        .await
        .unwrap();

        assert!(outcome.has_warnings());
        assert!(outcome.warnings[0].contains("Could not check space children"));
        // Still joined to the room itself
        let joined = synapse.joined.lock().unwrap();
        assert_eq!(joined.len(), 1);
        assert_eq!(joined[0].1, "!space1:test.com");
    }
```

**Step 2: Run tests**

Run: `flox activate -- cargo test`
Expected: All tests pass.

**Step 3: Commit**

```bash
git add src/services/reconcile_membership.rs
git commit -m "test(reconcile): add space expansion edge case tests

Covers: kick order (children before space), mixed space/room
mappings, and space discovery failure fallback."
```

---

### Task 4: Wire space expansion into `preview_membership`

**Files:**
- Modify: `src/services/reconcile_membership.rs`

**Step 1: Write the failing test**

```rust
    #[tokio::test]
    async fn preview_expands_space_children() {
        let mut space_children = std::collections::HashMap::new();
        space_children.insert(
            "!space1:test.com".to_string(),
            vec!["!child1:test.com".to_string(), "!child2:test.com".to_string()],
        );
        let synapse = MockSynapse {
            space_children,
            ..Default::default()
        };
        let policy = policy(vec![mapping("staff", "!space1:test.com")]);
        let groups = vec!["staff".to_string()];

        let preview = preview_membership("@alice:test.com", &policy, &groups, &synapse, false)
            .await
            .unwrap();

        assert_eq!(preview.joins.len(), 3); // space + 2 children
        assert_eq!(preview.joins[0].room_id, "!space1:test.com");
        assert_eq!(preview.joins[1].room_id, "!child1:test.com");
        assert_eq!(preview.joins[2].room_id, "!child2:test.com");
    }
```

**Step 2: Run test to verify it fails**

Run: `flox activate -- cargo test preview_expands_space_children`
Expected: FAIL — preview doesn't expand spaces yet.

**Step 3: Update `preview_membership` to use `expand_targets`**

Change the function to use `MatrixService` and expand targets (same pattern as `reconcile_membership`):

```rust
pub async fn preview_membership(
    matrix_user_id: &str,
    policy: &PolicyEngine,
    keycloak_groups: &[String],
    synapse: &dyn MatrixService,
    remove_from_rooms: bool,
) -> Result<ReconcilePreview, AppError> {
    let mut preview = ReconcilePreview::default();

    for mapping in policy.all_mappings() {
        let in_group = keycloak_groups.contains(&mapping.keycloak_group);

        let targets = expand_targets(&mapping.matrix_room_id, synapse, &mut preview.warnings).await;

        for target_room_id in &targets {
            let members = match synapse.get_joined_room_members(target_room_id).await {
                Ok(m) => m,
                Err(e) => {
                    preview.warnings.push(format!(
                        "Could not fetch members of {target_room_id}: {e}"
                    ));
                    continue;
                }
            };

            let in_room = members.contains(&matrix_user_id.to_string());
            let action = RoomAction {
                room_id: target_room_id.clone(),
                keycloak_group: mapping.keycloak_group.clone(),
            };

            if in_group && !in_room {
                preview.joins.push(action);
            } else if remove_from_rooms && !in_group && in_room {
                preview.kicks.push(action);
            } else if in_group && in_room {
                preview.already_correct.push(action);
            }
        }
    }

    Ok(preview)
}
```

**Step 4: Run tests**

Run: `flox activate -- cargo test`
Expected: All tests pass.

**Step 5: Commit**

```bash
git add src/services/reconcile_membership.rs
git commit -m "feat(preview): expand space mappings in reconcile preview

Preview now shows individual child rooms when a mapping targets a
space, giving admins full visibility before reconciling."
```

---

### Task 5: Wire space expansion into `kick_from_all_mapped_rooms`

**Files:**
- Modify: `src/services/lifecycle_steps.rs`

**Step 1: Write the failing test**

Update the local `MockSynapse` in the test module to support `get_space_children`:

```rust
    #[derive(Default)]
    struct MockSynapse {
        members: Vec<String>,
        fail_get_members: bool,
        fail_kick: bool,
        space_children: std::collections::HashMap<String, Vec<String>>,
        kicked_rooms: std::sync::Mutex<Vec<String>>,  // track room IDs in kick order
    }
```

Add `get_space_children` to the `impl MatrixService for MockSynapse`:

```rust
        async fn get_space_children(&self, space_id: &str) -> Result<Vec<String>, AppError> {
            Ok(self.space_children.get(space_id).cloned().unwrap_or_default())
        }
```

Update `kick_user_from_room` to track the room:

```rust
        async fn kick_user_from_room(&self, _: &str, room_id: &str, _: &str) -> Result<(), AppError> {
            if self.fail_kick {
                Err(AppError::Upstream {
                    service: "synapse".into(),
                    message: "mock kick failure".into(),
                })
            } else {
                self.kicked_rooms.lock().unwrap().push(room_id.to_string());
                Ok(())
            }
        }
```

Add the test:

```rust
    #[tokio::test]
    async fn kick_expands_space_and_kicks_children_first() {
        let audit = audit_svc().await;
        let mut space_children = std::collections::HashMap::new();
        space_children.insert(
            "!space1:example.com".to_string(),
            vec!["!child1:example.com".to_string(), "!child2:example.com".to_string()],
        );
        let synapse = MockSynapse {
            members: vec!["@alice:example.com".to_string()],
            space_children,
            ..Default::default()
        };
        let mappings = vec![mapping("staff", "!space1:example.com")];

        let outcome = kick_from_all_mapped_rooms(
            "offboard",
            "kc-1",
            "@alice:example.com",
            &mappings,
            &synapse,
            &audit,
            "sub",
            "admin",
        )
        .await;

        assert!(!outcome.has_warnings());
        let kicked = synapse.kicked_rooms.lock().unwrap();
        assert_eq!(kicked.len(), 3);
        // Children first, then space
        assert_eq!(kicked[0], "!child2:example.com");
        assert_eq!(kicked[1], "!child1:example.com");
        assert_eq!(kicked[2], "!space1:example.com");
    }
```

**Step 2: Run test to verify it fails**

Run: `flox activate -- cargo test kick_expands_space`
Expected: FAIL — `kick_from_all_mapped_rooms` doesn't expand spaces yet.

**Step 3: Implement space expansion in `kick_from_all_mapped_rooms`**

Modify `kick_from_all_mapped_rooms` to expand targets. Add a similar `expand_targets` helper (or make the one from reconcile_membership available — but since lifecycle_steps uses `MatrixService` directly and it's a different module, just inline the logic or add a shared helper).

Since `lifecycle_steps.rs` already uses `MatrixService`, add the expansion inline:

```rust
pub(crate) async fn kick_from_all_mapped_rooms(
    context: &str,
    keycloak_id: &str,
    matrix_user_id: &str,
    group_mappings: &[GroupMapping],
    synapse: &dyn MatrixService,
    audit: &AuditService,
    admin_subject: &str,
    admin_username: &str,
) -> WorkflowOutcome {
    let mut outcome = WorkflowOutcome::ok();
    let action = format!("kick_room_on_{context}");

    for mapping in group_mappings {
        // Expand spaces: discover child rooms and kick in reverse order
        // (children first, then space).
        let targets = match synapse.get_space_children(&mapping.matrix_room_id).await {
            Ok(children) if !children.is_empty() => {
                let mut t = vec![mapping.matrix_room_id.clone()];
                t.extend(children);
                t
            }
            Ok(_) => vec![mapping.matrix_room_id.clone()],
            Err(e) => {
                outcome.add_warning(format!(
                    "Could not check space children for {}: {}",
                    mapping.matrix_room_id, e
                ));
                vec![mapping.matrix_room_id.clone()]
            }
        };

        // Iterate in reverse: children first, then space
        for target_room_id in targets.iter().rev() {
            let members = match synapse.get_joined_room_members(target_room_id).await {
                Ok(m) => m,
                Err(e) => {
                    outcome.add_warning(format!(
                        "Could not fetch members of {target_room_id}: {e}"
                    ));
                    continue;
                }
            };

            if !members.contains(&matrix_user_id.to_string()) {
                continue;
            }

            let result = synapse
                .kick_user_from_room(matrix_user_id, target_room_id, "Offboarded")
                .await;
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
                        "room_id": target_room_id,
                        "keycloak_group": mapping.keycloak_group,
                    }),
                )
                .await;

            if let Err(e) = result {
                outcome.add_warning(format!(
                    "Could not kick {matrix_user_id} from {target_room_id}: {e}"
                ));
            }
        }
    }

    outcome
}
```

**Step 4: Run tests**

Run: `flox activate -- cargo test`
Expected: All tests pass.

**Step 5: Commit**

```bash
git add src/services/lifecycle_steps.rs
git commit -m "feat(lifecycle): expand spaces in kick_from_all_mapped_rooms

Offboard workflow now kicks from child rooms before the space itself,
matching the design's symmetric join/kick ordering."
```

---

### Task 6: Update handler tests and verify full integration

**Files:**
- Modify: `src/handlers/reconcile.rs` (test module)

The handler tests use `MockSynapse` from `test_helpers.rs` which now has `get_space_children`. The handler now uses `state.synapse` instead of `state.room_mgmt`. Verify all handler tests still pass and update any that broke.

**Step 1: Run the full test suite**

Run: `flox activate -- cargo test`
Expected: All tests pass. If handler tests fail because of the `room_mgmt` → `synapse` switch, update accordingly.

**Step 2: Run clippy and fmt**

Run: `flox activate -- cargo fmt && flox activate -- cargo clippy --all-targets -- -D warnings`
Expected: Clean.

**Step 3: Commit any fixes**

If any test updates were needed:

```bash
git add -u
git commit -m "test(reconcile): update handler tests for MatrixService migration

Handlers now use state.synapse directly instead of state.room_mgmt
for reconciliation."
```

---

### Task 7: Final verification and cleanup

**Step 1: Run the full pre-commit gate**

```bash
flox activate -- cargo fmt
flox activate -- cargo clippy --all-targets -- -D warnings
flox activate -- cargo test
```

All three must pass.

**Step 2: Review diff against design doc**

Read `docs/plans/2026-03-09-space-support-design.md` and verify all requirements are met:
- [x] `get_space_children` on `MatrixService` trait
- [x] Auto-detect spaces (no config change)
- [x] `reconcile_membership` expands space targets
- [x] `preview_membership` expands space targets
- [x] `kick_from_all_mapped_rooms` expands space targets
- [x] Join order: space first, then children
- [x] Kick order: children first, then space
- [x] Failure falls back to single-room behavior
- [x] Tests for all of the above

**Step 3: Commit design plan**

```bash
git add docs/plans/2026-03-09-space-support-plan.md
git commit -m "docs(spaces): add implementation plan"
```
