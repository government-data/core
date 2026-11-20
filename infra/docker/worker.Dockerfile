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
# and the sources' drivers' (`postgres`, `mysql`), are
# installed here, at build time, into the image's extension directory: a
# job loads them and fetches nothing (its root filesystem is read-only, so
# it could not), and after it attaches the lake its connection cannot
# install anything anyway (the lockdown of decision 0033).

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
USER nonroot
ENTRYPOINT ["/usr/local/bin/qd-worker"]
