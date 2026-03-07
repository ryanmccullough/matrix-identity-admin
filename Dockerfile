# ── Build stage ───────────────────────────────────────────────────────────────
FROM rust:1.84-slim AS builder

WORKDIR /app

# Install build dependencies
RUN apt-get update && apt-get install -y pkg-config libssl-dev && rm -rf /var/lib/apt/lists/*

# Cache dependencies by building with a dummy main first.
COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo 'fn main() {}' > src/main.rs
RUN cargo build --release
RUN rm -f target/release/deps/matrix_identity_admin*

# Build the real binary.
# migrations/ must be present here because sqlx::migrate! embeds them at compile time.
COPY src ./src
COPY templates ./templates
COPY migrations ./migrations
RUN cargo build --release

# ── Runtime stage ─────────────────────────────────────────────────────────────
FROM debian:bookworm-slim

RUN apt-get update \
    && apt-get install -y ca-certificates curl \
    && rm -rf /var/lib/apt/lists/*

# Run as a non-root user.
RUN groupadd --system app && useradd --system --gid app app

WORKDIR /app

COPY --from=builder /app/target/release/matrix-identity-admin ./matrix-identity-admin
COPY --from=builder /app/templates ./templates
COPY static ./static

# Create the data directory and hand it to the app user.
# migrations/ is NOT needed at runtime — sqlx::migrate! embeds them in the binary.
RUN mkdir -p data && chown app:app data

USER app

EXPOSE 3000

ENV APP_BIND_ADDR=0.0.0.0:3000
ENV RUST_LOG=info

CMD ["./matrix-identity-admin"]
