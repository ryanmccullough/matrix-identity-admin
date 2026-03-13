# UX Polish Design

## Goal

Adopt Pico CSS as the base stylesheet, trim custom CSS to MIA-specific styles only, and add accessibility and responsive improvements across all templates.

## Architecture

Subtractive approach: add Pico CSS via CDN, then remove redundant rules from `app.css`. Pico provides responsive tables, typography, buttons, forms, dialogs, focus indicators, dark mode, and mobile layout from semantic HTML alone. Templates get minor edits for Pico conventions and accessibility attributes.

## Components

### 1. Base template (`base.html`)

- Add Pico CSS `<link>` from CDN before `app.css`
- Add `aria-current="page"` on active nav link (pass current path to template)
- Wrap `<main>` content in Pico's `<div class="container">` for max-width/centering

### 2. CSS cleanup (`app.css`)

- **Remove:** button base styles, table styles, form/input styles, typography resets, link styles, dialog backdrop styling — Pico handles all of these
- **Keep:** status cards, stats grid, badge colors, detail lists, flash messages, MIA-specific layout
- **Add:** CSS custom properties for MIA brand colors (header background, accent) to override Pico defaults where needed

### 3. Responsive tables

- Wrap all `<table>` elements in `<figure>` — Pico makes these horizontally scrollable on mobile automatically
- No additional CSS needed

### 4. Accessibility additions

- `aria-current="page"` on active nav links
- `aria-label` on status indicators (the bullet symbols)
- `aria-live="polite"` on flash message containers
- `<caption>` on data tables (Sessions, Audit, Policy, Search results)
- `role="alertdialog"` + `aria-labelledby` on confirmation dialogs

### 5. Consistent confirmation dialogs

- Replace `onsubmit="return confirm()"` on bulk reconcile and session revoke with `<dialog>` modals matching existing pattern
- Removes native browser `confirm()` inconsistency

### 6. User detail page

- Wrap session and audit sections in `<details><summary>` for collapsible sections — Pico styles these natively
- Reduces scroll on pages with many sessions/audit entries

## Testing

- Visual verification: all pages render correctly with Pico
- Check mobile layout at 375px and 768px widths
- Verify dark mode doesn't break custom styles
- Confirm dialogs still open/close correctly

## Out of scope

- Icon system (future improvement)
- Loading spinners on form submit (separate feature)
- CSS build pipeline (no Node.js dependency)
- Custom dark mode toggle (Pico auto-detects from OS)
- Visual regression testing / Playwright (separate Phase 4 item)
- Accessibility audits via axe-core (separate Phase 4 item)
