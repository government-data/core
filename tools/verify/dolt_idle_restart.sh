#!/bin/bash
# Verification of the idle restart of Dolt (decision 0066, point 11): how an
# agent on the machine asks Dolt whether a client is using it, what happens to
# the connections, statements and transactions open when it restarts, how long
# it takes to serve TLS with client certificates again (and the renewed server
# certificate), and whether a stop lets running statements finish.
#
# A throwaway Dolt (dolthub/dolt-sql-server:2.6.1, the image of infra/dolt) on
# Docker's default bridge, with a throwaway CA, set up as dolt_mtls.sh leaves
# it: TLS required and a client certificate of the CA required for every
# connection. Passwords are random and never printed; the rows are synthetic.
# The dev stack is not touched. Run from the repository root:
#
#   bash tools/verify/dolt_idle_restart.sh
#
# Needs docker, openssl and uv (pymysql).
set -u
export LC_ALL=C
IMAGE=${DOLT_IMAGE:-dolthub/dolt-sql-server:2.6.1}
NAME=qd-verify-dolt-idle
D=$(mktemp -d)
chmod 700 "$D"
trap 'docker rm -f "$NAME" >/dev/null 2>&1; rm -rf "$D"' EXIT
FAILED=0

ok() { printf '[OK  ] %s\n' "$*"; }
ko() { printf '[FAIL] %s\n' "$*"; FAILED=1; }
expect() { if [ "$2" = "$3" ]; then ok "$1: $3"; else ko "$1: expected '$2', got '$3'"; fi; }
like() { case "$3" in *"$2"*) ok "$1: $(echo "$3" | head -1 | cut -c1-110)" ;; *) ko "$1: expected '$2', got '$3'" ;; esac; }

# --- Certificates (as dolt_mtls.sh) ------------------------------------------------
ca() {
  openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -days 1 \
    -subj "/CN=qd verify $1" -addext "basicConstraints=critical,CA:TRUE" \
    -keyout "$D/$1.key" -out "$D/$1.crt" 2>/dev/null
}
cert() { # FILE SUBJECT USAGE
  openssl req -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -subj "$2" \
    -keyout "$D/$1.key" -out "$D/$1.csr" 2>/dev/null
  printf 'basicConstraints=CA:FALSE\nextendedKeyUsage=%s\nsubjectAltName=DNS:dolt.qd.test\n' "$3" >"$D/ext"
  openssl x509 -req -in "$D/$1.csr" -CA "$D/ca.crt" -CAkey "$D/ca.key" -CAcreateserial \
    -extfile "$D/ext" -out "$D/$1.crt" -days 1 2>/dev/null
}
ca ca
cert server "/CN=dolt.qd.test" serverAuth
cert api "/O=qd/OU=api/CN=api-1" clientAuth
cert agent "/O=qd/OU=dolt/CN=dolt-1" clientAuth
T=$D/tls
mkdir "$T" && chmod 755 "$T"
put() { cp "$1" "$T/.$2.new" && chmod 644 "$T/.$2.new" && mv -f "$T/.$2.new" "$T/$2"; }
put "$D/server.crt" server.crt
put "$D/server.key" server.key
put "$D/ca.crt" ca.crt
config() { # REQUIRE_CLIENT_CERT
  mkdir -p "$D/cfg" && chmod 755 "$D/cfg"
  printf 'log_level: info\nlistener:\n  host: 0.0.0.0\n  port: 3306\n  tls_cert: /tls/server.crt\n  tls_key: /tls/server.key\n  ca_cert: /tls/ca.crt\n  require_secure_transport: true\n  require_client_cert: %s\ndata_dir: /var/lib/dolt\n' "$1" >"$D/cfg/config.yaml"
  chmod 644 "$D/cfg/config.yaml"
}
config false
PW=$(openssl rand -hex 12)
docker run -d --name "$NAME" -e DOLT_ROOT_PASSWORD="$(openssl rand -hex 12)" -e DOLT_ROOT_HOST=localhost \
  -v "$T:/tls:ro" -v "$D/cfg:/etc/dolt/servercfg.d:ro" "$IMAGE" >/dev/null || { echo "could not start $IMAGE"; exit 1; }
ready() { for _ in $(seq 120); do [ "$(docker logs "$NAME" 2>&1 | grep -c 'Ready for connections')" -ge "$1" ] && return 0; sleep 0.5; done; return 1; }
ready 1 || { echo "Dolt did not start"; exit 1; }
root() { docker exec -w /var/lib/dolt "$NAME" dolt sql -q "$1" 2>&1; }
# The API's user owns the database; the agent's has PROCESS only, or nothing.
root "CREATE DATABASE verify; CREATE TABLE verify.t (id int primary key, v varchar(20));
  CREATE USER qd_api@'%' IDENTIFIED BY '$PW'; GRANT ALL ON verify.* TO qd_api@'%';
  CREATE USER qd_agent@'%' IDENTIFIED BY '$PW'; GRANT PROCESS ON *.* TO qd_agent@'%';
  CREATE USER qd_nobody@'%' IDENTIFIED BY '$PW'" >/dev/null
config true
docker restart "$NAME" >/dev/null
IP=$(docker inspect -f '{{range .NetworkSettings.Networks}}{{.IPAddress}}{{end}}' "$NAME")

# The client, pymysql over TLS with a certificate:
#   q USER CERT SQL     the first column of the first row, or the error
#   ps USER CERT        the other connections: Command/first word of Info
#   pem                 the serial-bearing server certificate
#   hold                an idle connection, SLEEP(25), an open transaction; on
#                       ./go, the next statement on each; outcomes in ./hold.log
cat >"$D/c.py" <<'EOF'
import os, ssl as tls, sys, threading, time, pymysql
d, host, pw = os.environ["D"], os.environ["IP"], os.environ["PW"]
def conn(user, crt, **kw):
    ssl = {"ca": f"{d}/ca.crt", "check_hostname": False, "cert": f"{d}/{crt}.crt", "key": f"{d}/{crt}.key"}
    return pymysql.connect(host=host, user=user, password=pw, ssl=ssl, connect_timeout=3, **kw)
def err(e):
    return f"{type(e).__name__} {e.args[0] if e.args else ''}: {str(e.args[-1] if e.args else e).splitlines()[0][:70]}"
def one(c, sql):
    with c.cursor() as cur:
        cur.execute(sql); r = cur.fetchone()
        return r[0] if r else None
mode = sys.argv[1]
try:
    if mode == "q":
        db = "verify" if sys.argv[2] == "qd_api" else None
        print(one(conn(sys.argv[2], sys.argv[3], autocommit=True, database=db), sys.argv[4]))
    elif mode == "n":
        with conn(sys.argv[2], sys.argv[3]).cursor() as cur:
            print(cur.execute(sys.argv[4]))
    elif mode == "ps":
        c = conn(sys.argv[2], sys.argv[3]); me = one(c, "SELECT CONNECTION_ID()")
        with c.cursor() as cur:
            cur.execute("SELECT id, command, info FROM information_schema.processlist")
            rows = [f"{cmd}/{(info or '-').split(' ')[0]}" for i, cmd, info in cur.fetchall() if i != me]
        print(" ".join(sorted(rows)) or "none")
    elif mode == "pem":
        c = conn("qd_agent", "agent")
        print(tls.DER_cert_to_PEM_cert(c._sock.getpeercert(True)))
    elif mode == "hold":
        log = open(f"{d}/hold.log", "a", buffering=1)
        idle = conn("qd_api", "api", database="verify", autocommit=True)
        tx = conn("qd_api", "api", database="verify", autocommit=False)
        tx.begin(); one(tx, "INSERT INTO t VALUES (3, 'uncommitted')")
        def sleeper():
            t0 = time.time()
            try: r = one(conn("qd_api", "api", database="verify"), "SELECT SLEEP(25)"); log.write(f"sleep: returned {r}\n")
            except Exception as e: log.write(f"sleep: {err(e)} after {time.time() - t0:.0f}s\n")
        threading.Thread(target=sleeper).start()
        log.write("ready\n")
        while not os.path.exists(f"{d}/go"): time.sleep(0.2)
        for name, c, sql in (("idle", idle, "SELECT 1"), ("tx", tx, "COMMIT")):
            try: one(c, sql); log.write(f"{name}: ok\n")
            except Exception as e: log.write(f"{name}: {err(e)}\n")
        log.write("done\n")
except Exception as e:
    print(err(e))
EOF
py() { D=$D IP=$IP PW=$PW uv run -q --with pymysql==1.1.2 python3 -I "$D/c.py" "$@" 2>&1 | tail -1; }
served() { D=$D IP=$IP PW=$PW uv run -q --with pymysql==1.1.2 python3 -I "$D/c.py" pem 2>/dev/null | openssl x509 -noout -serial 2>/dev/null; }
up() { for _ in $(seq 240); do [ "$(py q qd_api api 'SELECT 1')" = 1 ] && return 0; sleep 0.25; done; return 1; }
logged() { for _ in $(seq 120); do grep -q "^$1" "$D/hold.log" 2>/dev/null && break; sleep 0.5; done; grep "^$1" "$D/hold.log" | head -1; }
up || { echo "Dolt did not come back with client certificates required"; exit 1; }
py q qd_api api "INSERT INTO verify.t VALUES (1, 'committed')" >/dev/null
py q qd_api api "CALL DOLT_COMMIT('-Am', 'synthetic row 1')" >/dev/null
py q qd_api api "INSERT INTO verify.t VALUES (2, 'working set')" >/dev/null
expect "set up: rows, and Dolt commits on main" "2 2" "$(py q qd_api api 'SELECT count(*) FROM verify.t') $(py q qd_api api 'SELECT count(*) FROM verify.dolt_log')"

echo "== (a) Asking Dolt whether a client is using it (information_schema.processlist)"
expect "no other client, as qd_agent (GRANT PROCESS)" none "$(py ps qd_agent agent)"
py hold &
HOLD=$!
logged ready >/dev/null
sleep 2
expect "as qd_agent: SLEEP(25) is a Query; the idle one and the open transaction, both Sleep" \
  "Query/SELECT Sleep/- Sleep/-" "$(py ps qd_agent agent)"
expect "as qd_nobody (no grant at all): the same, with their statements (no PROCESS needed)" \
  "Query/SELECT Sleep/- Sleep/-" "$(py ps qd_nobody agent)"
expect "SHOW PROCESSLIST as qd_agent: the three and its own" 4 "$(py n qd_agent agent 'SHOW PROCESSLIST')"
expect "the open transaction's insert is not visible to others" 2 "$(py q qd_api api 'SELECT count(*) FROM verify.t')"

echo "== (b) and (c) A restart under those clients, with a renewed server certificate"
cert server "/CN=dolt.qd.test" serverAuth
put "$D/server.key" server.key
put "$D/server.crt" server.crt
n=$(docker logs "$NAME" 2>&1 | grep -c 'Ready for connections')
t0=$(date +%s.%N)
docker restart "$NAME" >/dev/null
up
SECS=$(echo "$(date +%s.%N) - $t0" | bc)
ready $((n + 1))
SECS2=$(echo "$(date +%s.%N) - $t0" | bc)
if [ "$(echo "$SECS < 10" | bc)" = 1 ]; then ok "docker restart until a client certificate connects: $(printf %.1f "$SECS")s"; else ko "no connection within 10s"; fi
if [ "$(echo "$SECS2 < 15" | bc)" = 1 ]; then ok "until the entrypoint says Ready for connections (later than the server): $(printf %.1f "$SECS2")s"; else ko "no Ready line within 15s"; fi
expect "it serves the renewed certificate" "$(openssl x509 -noout -serial -in "$D/server.crt")" "$(served)"
touch "$D/go"
like "the SLEEP(25) that was running" "OperationalError 2013: Lost connection to MySQL server during query" "$(logged sleep)"
like "the idle pooled connection, its next statement" "OperationalError 2013" "$(logged idle)"
like "the open transaction, its COMMIT" "OperationalError 2013" "$(logged tx)"
wait "$HOLD"
expect "SQL-committed rows survive, the open transaction's insert is gone" "1,2" "$(py q qd_api api 'SELECT group_concat(id ORDER BY id) FROM t')"
expect "the Dolt commits survive" 2 "$(py q qd_api api 'SELECT count(*) FROM dolt_log')"
expect "the working set (row 2, SQL-committed, not Dolt-committed) survives" "t modified" "$(py q qd_api api "SELECT concat(table_name, ' ', status) FROM dolt_status")"
expect "after the restart, no other client" none "$(py ps qd_agent agent)"

echo "== (d) A stop (SIGTERM) under a running statement"
py q qd_api api 'SELECT SLEEP(20)' >"$D/stop.out" &
SLEEPER=$!
sleep 2
c=$(docker logs "$NAME" 2>&1 | grep -c 'Caught Ctrl+C')
t0=$(date +%s.%N)
docker stop -t 30 "$NAME" >/dev/null
SECS=$(echo "$(date +%s.%N) - $t0" | bc)
wait "$SLEEPER"
if [ "$(echo "$SECS < 3" | bc)" = 1 ]; then ok "docker stop -t 30 returns in $(printf %.1f "$SECS")s: no wait for the statement"; else ko "docker stop took ${SECS}s"; fi
like "the statement's client" "OperationalError 2013" "$(cat "$D/stop.out")"
expect "the entrypoint passed it on: Caught Ctrl+C lines, one more" $((c + 1)) "$(docker logs "$NAME" 2>&1 | grep -c 'Caught Ctrl+C, shutting down Dolt server')"

[ $FAILED = 0 ] && echo "ALL OK" || echo "SOME FAILED"
exit $FAILED
