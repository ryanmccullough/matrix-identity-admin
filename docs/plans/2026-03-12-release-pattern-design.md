# Release Pattern Design

## Goal

Establish a tag-triggered release pipeline that builds multi-platform Docker images and publishes them to GHCR and Docker Hub, with a GitHub Release for each version.

## Architecture

Manual semver tagging from `main`. Push a `v*` tag, CI validates, builds, publishes. No release branches — tags are cut from main. Cargo.toml version stays in sync with the git tag (manually updated before tagging, CI validates the match).

## Components

### 1. Release workflow (`.github/workflows/release.yml`)

Triggers on `v*` tag push. Steps:

1. Run existing CI checks (fmt, clippy, test)
2. Validate git tag matches `version` in `Cargo.toml`
3. Build Docker image for `linux/amd64` + `linux/arm64` using `docker/build-push-action`
4. Push to both registries:
   - `ghcr.io/ryanmccullough/matrix-identity-admin`
   - `docker.io/ryanmccullough/matrix-identity-admin`
5. Tag images: `v0.1.0` (exact), `v0.1` (minor track), `latest`
6. Create GitHub Release with auto-generated changelog
7. Attach SHA256 checksums file

### 2. Dockerfile updates

Add OCI image labels (`org.opencontainers.image.*`) populated from CI build args:
- `source`, `version`, `description`, `created`, `revision`

### 3. Registry setup (manual, one-time)

- **Docker Hub**: Create `ryanmccullough/matrix-identity-admin` repo. Add `DOCKERHUB_USERNAME` and `DOCKERHUB_TOKEN` as GitHub repo secrets.
- **GHCR**: Automatic — `GITHUB_TOKEN` has push access for public repos.

## Release flow

```
Developer                    GitHub Actions
────────                     ──────────────
1. Update Cargo.toml
   version = "0.2.0"
2. Commit: "build: bump
   version to 0.2.0"
3. git tag v0.2.0
4. git push && git push
   --tags
                             5. CI runs (fmt, clippy, test)
                             6. Validates tag == Cargo.toml version
                             7. Builds linux/amd64 + linux/arm64
                             8. Pushes to GHCR + Docker Hub
                                - :v0.2.0, :v0.2, :latest
                             9. Creates GitHub Release
                                - Auto-generated changelog
                                - SHA256 checksums
```

## Out of scope

- Release branches (not needed for single-maintainer; add later if supporting multiple release lines)
- Binary artifacts / Homebrew / cargo-install (Docker is the distribution format)
- Automated changelog tools like `release-please` (auto-generated from commits is sufficient)
- Image signing (future consideration)
- Cross-compiled binary uploads (Rust cross-compilation is painful; Docker handles multi-platform)
