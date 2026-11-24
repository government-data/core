# The lake admin adapter's image (`qd-lake-rustfs-admin`, decision 0078,
# step 13): `lake-rustfs-admin` serving lake-admin/1 with mTLS, on a
# distroless, non-root base, as the API's (api.Dockerfile). Built here for
# the dev stack and CI only, while the adapter lives in the core's crates;
# its image reaches ECR from the `lake-rustfs` repository (step 15).
#
# Build context is the repo root:
#   docker build -f infra/docker/lake-rustfs-admin.Dockerfile -t qd-lake-rustfs-admin .
#
# It keeps no state or secret (0078 point 9): its only files are the node
# agent's `ca.crt` and `machine.pem`, mounted read-only at
# QD_LAKE_ADMIN_TLS_DIR (/run/qd-node), as the lake's other services read
# them. It listens on QD_LAKE_ADMIN_ADDR (0.0.0.0:9020).

FROM rust:1.90-bookworm AS chef
RUN cargo install cargo-chef --locked
WORKDIR /app

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json -p lake-rustfs-admin
COPY . .
RUN cargo build --release --locked -p lake-rustfs-admin

FROM gcr.io/distroless/cc-debian12:nonroot AS runtime
COPY --from=builder /app/target/release/lake-rustfs-admin /usr/local/bin/lake-rustfs-admin
USER nonroot
EXPOSE 9020
ENTRYPOINT ["/usr/local/bin/lake-rustfs-admin"]
