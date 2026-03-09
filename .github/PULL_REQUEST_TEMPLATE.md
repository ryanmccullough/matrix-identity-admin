## What

<!-- One or two sentences: what changed and why. -->

## Type of change

<!-- Mark one -->

- [ ] `feat` — new feature or capability
- [ ] `fix` — bug fix
- [ ] `refactor` — code restructuring, no behaviour change
- [ ] `test` — tests only
- [ ] `ci` — CI/CD changes
- [ ] `docs` — documentation only
- [ ] `chore` — maintenance

## Branch name

<!-- Confirm branch follows type/short-description in kebab-case -->
<!-- e.g. feat/lifecycle-state-model, fix/mas-token-refresh -->

## Local checks (required — all must pass before requesting review)

- [ ] `cargo fmt` — no formatting changes outstanding
- [ ] `cargo clippy --all-targets -- -D warnings` — zero warnings
- [ ] `cargo test` — all tests pass

## Testing

- [ ] New handlers: success path, unauthenticated (→ login), invalid CSRF (400), upstream failure (502) covered
- [ ] New service/workflow functions: happy path + each upstream failure mode covered
- [ ] Coverage does not regress (check Codecov report on this PR)

## Security & correctness

- [ ] All new mutations are audit-logged
- [ ] No secrets, tokens, or credentials logged or exposed to the browser
- [ ] No `unwrap()` or `expect()` in production code paths
- [ ] New upstream calls have request timeouts

## Housekeeping

- [ ] `.env.example` updated if new environment variables were added
- [ ] `CLAUDE.md` / `AGENTS.md` updated if architecture or standards changed
- [ ] All CI checks are green (do not merge with red CI)
