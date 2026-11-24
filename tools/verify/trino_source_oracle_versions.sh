#!/bin/bash
# Verification for decision 0090, T19: Trino 484's oracle connector on
# Oracle 23 (gvenzl/oracle-free:23-slim) and Oracle 19c. Schemas and
# tables listed, system schemas hidden; a table owned by another user than
# the connected one; every type of the probe table under IGNORE and
# CONVERT_TO_VARCHAR: unconstrained NUMBER (refused by an Iceberg table,
# read through a varchar cast), DATE as timestamp(0), CLOB, RAW, INTERVAL,
# LONG, BFILE, XMLTYPE; the read-only user refusing writes; an extraction
# into Iceberg bronze with explicit casts; @@VERSION's equivalent and
# ALL_TAB_COLUMNS through system.query. TLS (TCPS with a wallet) is not
# covered here yet.
#
# NEEDS A PERSON before it runs: (1) decision 0090 Open 2, the ojdbc17
# licence in Trino's image, read and recorded first; (2) for 19c, Oracle's
# licence accepted on container-registry.oracle.com and the image named in
# ORACLE_IMAGE (tag to confirm), with ORACLE_SERVICE=ORCLPDB1 and
# ORACLE_PW_VAR=ORACLE_PWD. It is not run by agents.
#
# One Oracle, one RustFS and one Trino (2 GB heap, dynamic catalogs in
# memory) on a network of their own, no port published; synthetic rows;
# everything removed on exit. Run from the repository root:
#
#   bash tools/verify/trino_source_oracle_versions.sh
set -u
export LC_ALL=C
cd "$(dirname "$0")/../.."
P=qd-verify-t19
. tools/verify/trino_engine_lib.sh
. tools/verify/trino_source_lib.sh
IMAGE=${ORACLE_IMAGE:-gvenzl/oracle-free:23-slim}
SERVICE=${ORACLE_SERVICE:-FREEPDB1}
PW_VAR=${ORACLE_PW_VAR:-ORACLE_PASSWORD}
O=$P-oracle
printf '%s=Qs%s\n' "$PW_VAR" "$(openssl rand -hex 12)" >"$D/oracle.env"
printf 'Qr%s' "$(openssl rand -hex 12)" >"$D/reader.pw"
ora() { # SQL as system, in the PDB
  docker exec -i --env-file "$D/oracle.env" -e PW_VAR="$PW_VAR" "$O" sh -c \
    'eval "pw=\$$PW_VAR"; sqlplus -s -L "system/$pw@//localhost/'"$SERVICE"'"' <<SQL
WHENEVER SQLERROR EXIT FAILURE
SET HEADING OFF FEEDBACK OFF PAGESIZE 0 LINESIZE 300
$1
EXIT
SQL
}
te_rustfs || exit 1
TRINO_CONFIG=$DYNAMIC te_trino || exit 1
out=$(te_dynamic_lake) || { ko "the lake catalog: $out"; exit 1; }
docker run -d --name "$O" --network "$P" --memory 3g --env-file "$D/oracle.env" "$IMAGE" >/dev/null || exit 1
for _ in $(seq 1 150); do ora "SELECT 1 FROM dual;" >/dev/null 2>&1 && break; sleep 4; done
info "Oracle: $(ora "SELECT banner_full FROM v\$version;" | head -1)"
ora "CREATE USER ops IDENTIFIED BY \"$(openssl rand -hex 12)\" QUOTA UNLIMITED ON users;
CREATE USER reader IDENTIFIED BY \"$(cat "$D/reader.pw")\";
GRANT CREATE SESSION TO reader;
CREATE TABLE ops.types_probe (id NUMBER(10) PRIMARY KEY, c_num NUMBER, c_num_ps NUMBER(12, 2),
  c_date DATE, c_ts TIMESTAMP(9), c_tstz TIMESTAMP(6) WITH TIME ZONE, c_clob CLOB, c_raw RAW(8),
  c_iym INTERVAL YEAR TO MONTH, c_ids INTERVAL DAY TO SECOND, c_long LONG, c_bfile BFILE,
  c_xml XMLTYPE, c_v2 VARCHAR2(40), c_nchar NCHAR(5), c_float BINARY_DOUBLE);
INSERT INTO ops.types_probe VALUES (1, 12345.678901234567890123, 1234.56, DATE '2026-11-24',
  TIMESTAMP '2026-11-24 10:00:00.123456789', TIMESTAMP '2026-11-24 10:00:00 -05:00', 'synthetic clob',
  HEXTORAW('CAFE'), INTERVAL '1-2' YEAR TO MONTH, INTERVAL '3 04:05:06' DAY TO SECOND, 'synthetic long',
  NULL, XMLTYPE('<a>synthetic</a>'), 'año ñ', 'ab', 0.1);
COMMIT;
GRANT SELECT ON ops.types_probe TO reader;" >/dev/null || { ko "the probe table"; exit 1; }
URL="jdbc:oracle:thin:@//$O:1521/$SERVICE"
for c in ignore varchar; do
  h=IGNORE; [ $c = varchar ] && h=CONVERT_TO_VARCHAR
  tq qd-quality "CREATE CATALOG ora_$c USING oracle WITH (\"connection-url\" = '$URL',
    \"connection-user\" = 'reader', \"connection-password\" = '$(cat "$D/reader.pw")',
    \"unsupported-type-handling\" = '$h')" >/dev/null
  tq qd-quality "DESCRIBE ora_$c.ops.types_probe" | cut -f1,2 | tr '\t' ' ' >"$D/cols-$c"
  info "columns under $c: $(paste -sd, "$D/cols-$c")"
done
S=ora_varchar
schemas=$(tq qd-quality "SHOW SCHEMAS FROM $S" | paste -sd' ')
info "schemas: $schemas"
expect "system schemas hidden" "" "$(printf '%s\n' $schemas | grep -xE 'sys|system|xdb|ctxsys|mdsys' | paste -sd' ')"
expect "a table owned by another user listed" "types_probe" "$(tq qd-quality "SHOW TABLES FROM $S.ops")"
expect_match "DATE read as timestamp(0)" "^c_date timestamp\(0\)" "$(grep '^c_date ' "$D/cols-varchar")"
info "unconstrained NUMBER: $(grep '^c_num ' "$D/cols-varchar")"
tq_ok "the probe row read whole" qd-quality "SELECT * FROM $S.ops.types_probe"
tq_refused "the reader cannot INSERT" qd-quality "INSERT INTO $S.ops.types_probe (id) VALUES (2)"
tq_refused "the reader cannot CREATE TABLE" qd-quality "CREATE TABLE $S.ops.t2 (a int)"
tq_ok "the version through system.query" qd-quality \
  "SELECT * FROM TABLE($S.system.query(query => 'SELECT banner FROM v\$version'))"
expect "ALL_TAB_COLUMNS through system.query" "16" "$(tq qd-quality "SELECT count(*) FROM TABLE($S.system.query(
  query => 'SELECT column_name FROM all_tab_columns WHERE owner = ''OPS'' AND table_name = ''TYPES_PROBE'''))")"
tq_ok "bronze schema" qd-quality "CREATE SCHEMA lake.ora_bronze"
tq_refused "an Iceberg table of the unconstrained NUMBER as Trino types it" qd-quality \
  "CREATE TABLE lake.ora_bronze.num_raw AS SELECT c_num FROM $S.ops.types_probe"
tq_ok "the same through a varchar cast" qd-quality \
  "CREATE TABLE lake.ora_bronze.num_text AS SELECT CAST(c_num AS varchar) AS c_num FROM $S.ops.types_probe"
tq_ok "  read back" qd-quality "SELECT c_num FROM lake.ora_bronze.num_text"
tq_ok "an extraction with explicit casts" qd-quality "CREATE TABLE lake.ora_bronze.probe AS SELECT
  CAST(id AS bigint) AS id, CAST(c_num AS varchar) AS c_num, c_num_ps, CAST(c_date AS timestamp(6)) AS c_date,
  CAST(c_ts AS timestamp(6)) AS c_ts, CAST(c_tstz AS timestamp(6) with time zone) AS c_tstz,
  CAST(c_v2 AS varchar) AS c_v2, 'job-1' AS _load FROM $S.ops.types_probe"
[ "$FAILED" = 0 ] && echo "ALL OK" || echo "SOME CHECKS FAILED"
exit "$FAILED"
