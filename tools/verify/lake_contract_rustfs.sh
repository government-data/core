#!/bin/bash
# What RustFS answers on the lake's standard protocols (decision 0078, step
# 1), before the core is built on them: the Iceberg REST catalog, S3 and
# STS, asked with standard clients only (botocore's SigV4, boto3, DuckDB's
# `iceberg` extension), never RustFS's admin API but to set the scene.
#
#   (a) `GET /v1/config`: its `defaults` and `overrides` (a `prefix`, a
#       `uri`), with and without `?warehouse=`;
#   (b) `X-Iceberg-Access-Delegation: vended-credentials` on creating and
#       loading a table: whether the answer carries credentials (`config`
#       or `storage-credentials`; only their keys are printed);
#   (c) `DELETE …/tables/{t}?purgeRequested=true`: taken or refused, and
#       whether the table's files are still there after;
#   (d) `If-None-Match: *` on a second PUT of one object: refused (412) or
#       overwritten;
#   (e) STS `AssumeRole`: the shortest `DurationSeconds` taken, and which
#       `RoleArn` values it takes (any, empty, malformed);
#   (f) DuckDB's `iceberg` (at the engine the core pins) attaching with the
#       URI and prefix `/v1/config` gives, beside `<endpoint>/iceberg`.
#
# Throwaway containers only (qs-verify-lake-…, on a network of their own,
# no port published, removed with their volume and network on exit, also
# on failure), random root and issuer keys that are never printed, one
# synthetic table of invented numbers. Run alone (no build at the same
# time, one verification at a time), from the repository root:
#
#   bash tools/verify/lake_contract_rustfs.sh
#
# Needs docker and openssl. Exit 0 when every check ran; what RustFS
# answers is reported, not judged, except where the record expects one.
set -u
export LC_ALL=C
cd "$(dirname "$0")/../.." || exit 1
IMAGE=${RUSTFS_IMAGE:-rustfs/rustfs:1.0.5}
PY_IMAGE=${PY_IMAGE:-python:3.12-slim}
P=qs-verify-lake
NET=$P
D=$(mktemp -d)
chmod 700 "$D"
down() {
  docker ps -aq --filter "name=^$P-" | xargs -r docker rm -f -v >/dev/null 2>&1
  docker volume ls -q --filter "name=^$P-" | xargs -r docker volume rm >/dev/null 2>&1
  docker network rm "$NET" >/dev/null 2>&1
  true
}
cleanup() {
  down
  rm -rf "$D"
}
trap cleanup EXIT
ok() { printf '[OK  ] %s\n' "$*"; }
ko() { printf '[FAIL] %s\n' "$*"; }

# The engine the core pins: the duckdb crate in Cargo.lock and the version
# its bundled engine names (as tools/verify/duckdb_source_lib.sh reads it).
CRATE=$(awk '/^name = "duckdb"$/ { getline; gsub(/version = |"/, ""); print; exit }' Cargo.lock)
ENGINE=${DUCKDB_ENGINE:-}
if [ -z "$ENGINE" ]; then
  for f in "${CARGO_HOME:-$HOME/.cargo}"/registry/src/*/libduckdb-sys-"$CRATE"/duckdb.tar.gz; do
    [ -f "$f" ] || continue
    ENGINE=$(tar xzOf "$f" duckdb/src/function/table/version/pragma_version.cpp |
      sed -n 's/^#define DUCKDB_VERSION "\(v[^"]*\)"/\1/p')
    break
  done
fi
[ -n "$ENGINE" ] || { ko "the pinned DuckDB: crate '$CRATE' (fetch the crate, or set DUCKDB_ENGINE)"; exit 1; }
ok "DuckDB pinned: crate duckdb $CRATE, bundled engine $ENGINE; RustFS $IMAGE"

down
docker network create "$NET" >/dev/null || { ko "network $NET"; exit 1; }
printf 'ROOT_AK=qs%s\nROOT_SK=%s\nISSUER_AK=qs-issuer-%s\nISSUER_SK=%s\n' \
  "$(openssl rand -hex 8)" "$(openssl rand -hex 16)" "$(openssl rand -hex 4)" "$(openssl rand -hex 16)" >"$D/keys.env"
chmod 600 "$D/keys.env"
# shellcheck disable=SC1091
. "$D/keys.env"
docker run -d --name "$P-rustfs" --network "$NET" --network-alias rustfs \
  -v "$P-data:/data" -e RUSTFS_ACCESS_KEY="$ROOT_AK" -e RUSTFS_SECRET_KEY="$ROOT_SK" \
  -e RUSTFS_VOLUMES=/data -e RUSTFS_OBS_LOGGER_LEVEL=warn \
  "$IMAGE" server --address :9000 /data >/dev/null || { ko "start $IMAGE"; exit 1; }

# Everything else runs as Python in one container on the network: the
# issuer made with the admin API (the scene), then the checks with
# standard clients. Keys come from the environment file, never printed.
read -r -d '' CHECK <<'PY'
import hashlib, json, os, sys, time, urllib.parse
import boto3, duckdb, requests
from botocore.auth import S3SigV4Auth
from botocore.awsrequest import AWSRequest
from botocore.credentials import Credentials
from botocore.exceptions import ClientError

E = os.environ
BASE = "http://rustfs:9000"
CAT = BASE + "/iceberg"
ROOT = Credentials(E["ROOT_AK"], E["ROOT_SK"])
WH = "qd-tenant-00000000000000000000000000000001"
NS = "verify_ns"
failed = False
def ok(m): print(f"[OK  ] {m}", flush=True)
def ko(m):
    global failed; failed = True; print(f"[FAIL] {m}", flush=True)
def info(m): print(f"[INFO] {m}", flush=True)
def hide(text):
    for k in ("ROOT_AK", "ROOT_SK", "ISSUER_AK", "ISSUER_SK"):
        text = text.replace(E[k], "***")
    return text

def signed(method, url, creds=ROOT, body=b"", headers=None, token=None):
    """A request signed with SigV4 for `s3` in us-east-1, as RustFS wants
    every catalog and admin request (x-amz-content-sha256 included)."""
    if isinstance(body, (dict, list)):
        body = json.dumps(body).encode()
    h = dict(headers or {})
    h["x-amz-content-sha256"] = hashlib.sha256(body).hexdigest()
    if body:
        h.setdefault("content-type", "application/json")
    if token:
        h["x-amz-security-token"] = token
    req = AWSRequest(method=method, url=url, data=body, headers=h)
    S3SigV4Auth(creds, "s3", "us-east-1").add_auth(req)
    return requests.request(method, url, data=body, headers=dict(req.headers), timeout=60)

for _ in range(60):
    try:
        if signed("GET", BASE + "/").status_code == 200:
            break
    except requests.RequestException:
        pass
    time.sleep(1)
else:
    ko("RustFS did not answer within 60 s"); sys.exit(1)

# The scene: the issuer, as infra/rustfs/sts-issuer.sh makes it.
policy = open("/policy.json", "rb").read()
r1 = signed("PUT", BASE + "/rustfs/admin/v3/add-canned-policy?name=qs-verify-issuer", body=policy)
r2 = signed("PUT", BASE + f"/rustfs/admin/v3/add-user?accessKey={E['ISSUER_AK']}",
            body={"secretKey": E["ISSUER_SK"], "status": "enabled"})
r3 = signed("PUT", BASE + "/rustfs/admin/v3/set-user-or-group-policy?policyName=qs-verify-issuer"
            f"&userOrGroup={E['ISSUER_AK']}&isGroup=false")
codes = [r.status_code for r in (r1, r2, r3)]
ok(f"scene: the issuer with infra/rustfs/issuer/policy.json ({codes})") if all(c in (200, 204) for c in codes) \
    else ko(f"scene: the issuer {codes}")
s3 = boto3.client("s3", endpoint_url=BASE, aws_access_key_id=E["ROOT_AK"], aws_secret_access_key=E["ROOT_SK"],
                  region_name="us-east-1", config=boto3.session.Config(s3={"addressing_style": "path"}))
s3.create_bucket(Bucket=WH)
enable = signed("PUT", f"{CAT}/v1/buckets/{WH}")
ok(f"scene: warehouse {WH} made and enabled for tables ({enable.status_code})") if enable.status_code < 300 \
    else ko(f"scene: enabling tables on {WH}: {enable.status_code} {enable.text[:200]}")

print("== (a) GET /v1/config", flush=True)
config = {}
for label, query in (("no warehouse", ""), ("?warehouse=", "?warehouse=" + WH)):
    r = signed("GET", f"{CAT}/v1/config{query}")
    try:
        body = r.json()
    except ValueError:
        body = {"text": r.text[:200]}
    info(f"/v1/config ({label}): HTTP {r.status_code} {json.dumps(body, sort_keys=True)}")
    if query and r.status_code == 200:
        config = body
merged = {**config.get("defaults", {}), **config.get("overrides", {})}
PREFIX = merged.get("prefix")
URI = merged.get("uri")
info(f"the prefix it gives: {PREFIX!r}; the uri it gives: {URI!r}")
def base(prefix=PREFIX):
    return f"{CAT}/v1/{urllib.parse.quote(prefix, safe='')}" if prefix else f"{CAT}/v1"

print("== (b) X-Iceberg-Access-Delegation: vended-credentials", flush=True)
DELEG = {"X-Iceberg-Access-Delegation": "vended-credentials"}
r = signed("POST", f"{base()}/namespaces", body={"namespace": [NS], "properties": {}})
info(f"create namespace {NS}: HTTP {r.status_code}")
schema = {"type": "struct", "schema-id": 0, "fields": [
    {"id": 1, "name": "n", "required": False, "type": "long"},
    {"id": 2, "name": "label", "required": False, "type": "string"}]}
def table_body(name):
    return {"name": name, "schema": schema, "stage-create": False, "properties": {}}
SECRET_WORDS = ("access-key", "secret", "session-token", "token")
def credential_keys(body):
    """The credential settings of a table's answer: a vended key, secret or
    token by its name only; RustFS's own mode settings with their value."""
    config = body.get("config", {})
    shown = {k: (v if k.startswith("rustfs.") and not any(w in k for w in SECRET_WORDS) else "<set>")
             for k, v in sorted(config.items()) if "credential" in k or any(w in k for w in SECRET_WORDS)}
    stored = body.get("storage-credentials") or []
    return shown, [sorted(c.get("config", {})) for c in stored]
r = signed("POST", f"{base()}/namespaces/{NS}/tables", body=table_body("delegated"), headers=DELEG)
body = r.json() if r.headers.get("content-type", "").startswith("application/json") else {}
info(f"create table with the header: HTTP {r.status_code}; credential keys in config, storage-credentials: {credential_keys(body)}")
r = signed("GET", f"{base()}/namespaces/{NS}/tables/delegated", headers=DELEG)
body = r.json() if r.headers.get("content-type", "").startswith("application/json") else {}
info(f"load table with the header: HTTP {r.status_code}; credential keys in config, storage-credentials: {credential_keys(body)}")
r = signed("GET", f"{base()}/namespaces/{NS}/tables/delegated")
body = r.json() if r.headers.get("content-type", "").startswith("application/json") else {}
info(f"load table without it: HTTP {r.status_code}; credential keys in config, storage-credentials: {credential_keys(body)}")
location = body.get("metadata", {}).get("location") or body.get("metadata-location", "")
info(f"the table's location: {location}")

print("== (d) If-None-Match: * on a second write", flush=True)
key = f"{NS}/if-none-match.txt"
first = signed("PUT", f"{BASE}/{WH}/{key}", body=b"first", headers={"If-None-Match": "*"})
second = signed("PUT", f"{BASE}/{WH}/{key}", body=b"second", headers={"If-None-Match": "*"})
kept = s3.get_object(Bucket=WH, Key=key)["Body"].read().decode()
info(f"first PUT: HTTP {first.status_code}; second PUT: HTTP {second.status_code}; the object holds {kept!r}")
ok("If-None-Match: * is honoured (412, the first kept)") if second.status_code == 412 and kept == "first" \
    else info("If-None-Match: * is NOT honoured as S3 does")

print("== (e) STS AssumeRole", flush=True)
# The client's own checks off, so what is refused is refused by RustFS.
sts = boto3.client("sts", endpoint_url=BASE, aws_access_key_id=E["ISSUER_AK"],
                   aws_secret_access_key=E["ISSUER_SK"], region_name="us-east-1",
                   config=boto3.session.Config(parameter_validation=False))
read_only = json.dumps({"Version": "2012-10-17", "Statement": [{"Effect": "Allow",
    "Action": ["s3:GetObject", "s3:ListBucket"], "Resource": [f"arn:aws:s3:::{WH}", f"arn:aws:s3:::{WH}/*"]}]})
def assume(seconds=900, role="arn:aws:iam::000000000000:role/qd-job"):
    args = {"RoleSessionName": "verify", "DurationSeconds": seconds, "Policy": read_only}
    if role is not None:
        args["RoleArn"] = role
    try:
        c = sts.assume_role(**args)["Credentials"]
        left = (c["Expiration"].timestamp() - time.time())
        return f"taken, expires in {left:.0f} s", c
    except ClientError as e:
        return f"refused: {e.response['Error'].get('Code')} {e.response['Error'].get('Message', '')[:100]}", None
    except Exception as e:
        return f"refused: {type(e).__name__} {str(e)[:100]}", None
shortest = None
for seconds in (60, 300, 899, 900, 3600, 43200, 43201):
    what, c = assume(seconds)
    info(f"DurationSeconds={seconds}: {what}")
    if c and shortest is None:
        shortest = seconds
info(f"the shortest DurationSeconds taken among those asked: {shortest}")
for label, role in (("arn:aws:iam::000000000000:role/qd-job", "arn:aws:iam::000000000000:role/qd-job"),
                    ("another account and role", "arn:aws:iam::123456789012:role/anything"),
                    ("a malformed one", "not-an-arn"), ("an empty one", ""), ("none at all", None)):
    what, _ = assume(900, role)
    info(f"RoleArn {label}: {what}")
_, creds = assume(900)
if creds:
    job = Credentials(creds["AccessKeyId"], creds["SecretAccessKey"], creds["SessionToken"])
    r = signed("GET", f"{base()}/namespaces/{NS}/tables/delegated", creds=job, token=creds["SessionToken"])
    info(f"the catalog with the job's credentials (read-only policy, no admin: action): load table HTTP {r.status_code}")

print("== (f) DuckDB's iceberg with the URI and prefix of /v1/config", flush=True)
con = duckdb.connect()
con.execute("SET enable_progress_bar = false")
version = con.execute("SELECT library_version FROM pragma_version()").fetchone()[0]
ok(f"DuckDB {version}") if version == E["ENGINE"] else ko(f"DuckDB {version}, expected {E['ENGINE']}")
con.execute("INSTALL iceberg; LOAD iceberg; INSTALL httpfs; LOAD httpfs;")
ext = con.execute("SELECT extension_name, extension_version FROM duckdb_extensions() WHERE extension_name IN ('iceberg','httpfs') ORDER BY 1").fetchall()
info(f"extensions: {ext}")
con.execute(f"""CREATE SECRET s3 (TYPE s3, KEY_ID '{E['ROOT_AK']}', SECRET '{E['ROOT_SK']}', REGION 'us-east-1',
                ENDPOINT 'rustfs:9000', URL_STYLE 'path', USE_SSL false)""")
attaches = [("today's <endpoint>/iceberg", CAT)]
if URI:
    attaches.append(("the uri /v1/config gives", URI))
for i, (label, uri) in enumerate(attaches):
    name = f"lake{i}"
    try:
        con.execute(f"""ATTACH '{WH}' AS {name} (TYPE iceberg, URI '{uri}',
                        AUTHORIZATION_TYPE 'sigv4', SIGV4_SERVICE 's3', SIGV4_REGION 'us-east-1', SECRET s3)""")
        if i == 0:
            con.execute(f"CREATE TABLE {name}.{NS}.duck AS SELECT range AS n, 'row ' || range AS label FROM range(10)")
        n = con.execute(f"SELECT count(*) FROM {name}.{NS}.duck").fetchone()[0]
        ok(f"ATTACH with {label} ({uri}): a table written and read, {n} rows")
    except Exception as e:
        ko(f"ATTACH with {label} ({uri}): {hide(str(e))[:300]}")
if PREFIX:
    info(f"DuckDB is not told the prefix: it reads /v1/config itself, which gave {PREFIX!r}")
else:
    info("no prefix in /v1/config: DuckDB uses paths without one")
print("== (c) DELETE with purgeRequested=true, on the table DuckDB wrote", flush=True)
loaded = signed("GET", f"{base()}/namespaces/{NS}/tables/duck").json()
location = loaded.get("metadata", {}).get("location", "")
def objects_under(location):
    if not location.startswith("s3://"):
        return None
    bucket, _, key = location[5:].partition("/")
    return s3.list_objects_v2(Bucket=bucket, Prefix=key + "/").get("KeyCount", 0)
info(f"before: objects under {location}: {objects_under(location)}")
r = signed("DELETE", f"{base()}/namespaces/{NS}/tables/duck?purgeRequested=true")
info(f"DELETE ?purgeRequested=true: HTTP {r.status_code} {r.text[:200]}")
after = signed("GET", f"{base()}/namespaces/{NS}/tables/duck")
info(f"after it: the table loads: HTTP {after.status_code}; objects under it: {objects_under(location)}")
r = signed("DELETE", f"{base()}/namespaces/{NS}/tables/duck")
after = signed("GET", f"{base()}/namespaces/{NS}/tables/duck")
info(f"DELETE without purgeRequested: HTTP {r.status_code}; the table loads: HTTP {after.status_code}; "
     f"objects under it: {objects_under(location)}")
sys.exit(1 if failed else 0)
PY

docker run --rm --name "$P-client" --network "$NET" --env-file "$D/keys.env" -e ENGINE="$ENGINE" \
  -e CHECK="$CHECK" -v "$PWD/infra/rustfs/issuer/policy.json:/policy.json:ro" "$PY_IMAGE" \
  sh -c 'pip install --quiet --disable-pip-version-check --root-user-action=ignore \
      "duckdb==${ENGINE#v}" boto3 requests >/dev/null 2>&1 && python -I -c "$CHECK"'
status=$?
[ "$status" = 0 ] && ok "every check ran" || ko "the checks ended with $status"
exit "$status"
