#!/bin/bash
# What RustFS does that the lake roles' installation would build on
# (decision 0029, "Lake nodes", points 5 to 7; decision 0066, points 10 and
# 12, step 11): a pool of 4 nodes formed from RUSTFS_VOLUMES, with the same
# or different root keys, over TLS with certificates of one CA, with client
# certificates required between nodes; the image's user and the data
# directory it needs; and replication to a server behind a front that
# requires a client certificate, as the lake's gate does.
#
#   (a) the image runs as uid 10001; without root keys it starts on its
#       built-in default credential;
#   (b) a data directory bind-mounted from the host is written only when it
#       is RustFS's user's;
#   (c) a pool of 4 forms from RUSTFS_VOLUMES over plain HTTP, and not when
#       one node has other root keys;
#   (d) over TLS, with only client_ca.crt in RUSTFS_TLS_PATH (what
#       infra/rustfs/tls/entrypoint.sh writes) and with ca.crt too;
#   (e) with client certificates required (RUSTFS_SERVER_MTLS_ENABLE=1),
#       without and with client_cert.pem and client_key.pem;
#   (f) replication to a RustFS behind a Traefik 3.6 that does not require
#       a client certificate, and one that does, the source holding
#       client_cert.pem and RUSTFS_MTLS_CLIENT_CERT/_KEY.
#
# Throwaway containers only (qd-verify-lakenodes-…, on a network of their
# own, no port published), a throwaway CA, random root keys that are never
# printed. Run from the repository root:
#
#   bash tools/verify/rustfs_lake_nodes.sh
#
# Needs docker, curl 7.75 or later (--aws-sigv4), openssl and uv.
set -u
export LC_ALL=C
IMAGE=${RUSTFS_IMAGE:-rustfs/rustfs:1.0.5}
TRAEFIK=${TRAEFIK_IMAGE:-traefik:v3.6}
P=qd-verify-lakenodes
WAIT=${QD_VERIFY_WAIT:-45}
D=$(mktemp -d)
chmod 700 "$D"
export D # repl.py reads its files there
cleanup() {
  docker ps -aq --filter "name=^$P-" | xargs -r docker rm -f >/dev/null 2>&1
  docker network rm "$P" >/dev/null 2>&1
  # The data directory made RustFS's is not ours to remove.
  docker run --rm -v "$D:/d" alpine:3.20 rm -rf /d/data-ok >/dev/null 2>&1
  rm -rf "$D"
}
trap cleanup EXIT
FAILED=0
ok() { printf '[OK  ] %s\n' "$*"; }
ko() {
  printf '[FAIL] %s\n' "$*"
  FAILED=1
}
expect() { # NAME EXPECTED ACTUAL
  if [ "$2" = "$3" ]; then ok "$1: $3"; else ko "$1: expected '$2', got '$3'"; fi
}

# --- Certificates and keys -----------------------------------------------------
openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -days 1 \
  -subj "/CN=qd verify lake nodes" -addext "basicConstraints=critical,CA:TRUE" \
  -keyout "$D/ca.key" -out "$D/ca.crt" 2>/dev/null
# cert NAME: a machine's certificate as the CA makes them, client and server
# use in one, the name its only DNS name.
cert() {
  openssl req -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -subj "/O=qd/OU=lake/CN=$1" \
    -keyout "$D/$1.key" -out "$D/$1.csr" 2>/dev/null
  printf 'basicConstraints=CA:FALSE\nextendedKeyUsage=serverAuth,clientAuth\nsubjectAltName=DNS:%s\n' "$1" >"$D/ext"
  openssl x509 -req -in "$D/$1.csr" -CA "$D/ca.crt" -CAkey "$D/ca.key" -CAcreateserial \
    -extfile "$D/ext" -days 1 -out "$D/$1.crt" 2>/dev/null
}
keys() { # FILE: random root keys, as an env file only docker reads
  printf 'RUSTFS_ACCESS_KEY=qd%s\nRUSTFS_SECRET_KEY=%s\n' "$(openssl rand -hex 8)" "$(openssl rand -hex 16)" >"$1"
  chmod 600 "$1"
}
# tlsdir NAME WHAT...: NAME's TLS directory; WHAT among ca (ca.crt) and
# client (client_cert.pem, client_key.pem); client_ca.crt always.
tlsdir() {
  local n=$1 t=$D/tls-$1
  shift
  mkdir -p "$t"
  cp "$D/$n.crt" "$t/rustfs_cert.pem"
  cp "$D/$n.key" "$t/rustfs_key.pem"
  cp "$D/ca.crt" "$t/client_ca.crt"
  for w in "$@"; do
    case $w in
    ca) cp "$D/ca.crt" "$t/ca.crt" ;;
    client) cp "$D/$n.crt" "$t/client_cert.pem" && cp "$D/$n.key" "$t/client_key.pem" ;;
    esac
  done
  chmod 755 "$t"
  chmod 644 "$t"/*
}
ip() { docker inspect -f '{{range .NetworkSettings.Networks}}{{.IPAddress}}{{end}}' "$1"; }
# curlcfg KEYS [NODE]: a curl config that signs with KEYS (SigV4), and
# presents NODE's certificate when given; the keys stay in files.
curlcfg() {
  (
    # shellcheck source=/dev/null
    . "$1"
    printf 'user = "%s:%s"\naws-sigv4 = "aws:amz:us-east-1:s3"\n' "$RUSTFS_ACCESS_KEY" "$RUSTFS_SECRET_KEY"
    [ -z "${2:-}" ] || printf 'cert = "%s"\nkey = "%s"\n' "$D/$2.crt" "$D/$2.key"
  ) >"$D/curl.cfg"
  chmod 600 "$D/curl.cfg"
}
# s3 NAME ARGS...: a signed request to NAME, verified against the CA.
s3() {
  local n=$1
  shift
  curl -s --cacert "$D/ca.crt" --max-time 10 --resolve "$n:9000:$(ip "$n")" -K "$D/curl.cfg" "$@" 2>/dev/null
}
docker network create "$P" >/dev/null || exit 1
echo synthetic >"$D/obj"

# --- (a) The image's user, and no root keys --------------------------------------
echo "== (a) the image"
expect "the image's user and its uid" "rustfs 10001" \
  "$(docker inspect -f '{{.Config.User}}' "$IMAGE") $(docker run --rm --entrypoint id "$IMAGE" -u rustfs)"
docker run -d --name "$P-nokeys" --network "$P" -e RUSTFS_VOLUMES=/data "$IMAGE" server --address :9000 >/dev/null
sleep 3
case $(docker logs "$P-nokeys" 2>&1) in
*"falls back to its built-in default credential"*) ok "without RUSTFS_ACCESS_KEY it starts on the built-in default credential (a warning only)" ;;
*) ko "without root keys: no fallback warning" ;;
esac
docker rm -f "$P-nokeys" >/dev/null

# --- (b) The data directory -------------------------------------------------------
echo "== (b) a data directory bind-mounted from the host"
mkdir -m 755 "$D/data-bad" "$D/data-ok"
docker run --rm -v "$D/data-ok:/x" alpine:3.20 chown 10001:10001 /x
keys "$D/one.env"
curlcfg "$D/one.env"
for which in bad ok; do
  docker run -d --name "$P-$which" --network "$P" --env-file "$D/one.env" -e RUSTFS_VOLUMES=/data \
    -v "$D/data-$which:/data" "$IMAGE" server --address :9000 >/dev/null
done
sleep 8
code() { # NAME: PUT of a bucket, the HTTP status
  curl -s -o /dev/null -w '%{http_code}' --max-time 10 -K "$D/curl.cfg" -X PUT "http://$(ip "$1"):9000/qd-verify" 2>/dev/null
}
r=$(code "$P-bad")
if [ "$r" != 200 ]; then ok "a directory of another user (uid $(id -u), 0755): no bucket made ($r)"; else
  ko "a directory of another user: a bucket made"
fi
expect "the same directory owned by 10001: a bucket made" 200 "$(code "$P-ok")"
docker rm -f "$P-bad" "$P-ok" >/dev/null

# --- (c) to (e) Pools of 4 --------------------------------------------------------
# pool CASE SCHEME KEYS4 MTLS WHAT...: four nodes CASE-1..4, RUSTFS_VOLUMES
# naming all of them; node 4 with other root keys when KEYS4 is `other`; TLS
# with the TLS directory of WHAT when SCHEME is https; client certificates
# required when MTLS is 1.
pool() {
  local c=$1 scheme=$2 keys4=$3 mtls=$4 i n kf
  shift 4
  keys "$D/$c.env"
  for i in 1 2 3 4; do
    n=$P-$c-$i
    kf=$D/$c.env
    [ "$keys4" = other ] && [ $i = 4 ] && kf=$D/$c-4.env && keys "$kf"
    local tls=()
    if [ "$scheme" = https ]; then
      cert "$n"
      tlsdir "$n" "$@"
      tls=(-e RUSTFS_TLS_PATH=/tls -v "$D/tls-$n:/tls:ro" -e "RUSTFS_SERVER_MTLS_ENABLE=$mtls")
    fi
    docker run -d --name "$n" --network "$P" --env-file "$kf" -e RUSTFS_OBS_LOGGER_LEVEL=info \
      -e "RUSTFS_VOLUMES=$scheme://$P-$c-{1...4}:9000/data" "${tls[@]}" "$IMAGE" server --address :9000 >/dev/null
  done
  if [ "$mtls" = 1 ]; then curlcfg "$D/$c.env" "$P-$c-1"; else curlcfg "$D/$c.env"; fi
}
# formed CASE SCHEME: within WAIT seconds, a bucket and an object written on
# node 1 and read on node 3; prints `formed` or what node 3 answered.
formed() {
  local s=$2 n1=$P-$1-1 n3=$P-$1-3 got=''
  for _ in $(seq "$WAIT"); do
    if [ "$(s3 "$n1" -o /dev/null -w '%{http_code}' -X PUT "$s://$n1:9000/qd-verify")" = 200 ]; then
      s3 "$n1" -o /dev/null -T "$D/obj" "$s://$n1:9000/qd-verify/check.txt"
      got=$(s3 "$n3" "$s://$n3:9000/qd-verify/check.txt")
      [ "$got" = synthetic ] && { echo formed; return; }
    fi
    sleep 1
  done
  got=$(s3 "$n3" "$s://$n3:9000/qd-verify/check.txt" | head -c 60)
  echo "not formed: ${got:-no answer}"
}
logged() { # CASE TEXT: whether node 1's log holds TEXT
  docker exec "$P-$1-1" grep -q -- "$2" /logs/rustfs.log 2>/dev/null && echo yes || echo no
}
down() { docker ps -aq --filter "name=^$P-$1-" | xargs -r docker rm -f >/dev/null; }

echo "== (c) a pool of 4 from RUSTFS_VOLUMES, plain HTTP"
pool plain http same 0
expect "the same root keys on every node: written on node 1, read on node 3" formed "$(formed plain http)"
down plain
pool keys http other 0
r=$(formed keys http)
case $r in not\ formed*) ok "node 4 with other root keys: the pool does not form ($r)" ;; *) ko "node 4 with other root keys: $r" ;; esac
expect "… the nodes refuse each other's RPC" yes "$(logged keys 'RPC signature verification failed')"
down keys

echo "== (d) TLS, each node serving its machine certificate"
pool tlsonly https same 0
r=$(formed tlsonly https)
case $r in not\ formed*) ok "only client_ca.crt in RUSTFS_TLS_PATH: the pool does not form ($r)" ;; *) ko "only client_ca.crt: $r" ;; esac
expect "… a node does not trust its peers' certificates" yes "$(logged tlsonly 'UnknownCA')"
down tlsonly
pool tlsca https same 0 ca
expect "ca.crt in RUSTFS_TLS_PATH too: the pool forms over TLS" formed "$(formed tlsca https)"
down tlsca

echo "== (e) client certificates required between nodes (RUSTFS_SERVER_MTLS_ENABLE=1)"
pool mtls https same 1 ca
r=$(formed mtls https)
case $r in not\ formed*) ok "without client_cert.pem: the pool does not form ($r)" ;; *) ko "without client_cert.pem: $r" ;; esac
expect "… a node presents no certificate to its peers" yes "$(logged mtls 'peer sent no certificates')"
down mtls
pool mtlsc https same 1 ca client
expect "client_cert.pem and client_key.pem in RUSTFS_TLS_PATH: the pool forms" formed "$(formed mtlsc https)"
down mtlsc

# --- (f) Replication behind a front ---------------------------------------------
echo "== (f) replication to a RustFS behind a Traefik"
SRC=$P-src DST=$P-dst
cert "$SRC"
cert "$DST"
tlsdir "$SRC" ca client
keys "$D/src.env"
keys "$D/dst.env"
mkdir "$D/gate"
cp "$D/$DST.crt" "$D/$DST.key" "$D/ca.crt" "$D/gate/"
chmod 755 "$D/gate"
chmod 644 "$D/gate"/*
gate() { # CLIENTAUTH: the front's dynamic file
  cat >"$D/gate/dyn.yml" <<EOF
tls:
  stores:
    default:
      defaultCertificate: {certFile: /g/$DST.crt, keyFile: /g/$DST.key}
  options:
    default:
      clientAuth: {caFiles: [/g/ca.crt], clientAuthType: $1}
http:
  routers:
    all: {rule: "PathPrefix(\`/\`)", entryPoints: [s3], service: fs, tls: {}}
  services:
    fs:
      loadBalancer:
        servers: [{url: "http://$DST-fs:9000"}]
EOF
  chmod 644 "$D/gate/dyn.yml"
}
cat >"$D/repl.py" <<'EOF'
"""Replication from SRC to the front of DST; prints what happened, no key."""
import hashlib, json, os, sys, time
import boto3, requests
from botocore.auth import S3SigV4Auth
from botocore.awsrequest import AWSRequest
from botocore.config import Config
from botocore.credentials import Credentials
from requests.adapters import HTTPAdapter
D, BUCKET = os.environ["D"], sys.argv[1]
def keys(f):
    d = dict(l.strip().split("=", 1) for l in open(f"{D}/{f}") if "=" in l)
    return d["RUSTFS_ACCESS_KEY"], d["RUSTFS_SECRET_KEY"]
SRC, DST = keys("src.env"), keys("dst.env")
BASE = f"https://{os.environ['SRC_IP']}:9000"
class SigV4(HTTPAdapter):
    def __init__(self, creds):
        super().__init__(); self.creds = Credentials(*creds)
    def add_headers(self, request, **kw):
        body = request.body or b""
        body = body.encode() if isinstance(body, str) else body
        request.headers["x-amz-content-sha256"] = hashlib.sha256(body).hexdigest()
        request.headers["content-length"] = str(len(body))
        request.headers.pop("connection", None)
        aws = AWSRequest(method=request.method, url=request.url, data=body, headers=dict(request.headers))
        S3SigV4Auth(self.creds, "s3", "us-east-1").add_auth(aws)
        request.headers.update(dict(aws.headers))
def s3(creds, url, verify):
    return boto3.client("s3", endpoint_url=url, aws_access_key_id=creds[0], aws_secret_access_key=creds[1],
                        region_name="us-east-1", verify=verify,
                        config=Config(s3={"addressing_style": "path"}, retries={"max_attempts": 1}))
src = s3(SRC, BASE, False)  # reached by address: the name is not what this checks
dst = s3(DST, f"http://{os.environ['DST_IP']}:9000", True)
for c in (src, dst):
    c.create_bucket(Bucket=BUCKET)
    c.put_bucket_versioning(Bucket=BUCKET, VersioningConfiguration={"Status": "Enabled"})
s = requests.Session(); s.mount(BASE, SigV4(SRC))
r = s.put(f"{BASE}/rustfs/admin/v3/set-remote-target?bucket={BUCKET}", verify=False, data=json.dumps({
    "endpoint": f"{os.environ['DST']}:9000", "credentials": {"accessKey": DST[0], "secretKey": DST[1]},
    "targetbucket": BUCKET, "secure": True, "type": "replication", "region": "us-east-1"}))
if r.status_code >= 300:
    reason = "connection error" if "Connection error" in r.text else "another error"
    print(f"remote target refused ({r.status_code}, {reason})"); sys.exit(0)
arn = r.text.strip().strip('"')
src.put_bucket_replication(Bucket=BUCKET, ReplicationConfiguration={"Role": arn, "Rules": [
    {"ID": "to-the-replica", "Status": "Enabled", "Priority": 1, "Filter": {"Prefix": ""},
     "DeleteMarkerReplication": {"Status": "Disabled"}, "Destination": {"Bucket": arn}}]})
src.put_object(Bucket=BUCKET, Key="check.txt", Body=b"synthetic")
for _ in range(30):
    try:
        dst.head_object(Bucket=BUCKET, Key="check.txt"); print("replicated"); sys.exit(0)
    except Exception:
        time.sleep(1)
print("not replicated within 30 s")
EOF
docker run -d --name "$SRC" --network "$P" --env-file "$D/src.env" -e RUSTFS_VOLUMES=/data \
  -e RUSTFS_TLS_PATH=/tls -e RUSTFS_MTLS_CLIENT_CERT=/tls/client_cert.pem -e RUSTFS_MTLS_CLIENT_KEY=/tls/client_key.pem \
  -v "$D/tls-$SRC:/tls:ro" "$IMAGE" server --address :9000 >/dev/null
docker run -d --name "$DST-fs" --network "$P" --env-file "$D/dst.env" -e RUSTFS_VOLUMES=/data \
  "$IMAGE" server --address :9000 >/dev/null
gate NoClientCert
docker run -d --name "$DST" --network "$P" -v "$D/gate:/g:ro" "$TRAEFIK" \
  --entrypoints.s3.address=:9000 --providers.file.directory=/g --providers.file.watch=true >/dev/null
sleep 10
repl() { # BUCKET
  SRC_IP=$(ip "$SRC") DST_IP=$(ip "$DST-fs") DST=$DST \
    uv run -q --no-project --with boto3==1.44.2 --with requests==2.34.2 python3 -I "$D/repl.py" "$1" 2>/dev/null | tail -1
}
expect "a front that asks no client certificate: an object reaches the other server" replicated "$(repl qd-verify-open)"
gate RequireAndVerifyClientCert
sleep 3
expect "a front that requires one, the source holding client_cert.pem and RUSTFS_MTLS_CLIENT_*: the remote target cannot be set" \
  "remote target refused (400, connection error)" "$(repl qd-verify-gated)"

echo
if [ "$FAILED" = 0 ]; then echo "ALL OK"; else
  echo "SOME CHECKS FAILED"
  exit 1
fi
