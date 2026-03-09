# Matrix Identity Admin — Vision

## Purpose

This document defines the long-term vision for **matrix-identity-admin (MIA)** so that AI coding agents (Claude Code, OpenAI Codex, etc.) can reason about architectural decisions, scope boundaries, and feature priorities.

The goal is to ensure contributors and AI agents make consistent decisions that move the project toward a coherent system rather than a collection of scripts.

---

# Project Vision

`matrix-identity-admin` aims to become the **control plane for identity, access, and lifecycle management in self-hosted Matrix deployments.**

It fills the operational gap between:

- Matrix infrastructure (Synapse, MAS)
- Identity providers (Keycloak, Authentik, LDAP)
- Organizational policy (groups, roles, onboarding)

The project should enable administrators to manage Matrix the same way organizations manage access in modern collaboration systems like Slack or Google Workspace.

---

# Problem Statement

Self-hosted Matrix environments currently require administrators to manually coordinate several independent systems:

Identity
- Keycloak or other IdP

Authentication
- Matrix Authentication Service (MAS)

Messaging
- Synapse homeserver

Administration
- Synapse Admin
- CLI tools
- scripts

These systems do not share a unified lifecycle model.

Common operational problems include:

- onboarding new users
- assigning room access
- synchronizing identity groups with rooms
- removing access when users leave
- handling account state drift

There is currently **no canonical identity lifecycle orchestrator for Matrix.**

`matrix-identity-admin` aims to fill this gap.

---

# Core Concept

The system acts as an **identity lifecycle orchestrator**.

Instead of directly mirroring the state of external systems, the project maintains a **desired organizational state** and reconciles external systems to match that state.

Example:

Desired state

User: alice@example.com
Group: engineering

System enforces:

- Keycloak group membership
- MAS account lifecycle
- Matrix MXID existence
- membership in engineering rooms

If drift occurs, the system reconciles it.

---

# Core Responsibilities

The project should focus on five major capability areas.

## Identity

Manage identity records used by Matrix infrastructure.

Responsibilities:

- canonical user model
- identity linking
- MXID management
- username policy

Example fields:

email
username
mxid
lifecycle state
identity provider IDs

---

## Lifecycle

Manage the lifecycle of users within Matrix.

States may include:

invited
active
suspended
disabled
offboarded

Lifecycle operations include:

invite user
activate user
disable user
offboard user

---

## Access Control

Bridge identity groups with Matrix access.

Example:

Keycloak group → Matrix space → rooms

Policy example:

engineering group

members automatically join:

engineering space
engineering-general
engineering-rfc
engineering-private

Access control must support:

- room membership
- space membership
- power level assignment

---

## Automation

Automate operational workflows.

Examples:

invite workflow
room assignment
user reconciliation
access policy enforcement

Automation should be implemented as **workflows**.

---

## Administration

Provide administrative visibility and control.

Examples:

audit logging
admin actions
system status
lifecycle history

---

# Architectural Direction

The system should gradually evolve into a **control plane architecture**.

This means:

- defining internal models
- reconciling external systems
- enforcing policies

rather than simply wrapping APIs.

---

# Internal Architecture Model

The project should be organized into four conceptual layers.

## Domain Layer

Represents the system's internal concepts.

Examples:

User
Invite
LifecycleState
GroupMapping
AuditEvent

These models should represent organizational state rather than external API responses.

---

## Connector Layer

Responsible for interacting with external systems.

Examples:

MAS connector
Keycloak connector
Synapse connector
Mail connector

Connectors should encapsulate:

API requests
authentication
error handling

---

## Workflow Layer

Implements business logic.

Examples:

invite_user
onboard_user
disable_user
offboard_user
reconcile_memberships

Workflows coordinate domain models and connectors.

---

## Interface Layer

Handles input/output.

Examples:

REST API
Web UI
CLI
Webhooks

Interface code should remain thin and delegate logic to workflows.

---

# Key Feature Direction

## Group → Space → Room Synchronization

One of the most valuable capabilities is mapping identity groups to Matrix access.

Example policy:

Group: engineering

Users automatically join:

Engineering space
Engineering rooms

The system enforces this policy through reconciliation.

---

## Onboarding Automation

Example workflow:

Admin creates invite

User registers through MAS

System creates MXID

User automatically joins required rooms

---

## Offboarding Automation

Example workflow:

User disabled in identity provider

System:

removes room memberships
revokes sessions
disables account

---

## Drift Reconciliation

The system periodically checks whether actual state matches desired state.

Example:

User removed from group but still present in room.

System detects drift and removes access.

---

# Non-Goals

To keep the project focused, certain capabilities are explicitly out of scope for now.

These include:

- replacing Synapse Admin
- building a general moderation platform
- building a full observability suite
- implementing federation governance

The project should remain focused on **identity and lifecycle orchestration.**

---

# Integration Philosophy

The project should integrate with existing Matrix ecosystem tools rather than replacing them.

Examples:

Synapse Admin → server administration

Maubot → automation bots

Hookshot → event bridges

MIA → identity lifecycle orchestration

---

# Development Principles

AI agents and contributors should follow these principles.

1. Prefer incremental improvements over large rewrites.

2. Keep connectors isolated from domain logic.

3. Avoid duplicating external API logic in multiple places.

4. Implement workflows for multi-step operations.

5. Treat external systems as integrations, not sources of truth.

---

# Long-Term Vision

In the long term, `matrix-identity-admin` should provide functionality similar to:

Slack Admin Console

Google Workspace Admin

But for **self-hosted Matrix environments.**

Administrators should be able to manage identity, access, and lifecycle from one system.

---

# One-Sentence Vision

`matrix-identity-admin` is the **identity and lifecycle control plane for self-hosted Matrix infrastructure.**
