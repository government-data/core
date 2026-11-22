# The DuckDB worker's image (`qd-worker`, decision 0018): stateless, on a
# distroless, non-root base, as the API's (api.Dockerfile). It carries the
# node agent too (`qd-node`, decision 0066), which infra/worker runs as its
# own service from this image: the same user, so the worker reads the key
# the agent keeps 0600 in /var/lib/qd-node.
#
# Build context is the repo root:
#   docker build -f infra/docker/worker.Dockerfile -t qd-worker .
#
# DuckDB's extensions, the lake's (`iceberg`, the `avro` it loads, `httpfs`)
# and the sources' drivers' (`postgres`, `mysql`, and the community's
# `mssql` and `oracle_scanner`), are put here, at build time, into the
# image's extension directory: a job loads them offline and fetches nothing
# (its root filesystem is read-only, so it could not), and after it attaches
# the lake its connection cannot install anything anyway (the lockdown of
# decision 0033). The lake's are installed by `qd-worker
# --install-extensions`. The source drivers are the reviewed files this
# repository hosts as assets of its release `duckdb-extensions-v2.0.0`
# (decision 0077 point 8), each pinned by its SHA-256 at the engine the
# `duckdb` crate bundles (v2.0.0) on linux_amd64: the build fails if one
# differs. A community extension comes with the `.info` file its repository
# writes on install, without which DuckDB refuses its signature. A new
# version of an extension, or of DuckDB, is reviewed before its hash
# changes here (`mssql` v0.3.0, hugr-lab, MIT; `oracle_scanner` 0.8.1,
# krokozyab, Apache-2.0; `postgres_scanner` and `mysql_scanner`, DuckDB's,
# MIT).

FROM rust:1.90-bookworm AS chef
RUN cargo install cargo-chef --locked
WORKDIR /app

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json -p worker -p node-agent
COPY . .
RUN cargo build --release --locked -p worker -p node-agent
# Into the home of distroless's `nonroot` (uid 65532), where DuckDB looks.
RUN mkdir -p /out/home && HOME=/out/home target/release/qd-worker --install-extensions
ARG DUCKDB_EXTENSIONS=https://github.com/quality-data/core/releases/download/duckdb-extensions-v2.0.0
RUN mkdir -p /out/home/.duckdb/extensions/v2.0.0/linux_amd64 \
 && cd /out/home/.duckdb/extensions/v2.0.0/linux_amd64 \
 && printf '%s  %s\n' \
      39e661c5cfb678c9472f5acd88235f84f3ef4126e7444c7c8a01214d9b291601 postgres_scanner.duckdb_extension \
      6055f1a59dc527df795afccbf59279500d1ddc239608dd310c3eba0e98d1a11c mysql_scanner.duckdb_extension \
      473b2980d494df2a0653ec9a8a82588ae102a9ab25d5a2b117a6e9fcd5fc20c5 mssql.duckdb_extension \
      e319c9d709708142b7ed78490cb8cb90c83c2ced0ff9debb6fd5a61acf02c7a4 mssql.duckdb_extension.info \
      20fe6e698f29be6d01435422c04115adb0dbdee073e003432a159d4665be3dc3 oracle_scanner.duckdb_extension \
      9a83aef31991a2462527f3f1e8a2d5f63de6012aa7bb8285b20244d041133621 oracle_scanner.duckdb_extension.info \
    > /tmp/extensions.sha256 \
 && while read -r _ f; do \
      curl --fail --silent --show-error --location --proto '=https' --retry 3 -o "$f" "$DUCKDB_EXTENSIONS/$f" || exit 1; \
    done < /tmp/extensions.sha256 \
 && sha256sum --check --strict /tmp/extensions.sha256
# The agent's directory, the user's: a volume mounted there takes it.
RUN mkdir -m 0700 -p /out/qd-node
# The metrics files' folder (decision 0071), as /tmp: the worker and the
# agent (uid 65532) and a lake's agent (RustFS's 10001) each write their own,
# readable by all; a new volume mounted there takes the mode.
RUN mkdir -p /out/qd-metrics && chmod 1777 /out/qd-metrics

FROM gcr.io/distroless/cc-debian12:nonroot AS runtime
COPY --from=builder /app/target/release/qd-worker /usr/local/bin/qd-worker
COPY --from=builder /app/target/release/qd-node /usr/local/bin/qd-node
COPY --from=builder --chown=nonroot:nonroot /out/qd-node /var/lib/qd-node
COPY --from=builder --chown=nonroot:nonroot /out/qd-metrics /run/qd-metrics
COPY --from=builder --chown=nonroot:nonroot /out/home/.duckdb /home/nonroot/.duckdb
# Where the worker loads the sources' extensions from (`QD_DUCKDB_EXTENSIONS`).
ENV QD_DUCKDB_EXTENSIONS=/home/nonroot/.duckdb/extensions
USER nonroot
ENTRYPOINT ["/usr/local/bin/qd-worker"]
