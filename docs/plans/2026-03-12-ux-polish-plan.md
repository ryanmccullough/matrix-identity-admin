# UX Polish Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Adopt Pico CSS as base stylesheet, trim custom CSS, add responsive tables, active nav, accessibility attributes, collapsible sections, and consistent confirmation dialogs.

**Architecture:** Add Pico CSS via CDN link in `base.html` before `app.css`. Remove redundant CSS rules from `app.css` that Pico now provides. Update templates for Pico conventions and accessibility. Pico is classless — it styles semantic HTML elements directly, so existing markup mostly "just works."

**Tech Stack:** Pico CSS 2.x (CDN), Askama templates, HTMX

---

### Task 1: Add Pico CSS and update base template

**Files:**
- Modify: `templates/base.html`
- Modify: `templates/login.html`
- Modify: `templates/error.html`

**Step 1: Update `base.html`**

Add Pico CSS CDN link before app.css. Add `data-theme="light"` on `<html>` (Pico default). Pass `current_path` to template for active nav. Add `<div class="container">` inside `<main>`.

Replace full content of `templates/base.html` with:

```html
<!DOCTYPE html>
<html lang="en" data-theme="light">
<head>
  <meta charset="UTF-8">
  <meta name="viewport" content="width=device-width, initial-scale=1.0">
  <title>{% block title %}Matrix Identity Admin{% endblock %}</title>
  <link rel="stylesheet" href="https://cdn.jsdelivr.net/npm/@picocss/pico@2/css/pico.min.css">
  <link rel="stylesheet" href="/static/app.css">
</head>
<body>
  <header>
    <nav>
      <ul>
        <li><strong>Matrix Identity Admin</strong></li>
      </ul>
      <ul>
        <li><a href="/" {% if current_path == "/" %}aria-current="page"{% endif %}>Dashboard</a></li>
        <li><a href="/users/search" {% if current_path.starts_with("/users") %}aria-current="page"{% endif %}>Users</a></li>
        <li><a href="/policy" {% if current_path.starts_with("/policy") %}aria-current="page"{% endif %}>Policy</a></li>
        <li><a href="/templates" {% if current_path == "/templates" %}aria-current="page"{% endif %}>Templates</a></li>
        <li><a href="/audit" {% if current_path.starts_with("/audit") %}aria-current="page"{% endif %}>Audit Log</a></li>
      </ul>
      <ul>
        {% block nav_user %}{% endblock %}
      </ul>
    </nav>
  </header>

  <main class="container">
    {% block content %}{% endblock %}
  </main>
  <script src="/static/htmx.min.js"></script>
</body>
</html>
```

**Step 2: Update `login.html`**

Add Pico CSS CDN link. Use Pico's container centering.

Replace full content of `templates/login.html` with:

```html
<!DOCTYPE html>
<html lang="en" data-theme="light">
<head>
  <meta charset="UTF-8">
  <meta name="viewport" content="width=device-width, initial-scale=1.0">
  <title>Sign In — Matrix Identity Admin</title>
  <link rel="stylesheet" href="https://cdn.jsdelivr.net/npm/@picocss/pico@2/css/pico.min.css">
  <link rel="stylesheet" href="/static/app.css">
</head>
<body class="login-page">
  <main class="container">
    <article class="login-box">
      <h1>Matrix Identity Admin</h1>
      <p>Sign in with your Keycloak admin account.</p>
      <a href="/auth/login" role="button">Sign in with Keycloak</a>
    </article>
  </main>
</body>
</html>
```

**Step 3: Update `error.html`**

Add Pico CSS CDN link.

Replace full content of `templates/error.html` with:

```html
<!DOCTYPE html>
<html lang="en" data-theme="light">
<head>
  <meta charset="UTF-8">
  <meta name="viewport" content="width=device-width, initial-scale=1.0">
  <title>Error {{ status }} — Matrix Identity Admin</title>
  <link rel="stylesheet" href="https://cdn.jsdelivr.net/npm/@picocss/pico@2/css/pico.min.css">
  <link rel="stylesheet" href="/static/app.css">
</head>
<body>
  <main class="container" style="margin-top:4rem">
    <h1>Error {{ status }}</h1>
    <p>{{ message }}</p>
    <a href="/">Go to dashboard</a>
  </main>
</body>
</html>
```

**Step 4: Add `current_path` to all template structs**

Every template struct that extends `base.html` needs a `current_path: String` field. These are in the handler files. Search for `#[template(path =` to find them all.

The field should be set to the request path. In each handler, add:

```rust
current_path: "/the/path".to_string(),
```

Use these values:
- `dashboard.rs` (`DashboardTemplate`): `"/"`
- `dashboard.rs` (`StatusCardTemplate`): does not extend base.html — skip
- `users.rs` (`UsersSearchTemplate`): `"/users/search"`
- `users.rs` (`UserDetailTemplate`): `"/users"`
- `audit.rs` (`AuditTemplate`): `"/audit"`
- `policy.rs` (`PolicyTemplate`): `"/policy"`
- `templates.rs` (`TemplatesTemplate`): `"/templates"`
- `reconcile.rs` (`ReconcilePreviewTemplate`): does not extend base.html — skip
- `bulk_reconcile.rs` (`BulkReconcileResultTemplate`): `"/"`

**Step 5: Update `nav_user` blocks**

Every template that has a `nav_user` block needs to wrap the content in `<li>` tags for Pico's `<ul>` nav. Replace the nav_user block pattern in ALL child templates (dashboard.html, users_search.html, user_detail.html, audit.html, policy.html, templates.html, bulk_reconcile_result.html):

From:
```html
{% block nav_user %}
  <span>{{ username }}</span>
  <form method="post" action="/auth/logout" style="display:inline">
    <input type="hidden" name="_csrf" value="{{ csrf_token }}">
    <button type="submit" class="btn-link">Sign out</button>
  </form>
{% endblock %}
```

To:
```html
{% block nav_user %}
  <li>{{ username }}</li>
  <li>
    <form method="post" action="/auth/logout" style="display:inline">
      <input type="hidden" name="_csrf" value="{{ csrf_token }}">
      <button type="submit" class="outline secondary btn-link">Sign out</button>
    </form>
  </li>
{% endblock %}
```

**Step 6: Run pre-commit checks**

```bash
flox activate -- cargo fmt
flox activate -- cargo clippy --all-targets -- -D warnings
flox activate -- cargo test
```

All must pass. Some handler tests check for specific HTML content — verify they still pass with the new nav structure.

**Step 7: Commit**

```bash
git add templates/ src/handlers/
git commit -m "style: add Pico CSS and active nav indicator"
```

---

### Task 2: Trim app.css — remove Pico-redundant rules

**Files:**
- Modify: `static/app.css`

**Step 1: Remove rules that Pico now handles**

Remove these sections from `app.css` (Pico provides equivalent styling):

- **Reset & base** (lines 1-13): `*, body, a` rules — Pico has its own reset
- **Nav** (lines 15-40): `header, nav, .brand, .nav-links, .nav-user` — Pico styles `<nav>` with `<ul>` natively
- **Layout** (lines 42-43): `main` max-width/margin — Pico's `.container` handles this
- **Tables** (lines 59-64): `table, th, td, tr` rules — Pico styles tables
- **Forms** (lines 68-80): `.search-row input, .filter-row input/select` base styles — Pico styles form elements
- **Buttons** (lines 82-110): `.btn, .btn-primary, .btn-danger, .btn-sm, .btn-link` — Pico has button styling. Keep `.btn-link` for the nav sign-out button only if needed
- **Confirmation dialogs** (lines 236-248): `.confirm-dialog, ::backdrop, .dialog-actions` — Pico styles `<dialog>` natively

**Step 2: Keep MIA-specific rules**

Keep these sections (Pico doesn't provide equivalents):

- `.page-header` (lines 45-47)
- `.card` (lines 49-57) — Pico has `<article>` but `.card` is used everywhere; keep for now
- `.detail-list` (lines 112-120)
- `.muted` (line 123)
- `.badge-ok, .badge-info, .badge-warning` (lines 124-126)
- `.result-success, .result-failure` (lines 128-129)
- `.flash, .flash-notice, .flash-warning, .flash-error` (lines 131-135)
- `.pagination` (lines 137-139)
- `.btn-disabled` (line 139)
- `.stats-grid, .stat-card` (lines 141-145)
- `.login-page, .login-box` (lines 147-151) — adjust to work with Pico
- `.status-grid, .status-item, .status-label, .status-value, .status-sub, .status-ok/err/warn` (lines 153-178)
- `.form-row, .form-group` (lines 180-207)
- `.alert, .alert-success, .alert-warning` (lines 209-228)
- `.action-buttons` (line 232)
- `.session-count, .badge-muted, .hidden, .toggle-finished` (lines 250-256)

**Step 3: Add CSS custom properties**

Add at the top of `app.css`:

```css
:root {
  --mia-header-bg: #1a1a2e;
  --mia-accent: #0055cc;
  --mia-danger: #c0392b;
  --mia-success: #27ae60;
  --mia-warning: #e67e22;
  --mia-muted: #888;
}
```

Replace hardcoded color values in the kept rules with these variables.

**Step 4: Add Pico overrides for MIA branding**

```css
/* Override Pico header to use MIA brand */
header {
  background: var(--mia-header-bg);
  --pico-color: #fff;
}

/* Override Pico nav link colors */
header nav a {
  color: #ccc;
}
header nav a:hover {
  color: #fff;
}
header nav [aria-current="page"] {
  color: #fff;
  font-weight: 600;
}
```

**Step 5: Run pre-commit checks**

```bash
flox activate -- cargo fmt
flox activate -- cargo clippy --all-targets -- -D warnings
flox activate -- cargo test
```

One test in `dashboard.rs` checks for `"status-grid"` — this class is kept, so it should pass.

**Step 6: Commit**

```bash
git add static/app.css
git commit -m "style: trim CSS to MIA-specific rules only"
```

---

### Task 3: Responsive tables — wrap in `<figure>`

**Files:**
- Modify: `templates/dashboard.html`
- Modify: `templates/users_search.html`
- Modify: `templates/user_detail.html`
- Modify: `templates/audit.html`
- Modify: `templates/policy.html`
- Modify: `templates/templates.html`

**Step 1: Wrap every `<table>` in `<figure>`**

In each template, find every `<table>` and wrap it:

```html
<!-- Before -->
<table>
  ...
</table>

<!-- After -->
<figure>
  <table>
    ...
  </table>
</figure>
```

Tables to wrap:
- `dashboard.html`: recent actions table (line 92)
- `users_search.html`: search results table (line 37)
- `user_detail.html`: sessions table (line 180), audit table (line 234)
- `audit.html`: audit log table (line 48)
- `policy.html`: bindings table (line 31)
- `templates.html`: templates table (line 57)

**Step 2: Add `<caption>` to each table for accessibility**

```html
<table>
  <caption>Search results</caption>
  ...
```

Captions:
- dashboard.html recent actions: `Recent admin actions`
- users_search.html: `Search results`
- user_detail.html sessions: `MAS sessions`
- user_detail.html audit: `Admin actions for this user`
- audit.html: `Audit log entries`
- policy.html bindings: `Policy bindings`
- templates.html: `Onboarding templates`

**Step 3: Run pre-commit checks**

```bash
flox activate -- cargo fmt
flox activate -- cargo clippy --all-targets -- -D warnings
flox activate -- cargo test
```

**Step 4: Commit**

```bash
git add templates/
git commit -m "style: wrap tables in figure for responsive scroll"
```

---

### Task 4: Accessibility — ARIA attributes

**Files:**
- Modify: `templates/status_card.html`
- Modify: `templates/user_detail.html`
- Modify: `templates/dashboard.html`
- Modify: `templates/users_search.html`
- Modify: `templates/audit.html`
- Modify: `templates/policy.html`
- Modify: `templates/templates.html`

**Step 1: Add `aria-label` to status indicators**

In `status_card.html`, replace the `&#9679;` bullets with labeled spans:

```html
<!-- Before -->
<span class="status-value">{% if keycloak_ok %}&#9679;  reachable{% else %}&#9679;  unreachable{% endif %}</span>

<!-- After -->
<span class="status-value">{% if keycloak_ok %}<span aria-hidden="true">&#9679;</span> reachable{% else %}<span aria-hidden="true">&#9679;</span> unreachable{% endif %}</span>
```

Apply to all three status items (Keycloak, MAS, Synapse).

**Step 2: Add `aria-live="polite"` to flash message containers**

In each template that has flash messages, wrap them in a live region. Add a wrapper div right after `{% block content %}` in the templates that show flashes:

In `dashboard.html`, `users_search.html`, `user_detail.html`, `templates.html`:

```html
<div aria-live="polite">
{% match notice %}
{% when Some with (msg) %}
<div class="flash flash-notice">{{ msg }}</div>
{% when None %}{% endmatch %}
...
</div>
```

In `policy.html` (uses alert classes):

```html
<div aria-live="polite">
{% if !notice.is_empty() %}
<div class="alert alert-success">{{ notice }}</div>
{% endif %}
{% if !warning.is_empty() %}
<div class="alert alert-warning">{{ warning }}</div>
{% endif %}
</div>
```

**Step 3: Add `role="alertdialog"` and `aria-labelledby` to confirmation dialogs**

In `user_detail.html`, update each `<dialog>`:

```html
<!-- Before -->
<dialog id="dlg-logout" class="confirm-dialog">
  <h3>Force Keycloak Logout</h3>

<!-- After -->
<dialog id="dlg-logout" class="confirm-dialog" role="alertdialog" aria-labelledby="dlg-logout-title">
  <h3 id="dlg-logout-title">Force Keycloak Logout</h3>
```

Apply to all dialogs: `dlg-logout`, `dlg-disable`, `dlg-offboard`, `dlg-reactivate`, `dlg-delete`, `dlg-reconcile`.

**Step 4: Run pre-commit checks**

```bash
flox activate -- cargo fmt
flox activate -- cargo clippy --all-targets -- -D warnings
flox activate -- cargo test
```

**Step 5: Commit**

```bash
git add templates/
git commit -m "a11y: add ARIA labels, live regions, dialog roles"
```

---

### Task 5: Consistent confirmation dialogs

**Files:**
- Modify: `templates/user_detail.html`
- Modify: `templates/dashboard.html`
- Modify: `templates/templates.html`
- Modify: `templates/reconcile_preview.html`

**Step 1: Replace `onsubmit="return confirm()"` with `<dialog>` in session revoke**

In `user_detail.html`, replace the inline confirm on session revoke (line 211) with a dialog trigger button. Since there are multiple sessions, use a JavaScript function to set the session ID dynamically.

Replace the session revoke form (lines 208-216):

```html
{% if session.state == "active" %}
<button type="button" class="btn btn-danger btn-sm"
        onclick="openRevokeDialog('{{ session.id }}', '{{ session.session_type }}')">Revoke</button>
{% endif %}
```

Add a single dialog before `</div>` of the sessions card:

```html
<dialog id="dlg-revoke-session" class="confirm-dialog" role="alertdialog" aria-labelledby="dlg-revoke-title">
  <h3 id="dlg-revoke-title">Revoke Session</h3>
  <p>This will immediately terminate this session. The user will need to log in again.</p>
  <div class="dialog-actions">
    <button type="button" class="btn" onclick="this.closest('dialog').close()">Cancel</button>
    <form id="revoke-session-form" method="post" action="">
      <input type="hidden" name="_csrf" value="{{ csrf_token }}">
      <input type="hidden" name="session_type" id="revoke-session-type" value="">
      <button type="submit" class="btn btn-danger">Revoke Session</button>
    </form>
  </div>
</dialog>
```

Add JavaScript:

```javascript
function openRevokeDialog(sessionId, sessionType) {
  var form = document.getElementById('revoke-session-form');
  form.action = '/users/{{ user.keycloak_id }}/sessions/' + sessionId + '/revoke';
  document.getElementById('revoke-session-type').value = sessionType;
  document.getElementById('dlg-revoke-session').showModal();
}
```

**Step 2: Replace `onsubmit="return confirm()"` in bulk reconcile**

In `dashboard.html` (line 80), replace:

```html
<form method="post" action="/users/reconcile/all"
      onsubmit="return confirm('Reconcile room membership for ALL enabled users? This may take a while.')">
```

With a dialog trigger:

```html
<button type="button" class="btn btn-primary" onclick="document.getElementById('dlg-bulk-reconcile').showModal()">Reconcile All Users</button>

<dialog id="dlg-bulk-reconcile" class="confirm-dialog" role="alertdialog" aria-labelledby="dlg-bulk-title">
  <h3 id="dlg-bulk-title">Bulk Reconcile</h3>
  <p>This will reconcile room membership for <strong>all enabled users</strong>. This may take a while for large user bases.</p>
  <div class="dialog-actions">
    <button type="button" class="btn" onclick="this.closest('dialog').close()">Cancel</button>
    <form method="post" action="/users/reconcile/all">
      <input type="hidden" name="_csrf" value="{{ csrf_token }}">
      <button type="submit" class="btn btn-primary">Reconcile All</button>
    </form>
  </div>
</dialog>
```

**Step 3: Replace `onsubmit="return confirm()"` in template delete**

In `templates.html` (line 76), replace:

```html
<form method="post" action="/templates/delete"
      onsubmit="return confirm('Delete template ' + this.dataset.name + '?')" data-name="{{ tmpl.name }}">
```

With a dialog trigger:

```html
<button type="button" class="btn btn-danger btn-sm"
        onclick="openDeleteTemplateDialog('{{ tmpl.name }}')">Delete</button>
```

Add a single dialog and JavaScript at the bottom of the templates card:

```html
<dialog id="dlg-delete-template" class="confirm-dialog" role="alertdialog" aria-labelledby="dlg-del-tmpl-title">
  <h3 id="dlg-del-tmpl-title">Delete Template</h3>
  <p>Delete template <strong id="del-tmpl-name"></strong>? This cannot be undone.</p>
  <div class="dialog-actions">
    <button type="button" class="btn" onclick="this.closest('dialog').close()">Cancel</button>
    <form id="delete-template-form" method="post" action="/templates/delete">
      <input type="hidden" name="_csrf" value="{{ csrf_token }}">
      <input type="hidden" name="name" id="del-tmpl-input" value="">
      <button type="submit" class="btn btn-danger">Delete</button>
    </form>
  </div>
</dialog>

<script>
function openDeleteTemplateDialog(name) {
  document.getElementById('del-tmpl-name').textContent = name;
  document.getElementById('del-tmpl-input').value = name;
  document.getElementById('dlg-delete-template').showModal();
}
</script>
```

**Step 4: Replace `onsubmit="return confirm()"` in reconcile preview**

In `reconcile_preview.html` (line 54), replace:

```html
<form method="post" action="/users/{{ keycloak_id }}/reconcile" style="margin-top:1rem"
      onsubmit="return confirm('Apply these changes now?')">
```

With:

```html
<button type="button" class="btn btn-primary" style="margin-top:1rem"
        onclick="document.getElementById('dlg-confirm-reconcile').showModal()">Confirm and Run</button>

<dialog id="dlg-confirm-reconcile" class="confirm-dialog" role="alertdialog" aria-labelledby="dlg-confirm-reconcile-title">
  <h3 id="dlg-confirm-reconcile-title">Apply Changes</h3>
  <p>Apply the previewed membership changes now?</p>
  <div class="dialog-actions">
    <button type="button" class="btn" onclick="this.closest('dialog').close()">Cancel</button>
    <form method="post" action="/users/{{ keycloak_id }}/reconcile">
      <input type="hidden" name="_csrf" value="{{ csrf_token }}">
      <button type="submit" class="btn btn-primary">Apply Changes</button>
    </form>
  </div>
</dialog>
```

**Step 5: Run pre-commit checks**

```bash
flox activate -- cargo fmt
flox activate -- cargo clippy --all-targets -- -D warnings
flox activate -- cargo test
```

**Step 6: Commit**

```bash
git add templates/
git commit -m "style: replace native confirm() with dialog modals"
```

---

### Task 6: Collapsible sections on user detail page

**Files:**
- Modify: `templates/user_detail.html`

**Step 1: Wrap sessions card content in `<details>`**

Replace the sessions card (lines 170-226) structure. Keep the `<div class="card">` wrapper, but put the table inside `<details>`:

```html
<!-- Sessions -->
<div class="card">
  <details open>
    <summary>
      MAS Sessions
      {% if !user.sessions.is_empty() %}
        <span class="session-count">({{ active_session_count }} active{% if finished_session_count > 0 %}, {{ finished_session_count }} finished{% endif %})</span>
      {% endif %}
    </summary>
    {% if user.sessions.is_empty() %}
      <p class="muted">No sessions found.</p>
    {% else %}
      <!-- existing table content here, wrapped in <figure> from Task 3 -->
    {% endif %}
  </details>
</div>
```

Remove the `<h2>` that was previously there — `<summary>` replaces it. Pico styles `<details>/<summary>` natively.

**Step 2: Wrap audit card content in `<details>`**

Same pattern for the audit card (lines 228-250):

```html
<!-- Audit -->
<div class="card">
  <details>
    <summary>Recent Admin Actions</summary>
    {% if audit_logs.is_empty() %}
      <p class="muted">No admin actions recorded for this user.</p>
    {% else %}
      <!-- existing table content here -->
    {% endif %}
  </details>
</div>
```

Note: sessions default to `open`, audit defaults to closed (less important, reduces scroll).

**Step 3: Run pre-commit checks**

```bash
flox activate -- cargo fmt
flox activate -- cargo clippy --all-targets -- -D warnings
flox activate -- cargo test
```

**Step 4: Commit**

```bash
git add templates/user_detail.html
git commit -m "style: collapsible sessions and audit sections"
```

---

### Task 7: Visual verification

This task is manual — no code changes.

**Step 1: Start the app**

```bash
flox activate -- cargo run
```

**Step 2: Verify each page at desktop width (1200px+)**

1. **Login page** (`/auth/login`) — centered card, Keycloak button styled
2. **Dashboard** (`/`) — active nav on "Dashboard", stats grid, status card, invite form, recent actions table
3. **User search** (`/users/search`) — active nav on "Users", search input, results table
4. **User detail** (`/users/{id}`) — back link, identity card, collapsible sessions (open by default), collapsible audit (closed by default), all dialogs open/close correctly
5. **Audit log** (`/audit`) — active nav on "Audit Log", filter row, table with pagination
6. **Policy** (`/policy`) — active nav on "Policy", bindings table, add form
7. **Templates** (`/templates`) — active nav on "Templates", create form, templates table

**Step 3: Verify at mobile width (375px)**

1. Tables scroll horizontally (no page overflow)
2. Nav wraps or collapses gracefully
3. Cards don't overflow
4. Dialogs are usable

**Step 4: Verify dark mode**

Set OS to dark mode. Check:
1. Header still uses MIA brand color
2. Cards, tables, forms are readable
3. Status colors (green/red/orange) still contrast well
4. Flash messages are readable

**Step 5: Verify accessibility**

1. Tab through nav — active page is indicated
2. Tab to buttons — focus indicator visible
3. Open a dialog — focus trapped in dialog
4. Screen reader: flash messages announced (aria-live)
