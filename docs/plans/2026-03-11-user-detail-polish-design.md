# User Detail Page Polish — Design

## Goal

Polish the user detail page with context-aware actions, styled confirmation modals, session display improvements, and correlation status badges.

## Changes

### 1. Context-aware action buttons

Show only actions valid for the current lifecycle state:

| State | Show | Hide |
|-------|------|------|
| Invited | Delete | Disable, Reactivate, Offboard, Force Logout |
| Active | Disable, Offboard, Delete, Force Logout | Reactivate |
| Disabled | Reactivate, Delete | Disable, Offboard, Force Logout |

Reconcile button visibility unchanged (controlled by `synapse_enabled`).

### 2. Styled confirmation modals

Replace `confirm()` with native `<dialog>` elements:
- Each destructive action gets its own `<dialog>` with consequence description
- Danger styling (red confirm button, warning text)
- Cancel / Confirm buttons
- Vanilla JS: button click opens dialog, Cancel closes, Confirm submits the form

### 3. Session display improvements

- Header: "MAS Sessions (N active)" with count
- Status badge per session: green "Active" / gray "Finished"
- Finished sessions hidden by default with "Show N finished" toggle button
- Vanilla JS toggle

### 4. Correlation status badge

- Badge next to the Correlation value in identity card
- Confirmed = `badge-ok` (green), Inferred = `badge-info` (blue/yellow)

## Approach

Template + CSS changes only. No new Rust code needed — all data (`lifecycle_state`, `correlation_status`, session states) is already available in the template context. Add `CorrelationStatus::css_class()` method to unified.rs for consistency with `LifecycleState::css_class()`.
