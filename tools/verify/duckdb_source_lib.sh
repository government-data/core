# shellcheck shell=bash
# The helpers of tools/verify/duckdb_source_*.sh (decision 0077 step 3),
# which source this file after setting SRC (a short name: mssql, oracle,
# openedge). Not run by itself.
#
# DuckDB runs at the engine the core pins: the `duckdb` crate in Cargo.lock,
# whose bundled engine (libduckdb-sys's duckdb.tar.gz in the cargo registry)
# names its version; the Python package of that same engine runs in a
# throwaway container on the script's own network. Every container and
# network is named qs-verify-<SRC>…, removed on exit, also on failure.
export LC_ALL=C
cd "$(dirname "${BASH_SOURCE[0]}")/../.." || exit 1
NET=qs-verify-$SRC
DUCK=qs-verify-$SRC-duckdb
PY_IMAGE=${PY_IMAGE:-python:3.12-slim}
FAILED=0
CONTAINERS=("$DUCK")
# The extension directory of the offline checks (ext_install, ext_offline).
EXTVOL=qs-verify-$SRC-ext
# The reviewed extensions the worker's image takes (decision 0077 point 8),
# self-hosted for DuckDB v2.0.0 on linux_amd64.
RELEASE=https://github.com/quality-data/core/releases/download/duckdb-extensions-v2.0.0

ok() { printf '[OK  ] %s\n' "$*"; }
ko() { printf '[FAIL] %s\n' "$*"; FAILED=1; }
cleanup() {
  docker rm -fv "${CONTAINERS[@]}" >/dev/null 2>&1
  docker network rm "$NET" >/dev/null 2>&1
  docker volume rm "$EXTVOL" >/dev/null 2>&1
  true
}
trap cleanup EXIT

# pinned_engine: sets CRATE (the duckdb crate's version in Cargo.lock) and
# ENGINE (its bundled engine, e.g. v2.0.0), or fails.
pinned_engine() {
  CRATE=$(awk '/^name = "duckdb"$/ { getline; gsub(/version = |"/, ""); print; exit }' Cargo.lock)
  local tarball='' f
  for f in "${CARGO_HOME:-$HOME/.cargo}"/registry/src/*/libduckdb-sys-"$CRATE"/duckdb.tar.gz \
    "$HOME"/.cargo/registry/src/*/libduckdb-sys-"$CRATE"/duckdb.tar.gz; do
    [ -f "$f" ] && { tarball=$f; break; }
  done
  if [ -n "${DUCKDB_ENGINE:-}" ]; then
    ENGINE=$DUCKDB_ENGINE
  elif [ -n "$tarball" ]; then
    ENGINE=$(tar xzOf "$tarball" duckdb/src/function/table/version/pragma_version.cpp |
      sed -n 's/^#define DUCKDB_VERSION "\(v[^"]*\)"/\1/p')
  fi
  [ -n "${CRATE:-}" ] && [ -n "${ENGINE:-}" ] ||
    { ko "the pinned DuckDB: crate '${CRATE:-}' engine '${ENGINE:-}' (fetch the crate, or set DUCKDB_ENGINE)"; exit 1; }
  ok "DuckDB pinned: crate duckdb $CRATE (Cargo.toml, Cargo.lock), bundled engine $ENGINE"
}

# ext_pins EXT: the release's files of a source extension and their SHA-256,
# as "file sha256" pairs separated by ";" (the same as the worker image's);
# a community extension takes the `.info` its repository writes, without
# which DuckDB refuses its signature.
ext_pins() {
  case $1 in
    mssql) echo "mssql.duckdb_extension 473b2980d494df2a0653ec9a8a82588ae102a9ab25d5a2b117a6e9fcd5fc20c5;mssql.duckdb_extension.info e319c9d709708142b7ed78490cb8cb90c83c2ced0ff9debb6fd5a61acf02c7a4" ;;
    oracle_scanner) echo "oracle_scanner.duckdb_extension 20fe6e698f29be6d01435422c04115adb0dbdee073e003432a159d4665be3dc3;oracle_scanner.duckdb_extension.info 9a83aef31991a2462527f3f1e8a2d5f63de6012aa7bb8285b20244d041133621" ;;
    postgres) echo "postgres_scanner.duckdb_extension 39e661c5cfb678c9472f5acd88235f84f3ef4126e7444c7c8a01214d9b291601" ;;
    mysql) echo "mysql_scanner.duckdb_extension 6055f1a59dc527df795afccbf59279500d1ddc239608dd310c3eba0e98d1a11c" ;;
    *) ko "no pinned files for the extension $1"; exit 1 ;;
  esac
}

# Python that downloads PINS from RELEASE into <dir>/<ENGINE>/linux_amd64
# with `fetch(dir)`, failing on a SHA-256 that differs.
read -r -d '' FETCH <<'PY'
import hashlib, os, urllib.request
def fetch(top):
    d = os.path.join(top, os.environ["ENGINE"], "linux_amd64")
    os.makedirs(d, exist_ok=True)
    for pin in os.environ["PINS"].split(";"):
        name, want = pin.split()
        data = urllib.request.urlopen(f"{os.environ['RELEASE']}/{name}").read()
        got = hashlib.sha256(data).hexdigest()
        if got != want:
            raise SystemExit(f"[FAIL] {name}: sha256 {got}, pinned {want}")
        open(os.path.join(d, name), "wb").write(data)
        print(f"[OK  ] {name} from the release, sha256 {got}", flush=True)
PY

net_up() {
  cleanup
  docker network create "$NET" >/dev/null || { ko "network $NET"; exit 1; }
}

# The checks DuckDB runs, as Python on the duckdb package. Everything comes
# from the environment: ENGINE, EXT, PINS (with RELEASE; else REPO, to
# install from DuckDB's repository), SECRET_SQL (with @PASSWORD@ for
# SRC_PASSWORD, never printed), ATTACH_SQL, SCHEMA, TABLE, TYPES (JSON:
# column -> accepted DuckDB types).
read -r -d '' CHECK <<'PY'
import datetime as dt, json, os, sys
from decimal import Decimal
import duckdb
@FETCH@
failed = False
def ok(m): print(f"[OK  ] {m}", flush=True)
def ko(m):
    global failed; failed = True; print(f"[FAIL] {m}", flush=True)
def expect(name, want, got):
    ok(f"{name}: {got!r}") if want == got else ko(f"{name}: expected {want!r}, got {got!r}")
env = os.environ
con = duckdb.connect()
con.execute("SET enable_progress_bar = false")
version, source_id = con.execute("SELECT library_version, source_id FROM pragma_version()").fetchone()
expect("DuckDB engine in the container", env["ENGINE"], version)
ok(f"source id {source_id}, platform {con.execute('PRAGMA platform').fetchone()[0]}")
ext = env["EXT"]
try:
    if env.get("PINS"):
        fetch("/tmp/ext")
        con.execute("SET extension_directory = '/tmp/ext'")
    else:
        con.execute(f"INSTALL {ext} FROM {env['REPO']}")
    con.execute(f"LOAD {ext}")
except Exception as e:
    ko(f"install {ext}; LOAD {ext}: {e}"); sys.exit(1)
ok("extension %s %s, installed from %s (%s)" % ((ext,) + con.execute(
    "SELECT extension_version, installed_from, install_mode FROM duckdb_extensions() WHERE extension_name = ?",
    [ext]).fetchone()))
if env.get("ONLY_LOAD"):
    sys.exit(1 if failed else 0)
password = env["SRC_PASSWORD"].replace("'", "''")
try:
    con.execute(env["SECRET_SQL"].replace("@PASSWORD@", password))
    con.execute(env["ATTACH_SQL"])
    ok(f"attached: {env['ATTACH_SQL']}")
except Exception as e:
    ko(f"attach: {str(e).replace(env['SRC_PASSWORD'], '***')}"); sys.exit(1)
schema, t = env["SCHEMA"], env["TABLE"]
tables = [r[0].lower() for r in con.execute(
    "SELECT table_name FROM duckdb_tables() WHERE database_name = 'src' AND schema_name = ? ORDER BY 1", [schema]).fetchall()]
ok(f"tables of src.{schema}: {tables}") if "clients" in tables else ko(f"tables of src.{schema}: {tables}")
seen = {c[0].lower(): c[1] for c in con.execute(f"DESCRIBE {t}").fetchall()}
ok("columns as the source is seen: " + ", ".join(f"{k} {v}" for k, v in seen.items()))
# What an extraction keeps: the table copied into DuckDB, then written to
# Parquet (what bronze's files are) and read back, with their types.
con.execute(f"CREATE TEMP TABLE copied AS SELECT * FROM {t}")
copied = {c[0].lower(): c[1] for c in con.execute("DESCRIBE copied").fetchall()}
ok("columns copied into DuckDB: " + ", ".join(f"{k} {v}" for k, v in copied.items()))
con.execute("COPY copied TO '/tmp/copied.parquet' (FORMAT parquet)")
types = {c[0].lower(): c[1] for c in con.execute("DESCRIBE SELECT * FROM '/tmp/copied.parquet'").fetchall()}
for col, accepted in json.loads(env["TYPES"]).items():
    got = types.get(col)
    ok(f"type of {col} in Parquet: {got}") if got in accepted else ko(f"type of {col} in Parquet: {got}, expected one of {accepted}")
expect("rows in Parquet", 4, con.execute("SELECT count(*) FROM '/tmp/copied.parquet'").fetchone()[0])
def day(v):  # an Oracle DATE reads as a TIMESTAMP at midnight
    return v.date() if isinstance(v, dt.datetime) and v.time() == dt.time(0) else v
q = lambda sql, *p: con.execute(sql, list(p)).fetchall()
expect("row count", 4, q(f"SELECT count(*) FROM {t}")[0][0])
rows = [tuple(map(day, r)) for r in q(f"SELECT id, code, name, opened, amount, updated FROM {t} WHERE amount > 100 AND opened >= DATE '2021-01-01'")]
expect("filter on a decimal and a date", [(4, "300", "Ñandú Señorial", dt.date(2024, 2, 29), Decimal("987654321.99"), dt.datetime(2026, 11, 20, 23, 59, 59))], rows)
expect("filter on text with accents", [(3,)], q(f"SELECT id FROM {t} WHERE name = ?", "Árbol Único"))
expect("LIKE with an accent", [(2,)], q(f"SELECT id FROM {t} WHERE name LIKE '%ü%'"))
expect("accents read back", ["Café Añejo", "Pingüino Azul", "Árbol Único", "Ñandú Señorial"], [r[0] for r in q(f"SELECT name FROM {t} ORDER BY id")])
expect("exact decimal sum", Decimal("987655812.24"), q(f"SELECT sum(amount) FROM {t}")[0][0])
expect("negative decimal", Decimal("-10.25"), q(f"SELECT amount FROM {t} WHERE id = 3")[0][0])
expect("a code keeps its leading zeros", "007", q(f"SELECT code FROM {t} WHERE id = 1")[0][0])
expect("NULLs stay NULL", [(2, None), (3, None)], q(f"SELECT id, CASE WHEN id = 2 THEN opened ELSE updated END FROM {t} WHERE id IN (2, 3) ORDER BY id"))
expect("a page, last first", [(4,), (3,)], q(f"SELECT id FROM {t} ORDER BY id DESC LIMIT 2 OFFSET 0"))
expect("the next page", [(2,), (1,)], q(f"SELECT id FROM {t} ORDER BY id DESC LIMIT 2 OFFSET 2"))
try:
    con.execute(f"INSERT INTO {t} (id) VALUES (9)")
    ko("a write through the read-only attach was taken")
except Exception as e:
    ok(f"a write is refused: {str(e).splitlines()[0][:120]}")
sys.exit(1 if failed else 0)
PY
CHECK=${CHECK/@FETCH@/"$FETCH"}

# duckdb_check: runs CHECK in the DuckDB container; extra `-e NAME` flags for
# the variables above come as arguments (PREP: a command run first, ONLY_LOAD:
# stop once the extension loads). The engine's Python package is the version
# without its `v`.
duckdb_check() {
  docker run --rm --name "$DUCK" --network "$NET" -e ENGINE="$ENGINE" -e RELEASE="$RELEASE" -e CHECK="$CHECK" "$@" "$PY_IMAGE" \
    sh -c 'eval "${PREP:-true}" && pip install --quiet --disable-pip-version-check --root-user-action=ignore "duckdb==${ENGINE#v}" >/dev/null 2>&1 &&
      python -I -c "$CHECK"' || FAILED=1
}

# ext_install EXT: what the worker's image does at build time (decision
# 0077 point 8), into the volume EXTVOL: the engine's Python package under
# /ext/py and the extension's pinned files from the release under
# /ext/duckdb, the extension directory; prints each file and its SHA-256.
ext_install() {
  docker run --rm --name "$DUCK" -v "$EXTVOL:/ext" -e ENGINE="$ENGINE" -e RELEASE="$RELEASE" \
    -e PINS="$(ext_pins "$1")" -e CHECK="$FETCH
fetch('/ext/duckdb')" "$PY_IMAGE" sh -c 'pip install --quiet --disable-pip-version-check \
      --root-user-action=ignore --target /ext/py "duckdb==${ENGINE#v}" >/dev/null 2>&1 && python -c "$CHECK"' ||
    ko "install $1 into the extension directory"
}

# The start of a check run offline: the package and the extension from the
# volume, nothing installed or loaded by itself.
read -r -d '' OFFLINE <<'PY'
import sys; sys.path.insert(0, "/ext/py")
import os, duckdb
def ok(m): print(f"[OK  ] {m}", flush=True)
def ko(m): print(f"[FAIL] {m}", flush=True); sys.exit(1)
con = duckdb.connect()
con.execute("SET enable_progress_bar = false")
for s in ("extension_directory = '/ext/duckdb'", "autoinstall_known_extensions = false", "autoload_known_extensions = false"):
    con.execute(f"SET {s}")
ext = os.environ["EXT"]
try:
    con.execute(f"LOAD {ext}")
except Exception as e:
    ko(f"LOAD {ext} from the extension directory: {e}")
ok("LOAD %s from the extension directory: %s %s, installed from %s" % ((ext,) + con.execute(
    "SELECT extension_version, install_mode, installed_from FROM duckdb_extensions() WHERE extension_name = ?",
    [ext]).fetchone()))
PY

# ext_offline EXT: loads the extension from the volume with no network at
# all, as the worker does at run time.
ext_offline() {
  docker run --rm --name "$DUCK" --network none -v "$EXTVOL:/ext:ro" -e EXT="$1" -e CHECK="$OFFLINE" "$PY_IMAGE" \
    sh -c 'python -c "$CHECK"' || FAILED=1
}

# ext_check EXT: OFFLINE then the Python in CHECK2, on the script's network,
# with the extension from the volume; extra `-e` flags as arguments.
ext_check() {
  local ext=$1; shift
  docker run --rm --name "$DUCK" --network "$NET" -v "$EXTVOL:/ext:ro" -e EXT="$ext" -e CHECK="$OFFLINE
$CHECK2" "$@" "$PY_IMAGE" sh -c 'python -c "$CHECK"' || FAILED=1
}

verdict() { # SOURCE
  if [ "$FAILED" = 0 ]; then echo "== PASS: DuckDB $ENGINE reads $1"; else echo "== FAIL: DuckDB $ENGINE does not read $1 as checked"; fi
  exit "$FAILED"
}
