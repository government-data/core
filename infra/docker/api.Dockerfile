# Backend image (bead authapp-3cb595): a multi-stage build that compiles the
# `server` binary and ships it on a distroless, non-root base.
#
# Build context is the repo root (see infra/traefik/docker-compose.yml):
#   docker build -f infra/docker/api.Dockerfile -t data-quality-api .
#
# `cargo-chef` splits dependency compilation from the application build so an
# edit that touches only our crates reuses the cached dependency layer — the
# slow part — and rebuilds incrementally.
#
# The runtime is `gcr.io/distroless/cc-debian12:nonroot`: glibc + libgcc and
# nothing else (no shell, no package manager), running as an unprivileged user.
# We link TLS through rustls/ring (see the workspace Cargo.toml), so no OpenSSL
# is needed at runtime, and sqlx embeds the migrations at compile time
# (`sqlx::migrate!`), so the binary is fully self-contained.

FROM rust:1.90-bookworm AS chef
RUN cargo install cargo-chef --locked
WORKDIR /app

# Plan: capture the dependency graph as a recipe, so the cook stage below is
# invalidated only when dependencies actually change.
FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

# Build: cook the dependencies from the recipe (cached across app-only edits),
# then compile just the server binary.
FROM chef AS builder
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json
COPY . .
# The module's version and commit (domain::VERSION and domain::COMMIT, read
# at compile time): CI passes version.txt's and the commit's. Without them,
# the crate's version and no commit.
ARG APP_VERSION=""
ARG APP_COMMIT=""
RUN if [ -n "$APP_VERSION" ]; then export APP_VERSION; else unset APP_VERSION; fi; \
    if [ -n "$APP_COMMIT" ]; then export APP_COMMIT; else unset APP_COMMIT; fi; \
    cargo build --release --locked -p server
# The folder of the key that encrypts the Settings secrets: the final image
# has no shell to create it, and a new volume inherits its owner. The same
# for the metrics file's folder (decision 0071), shared with Alloy.
RUN mkdir -p /out/settings /out/qd-metrics

FROM gcr.io/distroless/cc-debian12:nonroot AS runtime
WORKDIR /app
COPY --from=builder /app/target/release/server /usr/local/bin/server
# The key that encrypts the Settings secrets (SETTINGS_KEY_FILE).
COPY --from=builder --chown=nonroot:nonroot /out/settings /var/lib/orquestador/settings
COPY --from=builder --chown=nonroot:nonroot /out/qd-metrics /run/qd-metrics
# The app binds APP_HOST:APP_PORT; the compose files set 0.0.0.0:8080.
EXPOSE 8080
# distroless:nonroot already runs as an unprivileged user (uid 65532); make it
# explicit for readers and for a read-only root filesystem at deploy time.
USER nonroot
ENTRYPOINT ["/usr/local/bin/server"]
CMD ["serve"]
