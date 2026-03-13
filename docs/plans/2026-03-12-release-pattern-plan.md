# Release Pattern Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Tag-triggered CI pipeline that builds multi-platform Docker images, publishes to GHCR + Docker Hub, and creates a GitHub Release.

**Architecture:** Push a `v*` tag → GitHub Actions validates tag matches Cargo.toml version → builds linux/amd64 + linux/arm64 Docker images → pushes to both registries with semver tags → creates GitHub Release with auto-generated changelog.

**Tech Stack:** GitHub Actions, `docker/build-push-action`, `docker/metadata-action`, GHCR, Docker Hub

---

### Task 1: Add OCI labels to Dockerfile

**Files:**
- Modify: `Dockerfile`

**Step 1: Add build args and labels to the runtime stage**

Add `ARG` and `LABEL` directives to `Dockerfile`. The build args allow CI to inject version/revision at build time, with sensible defaults for local builds.

Replace the runtime stage section (after `FROM debian:bookworm-slim`) with:

```dockerfile
# ── Runtime stage ─────────────────────────────────────────────────────────────
FROM debian:bookworm-slim

ARG VERSION=dev
ARG REVISION=unknown
ARG CREATED=unknown

LABEL org.opencontainers.image.title="matrix-identity-admin" \
      org.opencontainers.image.description="Identity and lifecycle control plane for self-hosted Matrix infrastructure" \
      org.opencontainers.image.source="https://github.com/ryanmccullough/matrix-identity-admin" \
      org.opencontainers.image.version="${VERSION}" \
      org.opencontainers.image.revision="${REVISION}" \
      org.opencontainers.image.created="${CREATED}" \
      org.opencontainers.image.licenses="MIT"

RUN apt-get update \
    && apt-get install -y ca-certificates curl \
    && rm -rf /var/lib/apt/lists/*
```

Keep everything else in the runtime stage unchanged.

**Step 2: Verify local build still works**

Run:
```bash
docker build -t mia-test .
```
Expected: Build succeeds. Labels have default values.

Verify labels:
```bash
docker inspect mia-test --format '{{json .Config.Labels}}' | python3 -m json.tool
```
Expected: Shows `org.opencontainers.image.version: "dev"`, etc.

**Step 3: Commit**

```bash
git add Dockerfile
git commit -m "build: add OCI image labels to Dockerfile"
```

---

### Task 2: Create the release workflow

**Files:**
- Create: `.github/workflows/release.yml`

**Step 1: Create the workflow file**

```yaml
name: Release

on:
  push:
    tags: ["v*"]

permissions:
  contents: write
  packages: write

env:
  CARGO_TERM_COLOR: always
  RUST_BACKTRACE: 1

jobs:
  ci:
    name: Check, lint, test
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v6

      - name: Install Rust stable
        uses: dtolnay/rust-toolchain@stable
        with:
          components: rustfmt, clippy

      - name: Cache cargo
        uses: actions/cache@v5
        with:
          path: |
            ~/.cargo/registry
            ~/.cargo/git
            target
          key: ${{ runner.os }}-cargo-${{ hashFiles('**/Cargo.lock') }}
          restore-keys: |
            ${{ runner.os }}-cargo-

      - name: cargo fmt --check
        run: cargo fmt --all -- --check

      - name: cargo check
        run: cargo check --all-targets

      - name: cargo clippy
        run: cargo clippy --all-targets -- -D warnings

      - name: cargo test
        run: cargo test

  validate-version:
    name: Validate tag matches Cargo.toml
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v6

      - name: Check version match
        run: |
          TAG_VERSION="${GITHUB_REF_NAME#v}"
          CARGO_VERSION=$(grep '^version' Cargo.toml | head -1 | sed 's/.*"\(.*\)".*/\1/')
          echo "Tag version: $TAG_VERSION"
          echo "Cargo.toml version: $CARGO_VERSION"
          if [ "$TAG_VERSION" != "$CARGO_VERSION" ]; then
            echo "::error::Tag $GITHUB_REF_NAME does not match Cargo.toml version $CARGO_VERSION"
            exit 1
          fi

  release:
    name: Build and publish
    needs: [ci, validate-version]
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v6

      - name: Set up QEMU
        uses: docker/setup-qemu-action@v3

      - name: Set up Docker Buildx
        uses: docker/setup-buildx-action@v3

      - name: Log in to GHCR
        uses: docker/login-action@v3
        with:
          registry: ghcr.io
          username: ${{ github.actor }}
          password: ${{ secrets.GITHUB_TOKEN }}

      - name: Log in to Docker Hub
        uses: docker/login-action@v3
        with:
          username: ${{ secrets.DOCKERHUB_USERNAME }}
          password: ${{ secrets.DOCKERHUB_TOKEN }}

      - name: Extract metadata for Docker
        id: meta
        uses: docker/metadata-action@v5
        with:
          images: |
            ghcr.io/ryanmccullough/matrix-identity-admin
            docker.io/ryanmccullough/matrix-identity-admin
          tags: |
            type=semver,pattern={{version}}
            type=semver,pattern={{major}}.{{minor}}
            type=raw,value=latest

      - name: Build and push
        uses: docker/build-push-action@v6
        with:
          context: .
          platforms: linux/amd64,linux/arm64
          push: true
          tags: ${{ steps.meta.outputs.tags }}
          labels: ${{ steps.meta.outputs.labels }}
          build-args: |
            VERSION=${{ github.ref_name }}
            REVISION=${{ github.sha }}
            CREATED=${{ github.event.head_commit.timestamp }}
          cache-from: type=gha
          cache-to: type=gha,mode=max

      - name: Create GitHub Release
        uses: softprops/action-gh-release@v2
        with:
          generate_release_notes: true
          draft: false
          prerelease: ${{ contains(github.ref_name, '-') }}
```

**Step 2: Verify YAML syntax**

Run:
```bash
python3 -c "import yaml; yaml.safe_load(open('.github/workflows/release.yml'))"
```
Expected: No output (valid YAML). If `yaml` module not available, use:
```bash
cat .github/workflows/release.yml | python3 -c "import sys,json; __import__('yaml').safe_load(sys.stdin)" 2>/dev/null || echo "Install pyyaml or verify manually"
```

**Step 3: Commit**

```bash
git add .github/workflows/release.yml
git commit -m "ci: add tag-triggered release workflow"
```

---

### Task 3: Update docker-compose.yml to reference published image

**Files:**
- Modify: `docker-compose.yml`

**Step 1: Add image reference alongside build**

Update `docker-compose.yml` so users who pull the published image don't need to build locally. Add a commented `image:` line and keep `build:` for local development:

```yaml
services:
  app:
    image: ghcr.io/ryanmccullough/matrix-identity-admin:latest
    # build: .  # Uncomment to build from source instead of pulling
    ports:
      - "3000:3000"
    volumes:
      - app-data:/app/data
    env_file:
      - .env
    environment:
      APP_BIND_ADDR: "0.0.0.0:3000"
      DATABASE_URL: "sqlite:///app/data/app.db"
    restart: unless-stopped
    healthcheck:
      test: ["CMD", "curl", "-sf", "http://localhost:3000/auth/login"]
      interval: 30s
      timeout: 5s
      retries: 3
      start_period: 10s

volumes:
  app-data:
```

**Step 2: Verify compose file is valid**

Run:
```bash
docker compose config --quiet
```
Expected: No errors.

**Step 3: Commit**

```bash
git add docker-compose.yml
git commit -m "build: reference published image in docker-compose"
```

---

### Task 4: Manual registry setup and dry-run

This task is manual — no code changes.

**Step 1: Create Docker Hub repository**

1. Go to https://hub.docker.com
2. Create repository: `ryanmccullough/matrix-identity-admin`
3. Set visibility to Public

**Step 2: Add GitHub repo secrets**

1. Go to https://github.com/ryanmccullough/matrix-identity-admin/settings/secrets/actions
2. Add `DOCKERHUB_USERNAME` — your Docker Hub username
3. Add `DOCKERHUB_TOKEN` — a Docker Hub access token (create at https://hub.docker.com/settings/security → New Access Token, scope: Read & Write)

**Step 3: Verify GHCR package visibility**

1. Go to https://github.com/ryanmccullough/matrix-identity-admin/settings → Packages
2. Ensure "Inherit access from source repository" is enabled (default for public repos)

**Step 4: Dry-run the release**

```bash
# Ensure Cargo.toml still says version = "0.1.0"
grep '^version' Cargo.toml

# Tag and push
git tag v0.1.0
git push --tags
```

**Step 5: Verify the release**

1. Go to https://github.com/ryanmccullough/matrix-identity-admin/actions — watch the Release workflow
2. Check that all three jobs pass: ci, validate-version, release
3. Verify images exist:
   - https://github.com/ryanmccullough/matrix-identity-admin/pkgs/container/matrix-identity-admin
   - https://hub.docker.com/r/ryanmccullough/matrix-identity-admin
4. Verify GitHub Release was created at https://github.com/ryanmccullough/matrix-identity-admin/releases
5. Test pulling the image:
```bash
docker pull ghcr.io/ryanmccullough/matrix-identity-admin:v0.1.0
docker inspect ghcr.io/ryanmccullough/matrix-identity-admin:v0.1.0 --format '{{json .Config.Labels}}' | python3 -m json.tool
```
Expected: Labels show `version: "v0.1.0"`, correct source URL, etc.
