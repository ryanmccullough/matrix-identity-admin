# Code Coverage & Fuzzing Design

## Goal

Reach 80% code coverage for SOC 2 evidence and add property-based testing and continuous fuzzing to harden input validation boundaries.

## Architecture

Three tiers, each building on the previous:

### Tier 1 — Close coverage gaps (target: 80%)

Add unit tests for untested files, prioritized by risk.

**Model tests:**
- `keycloak.rs` — KeycloakUser/Group/Role serde roundtrip
- `mas.rs` — MasUser/MasSession serde roundtrip
- `synapse.rs` — RoomList, SynapseDevice serde roundtrip
- `policy_binding.rs` — PolicyBinding
- `audit.rs` — AuditResult Display

**Client tests:**
- `clients/keycloak.rs` — URL construction, error mapping, group/role parsing (currently 1 test)

**Handler tests:**
- `handlers/auth.rs` — callback error handling, logout CSRF (currently 3 tests)

**CI config:**
- Add `codecov.yml` with 80% project threshold, PR comments, patch coverage reporting

### Tier 2 — Property-based testing with proptest

Add `proptest` to `[dev-dependencies]`. Property tests live in existing test modules (not new files).

**Targets:**
- `is_valid_matrix_localpart` — valid localparts round-trip, uppercase/special chars rejected
- Email validation in invite — well-formed emails accepted, strings without `@` rejected
- `validate_date_format` — `YYYY-MM-DD` accepted, anything else rejected
- Audit filter validation — only allowlisted values pass
- Model serde — `KeycloakUser`, `MasUser`, `UnifiedUserSummary` survive serialize/deserialize roundtrip

### Tier 3 — Cargo-fuzz harnesses

Add `fuzz/` directory with libfuzzer targets.

**Targets:**
- `fuzz_matrix_localpart` — arbitrary byte strings into `is_valid_matrix_localpart`
- `fuzz_date_validation` — arbitrary strings into `validate_date_format`
- `fuzz_template_json` — arbitrary bytes into onboarding template JSON parser
- `fuzz_audit_filter_query` — arbitrary strings into audit filter validation

**CI:**
- New workflow job on push to main only (not PRs)
- Nightly toolchain, `cargo +nightly fuzz run <target> -- -max_total_time=60` per target

## Out of Scope

- Fuzzing HTTP handlers directly (proptest covers parsing layer)
- Testing `main.rs` or `state.rs` (startup wiring)
- Mocking `reqwest` for connector integration tests (e2e covers this)
- `test_helpers.rs` coverage (test infrastructure, not production code)
