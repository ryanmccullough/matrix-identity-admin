# Matrix Identity Admin — Building Guide

## Purpose

This guide exists to keep `matrix-identity-admin` moving forward without a costly rewrite.

The project already solves a real problem: managing identity and lifecycle across Matrix Authentication Service (MAS), Keycloak, and Synapse for self-hosted Matrix environments. The goal is to keep shipping useful features while gradually improving structure.

The default strategy is:

**keep building, refactor only when a feature exposes a weak boundary.**

---

## What this project is

`matrix-identity-admin` is best understood as an **identity and lifecycle admin layer for self-hosted Matrix**.

It should own:

- user lifecycle management
- onboarding and invites
- group-to-room or group-to-space mapping
- offboarding and deprovisioning
- auditability of administrative actions
- reconciliation across Keycloak, MAS, and Synapse

It should not try to become everything at once.

It does **not** need to own, at least right now:

- a full observability platform
- a full moderation suite
- a federation reputation network
- a generic Matrix room management platform for every use case

---

## Core product definition

When in doubt, new work should support one or more of these core responsibilities:

1. **Identity**
   - user records
   - account state
   - authentication relationships

2. **Lifecycle**
   - invite
   - activate
   - disable
   - offboard

3. **Access**
   - group membership
   - room / space assignment
   - role mapping

4. **Administration**
   - admin actions
   - audit events
   - operational visibility

If a feature does not clearly fit here, it may belong in a separate app later.

---

## Build vs refactor rule

Use this rule before every new feature.

### Build now
Build the feature directly when most of the following are true:

- the feature fits project scope cleanly
- you can explain where it belongs
- the implementation touches only a few files or modules
- the existing code is imperfect but understandable
- the feature does not require redefining the app’s core state model

### Do a targeted refactor first
Do a small refactor before building when some of the following are true:

- you are about to duplicate logic
- vendor API calls are mixed into handlers again
- the feature is multi-step and failure handling is unclear
- user/account state is represented in inconsistent ways
- the new work spans too many unrelated files

### Consider a larger redesign only if
Only think about a major rewrite if these become consistently true:

- every feature requires invasive changes across the codebase
- there is no usable separation between logic and integrations
- state is fundamentally contradictory or broken
- the app no longer reflects the actual product model

Until that point, prefer **incremental refactoring while continuing to ship**.

---

## Architectural seams to protect

As the project grows, keep these four areas separate.

### 1. Domain models
Internal concepts owned by the app.

Examples:

- user
- invite
- lifecycle state
- group mapping
- audit event

These should describe the product’s concepts, not just mirror external APIs.

### 2. Connectors
Modules that talk to external systems.

Examples:

- MAS connector
- Keycloak connector
- Synapse connector
- mail connector

These should encapsulate HTTP calls, auth, request/response handling, and retries.

### 3. Workflows
Multi-step operations that represent business logic.

Examples:

- invite user
- onboard user
- disable user
- offboard user
- reconcile group membership

Workflows should coordinate connectors and domain state.

### 4. Interface layer
How humans or systems interact with the app.

Examples:

- API handlers
- web routes
- CLI commands
- UI actions

Interface code should be thin. It should call workflows, not contain lifecycle logic.

---

## Recommended internal shape

The project does not need a full rewrite, but it should gradually move toward a structure like this:

```text
src/
  domain/
    user.ts
    invite.ts
    lifecycle.ts
    audit.ts

  connectors/
    mas.ts
    keycloak.ts
    synapse.ts
    mail.ts

  workflows/
    invite-user.ts
    onboard-user.ts
    disable-user.ts
    offboard-user.ts
    reconcile-membership.ts

  api/
    routes/
    handlers/

  ui/
    ...
```

The exact language and file layout may differ, but the principle should stay the same.

---

## Canonical user model

One of the most important futureproofing moves is to create a simple internal user model.

Suggested baseline fields:

- email
- desired username
- mxid
- lifecycle state
- groups
- external IDs
  - keycloak user id
  - MAS user id if relevant
- audit metadata

This model should represent the app’s understanding of the user, even when external systems are temporarily out of sync.

That makes it easier to support:

- pending invites
- pre-provisioning
- suspended users
- offboarding workflows
- reconciliation and drift detection

---

## Preferred development strategy

Because coding output is limited, optimize for **small, high-leverage changes**.

### Prefer

- extracting one connector at a time
- extracting one workflow at a time
- introducing one domain model at a time
- refactoring without changing behavior
- adding narrow, incremental features

### Avoid

- rewriting the entire architecture
- changing every module at once
- mixing UI redesign with backend restructuring
- broad prompts that ask an AI tool to “clean up everything”

The best momentum comes from:

1. identify the next feature
2. identify the boundary it stresses
3. clean up that boundary
4. ship the feature

---

## Good prompt patterns for AI coding tools

Use small prompts with a single objective.

Examples:

- “Extract MAS API calls from this route into a connector.”
- “Create an `invite_user` workflow using these existing functions.”
- “Define a `UserLifecycleState` model and update these files to use it.”
- “Refactor this module without changing behavior.”
- “Move Keycloak group-sync logic into a dedicated service.”

Avoid prompts like:

- “Rewrite the whole app.”
- “Redesign the entire architecture from scratch.”
- “Refactor everything into a clean system.”

Small prompts usually produce better code and waste fewer tokens.

---

## Near-term roadmap

A practical order of operations:

### Phase 1 — Make it trustworthy

Focus on:

- reliable invite flow
- unified disable/offboard flow
- audit log for admin actions
- clear connectors for MAS, Keycloak, and Synapse
- basic lifecycle state model

### Phase 2 — Make it structurally sound

Focus on:

- extracting workflows
- reconciling user/group membership
- dry-run or preview support for admin actions
- better error handling across multi-step operations

### Phase 3 — Make it extensible

Focus on:

- provider interfaces
- policy configuration
- swappable identity or notification backends
- support for more deployment patterns

### Phase 4 — Make it polished

Focus on:

- better admin UI
- bulk actions
- dashboards
- templates for onboarding and org setup

---

## Decision checklist for every feature

Before starting a feature, answer these questions:

1. Is this a **domain** concern?
2. Is this a **connector** concern?
3. Is this a **workflow** concern?
4. Is this an **interface/UI** concern?

If the answer is “all of them in one place,” stop and separate one seam first.

Also ask:

- Does this fit the identity/lifecycle/access scope?
- Am I duplicating logic that should become a workflow or connector?
- Can this be shipped as a small increment?
- Would a small refactor make the next three features easier?

---

## Guiding principles

- Ship useful features continuously.
- Do not rewrite working code just because it is imperfect.
- Refactor only where the next feature needs clearer structure.
- Keep product scope centered on identity, lifecycle, and access.
- Treat external systems as integrations, not the app’s source of truth.
- Move toward a control-plane design gradually, not all at once.

---

## One-sentence direction

`matrix-identity-admin` should evolve into the **control plane for identity, access, and lifecycle management on self-hosted Matrix**.

