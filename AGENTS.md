# AGENTS.md

Instructions for coding agents working in this repository (`quality-data/core`,
public). Brought from the `calidad` repository on 2026-11-20.

## What this is

Quality Data core: the platform (identity and OIDC, IAM, audit, settings,
machines) and the data quality module (rules by pattern, indicators,
remediation queues), on Rust (API, workers) and SolidJS (web). It is the first
application installed and updated through `quality-sdk`.

The design lives in the **decisions** under `docs/decisions/`; the code map
and milestones in `docs/development/implementation.md`; what is done in
`docs/development/roadmap.md`. **`docs/` is internal: ignored by git, never
committed, pushed or published.** Ask for it if your checkout lacks it.

## Before you change anything

1. **Find the decision.** Every feature belongs to a decision. Read its
   **Decision** and its **Implementation** section: it names the crates,
   tables, routes, steps and the test that says it is done.
2. **Follow the decision; do not redesign it in code.** If the code needs
   something the decision does not say, or contradicts it, stop and say so.
   An accepted decision changes only through a new record; a **proposed** one
   can be edited.
3. **Verify external behaviour first.** Before building on what Iceberg,
   RustFS, Dolt, Trino or DuckDB do, write or run a script in `tools/verify/`
   and record the result in the decision (date, version, what was found).

## Architecture patterns

- **Layering** (enforced by the dependencies each crate opts into, see the
  root `Cargo.toml`): `api → application → domain`; `infrastructure`
  implements the ports; `server` is the composition root.
- **Ports and adapters.** Domain and use cases declare traits; SQL, HTTP,
  Dolt, Iceberg, RustFS and Trino live in adapters in `infrastructure` (or
  inside a module). No SQL or HTTP in a domain crate.
- **Feature modules through `contracts`.** The quality core is a module
  (`crates/quality`): its own migrations, its own router, registered by
  `server`. Modules never depend on one another. Its routes for people
  (`user_router`) get a `contracts::Caller` from `server`: they ask it what
  the caller may do and audit through it, and never see a session. They are
  described by the module's own `utoipa` annotations, returned from
  `Module::openapi`; `server openapi` merges them into `openapi.json`, so a
  module never depends on `api`.
- **Pure crates stay pure.** The rule language does no I/O: it takes YAML
  and returns lint findings, a tree, SQL and text. It is one crate per stage
  (`quality-rules-model` → `-tree` → `-compile`, then SQL, lint, explain),
  each depending only on the stages before it; `quality-rules` gathers them.
- **Rules reach an engine only through `quality-rules`.** Never write engine
  SQL for a rule by hand. The same tree gives DuckDB, Trino and the
  explanation; anything compiled for two engines has an equivalence test.
- **Rule semantics are fixed** (decision 0033): empty is `NULL`, blank or only
  spaces including U+00A0; other patterns skip empty values; regular
  expressions are anchored; dates compare with the cut (`:cut`), never today;
  results are three-valued (`NULL` does not apply, `TRUE` fails, `FALSE`
  passes).
- **Jobs are idempotent.** Every job can run twice: commits carry the job id
  and watermark, reports can be repeated.
- **Workers pull.** Nothing pushes to a worker; it claims jobs from the
  PostgreSQL queue over mTLS and holds no long-lived secret.

## Data rules

- **Tenants are being removed**: one installation is one organisation. Until
  the decision that removes them lands, every PostgreSQL table still has
  `tenant_id` with forced row-level security, Dolt one database per tenant
  and the lake one warehouse per tenant (`qd-tenant-<tenant>`). Add no new
  tenant logic beyond what a new table needs to work today.
- **Licenses are being removed**: add nothing that depends on a plan, a
  permit or the licensing cloud.
- **Migrations only add.** `cargo xtask migrations` rejects drops, renames,
  type changes and new required columns; those go later, marked
  `-- contract: vX.Y.Z no longer uses it`.
- **No personal data** in logs, notices, metrics, test fixtures, examples or
  documentation. Use synthetic rows. Values from sources are shown only
  behind `quality:ViewPersonalData`, and every view is audited.
- **Secrets** never go in the repository, a log or a commit. Configuration
  reads `*_FILE` for secrets; credentials are dynamic (decision 0030).
- **Never real cloud accounts** in development or tests: AWS is emulated by
  Floci (`task floci`).
- **RustFS is never used as root** after installation; use the platform's
  admin user and STS.

## Language

**English only. Nothing in Spanish**, with the exceptions at the end:

- Code, identifiers, file and directory names, commands and flags,
  configuration keys and paths.
- Comments, documentation, decisions, READMEs.
- Logs, commit messages, branch names, pull requests and issues.

**Exceptions: text for the people who use the product.** The interface's
translations (`crates/texts` on the server, `packages/i18n` on the web, with
English keys) and the user guide (the `docs` module, its own repository) are
in Spanish and English. No user-facing string literals in code.

Spanish left in code or comments from the original repository is translated
to English when its file is touched.

## Size limits

Decision 0034. Targets, then hard limits that fail CI:

- **Rust file** 400, hard 800 lines; **TypeScript file** 300, hard 600
  (`cargo xtask sizes`, ESLint `max-lines`).
- **Function** 60, hard 100 lines (Clippy `too_many_lines`, ESLint
  `max-lines-per-function`).
- **Test files** 800, no hard limit: split by use case. Generated files are
  exempt.
- **A change**: at most about 400 diff lines without generated files, one
  step of a decision's Implementation at a time.
- **Rules**: a rules file up to 30 rules or 300 lines, split by field group;
  a draft to approve up to 25 changed rules; large lists as CSV.

Above the target, split when you touch it. Never add an exception to
`crates/xtask/sizes-exceptions.txt` or a new `TODO(0034)` allow: split
instead.

## Checks before a commit

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo xtask migrations
cargo xtask sizes
pnpm run lint && pnpm run format:check && pnpm run test && pnpm run build
pnpm run gen:api && git diff --exit-code -- openapi.json packages/api-client/src/schema.ts
pnpm run check:grafana
```

A change to the API regenerates `openapi.json` and the client in the same
commit.

## Documentation with the code

- Update the decision's **Implementation** section and the **roadmap** state
  in the same change (they are local files under `docs/`, not part of the
  commit).
- A new decision follows the sections of the index: Context, Decision,
  Implementation, Consequences, Open or Verification.

## Do not

- Do not write engine SQL for rules, or a second evaluator of rules.
- Do not add a message broker, a second metrics store or a new service a
  decision does not name: the services' metrics go to Mimir through Alloy
  (0071), the machines' metrics to Mimir through Alloy (0072).
- Do not weaken a test, a lint or a check to make it pass.
- Do not copy data from a cooperative into the repository, a fixture or a
  prompt.
- Do not commit anything under `docs/`, nor copy or link a decision's content
  in code, comments or pull requests; name it by number ("decision 0033").

## Architecture

- all task always with subagents opus
- subagents always run with reasoning effort medium
- fable is orcherstator

## Builds on this machine

This machine is WSL with 16 GB and one disk. Builds saturate the disk's
reads and writes: on 2026-11-14 two sessions building at once, with a
four-node verification running, hung the disk and the machine restarted.
Every agent follows these rules, with no exception for "a quick check":

- **One build at a time, on the whole machine.** Before any `cargo` or
  `pnpm` command, check that no compiler is running (`pgrep -x rustc`,
  `pgrep -x cc1plus`) and wait if one is. Another session's build counts.
  Never use `pgrep -f`: it matches its own shell.
- **Every `cargo` and `pnpm` command goes through `~/.local/bin/capped`**,
  and `cargo` with `-j 2`.
- **Never `--workspace`.** Build, test and lint one crate at a time
  (`-p <crate>`), only the crates the change touches. Integration tests
  that start PostgreSQL run with `--test-threads=2`.
- **Never trigger a rebuild of DuckDB without need.** `libduckdb-sys`
  compiles from source for half an hour or more and is what loads the disk
  most. Do not run `cargo clean`, do not delete a `target` directory, do
  not change features, profiles or `RUSTFLAGS`, and do not create a new
  `target` directory (a new worktree or `CARGO_TARGET_DIR`) unless the
  user asked for it.
- **Do not edit Rust in the main checkout while the watch mode runs**
  (`task dev:watch`): every saved `.rs` restarts the API's build. Work in
  the worktree the user names, with its own `CARGO_TARGET_DIR`, and bring
  the commits over when they are done.
- **Verification scripts (`tools/verify/`) run alone**: never while a
  build runs, and one script at a time. Those that start several
  containers (a lake pool, replication) are the heaviest.
- **Clean up what you start.** Remove your `qd-verify-…` containers,
  networks and volumes when a script ends, also when it fails; give them
  no restart policy that outlives the script, or remove them in a trap.
  Leftovers start again by themselves after a restart of the machine.
- **If the machine slows down** (load above 8 in `uptime`, or a command
  that does not answer), stop your own build by its PID and tell the user;
  do not start anything else.
