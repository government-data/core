#!/bin/bash
# Verification for decision 0090, T18: Trino 484's sqlserver connector on
# SQL Server 2017 and 2019 (Linux images, Developer edition): TLS with a
# self-signed certificate (encryption forced by the server; refused
# without the certificate trusted, read with it in a truststore); schemas
# and tables listed; every type of the probe table under IGNORE and
# CONVERT_TO_VARCHAR; the read-only login refusing writes; table
# statistics with and without the rights DBCC SHOW_STATISTICS needs; an
# extraction into Iceberg bronze with explicit casts; @@VERSION and
# sys.columns readable through system.query.
#
# SQL Server 2016 has no Linux image: that part needs a person with a
# Windows server (the same checks, by hand or with this script's SQL).
#
# The versions run one after the other, each removed before the next; one
# RustFS and one Trino (2 GB heap, dynamic catalogs in memory) on a network
# of their own, no port published; synthetic rows; everything removed on
# exit. Run from the repository root:
#
#   bash tools/verify/trino_source_sqlserver_versions.sh [2017] [2019]
#
# Needs docker, openssl and jq.
set -u
export LC_ALL=C
cd "$(dirname "$0")/../.."
P=qd-verify-t18
. tools/verify/trino_engine_lib.sh
. tools/verify/trino_source_lib.sh
VERSIONS=${*:-2017 2019}
te_rustfs || exit 1
TRINO_CONFIG=$DYNAMIC te_trino || exit 1
out=$(te_dynamic_lake) || { ko "the lake catalog: $out"; exit 1; }
tq_ok "Trino" qd-quality "SELECT node_version FROM system.runtime.nodes"
tq_ok "bronze schema" qd-quality "CREATE SCHEMA lake.src_bronze"

PROBE="CREATE TABLE ops.types_probe (id int NOT NULL PRIMARY KEY, c_variant sql_variant,
  c_hier hierarchyid, c_geo geography, c_xml xml, c_guid uniqueidentifier, c_dto datetimeoffset(7),
  c_dt2 datetime2(7), c_dt datetime, c_date date, c_time time(7), c_money money, c_smallmoney smallmoney,
  c_tiny tinyint, c_bit bit, c_dec decimal(38, 10), c_float float, c_real real, c_nvar nvarchar(40),
  c_char char(5), c_bin varbinary(8));
INSERT INTO ops.types_probe VALUES (1, CAST(42 AS sql_variant), hierarchyid::Parse('/1/2/'),
  geography::Point(4.6, -74.1, 4326), '<a>synthetic</a>', '6F9619FF-8B86-D011-B42D-00C04FC964FF',
  '2026-11-24 10:00:00.1234567 -05:00', '2026-11-24 10:00:00.1234567', '2026-11-24 10:00:00.997',
  '2026-11-24', '10:00:00.1234567', 1234.5678, 12.34, 255, 1, 1234567890123456789012345678.0123456789,
  0.1, 0.5, N'año ñ', 'ab', 0xCAFE);
CREATE INDEX ix_tiny ON ops.types_probe (c_tiny);"
# Trino type -> the bronze type the core's type table gives (point 9.4).
bronze_type() {
  case $1 in
    tinyint) echo smallint ;;
    "timestamp("[7-9]")"*"with time zone") echo "timestamp(6) with time zone" ;;
    "timestamp("[7-9]")") echo "timestamp(6)" ;;
    "time("*) echo "time(6)" ;;
    char*) echo varchar ;;
    *) echo "$1" ;;
  esac
}

for V in $VERSIONS; do
  echo "== SQL Server $V"
  ms_start "$V" || { ko "SQL Server $V did not start"; continue; }
  ms_reader "$V" || ko "$V: the database and the reader"
  ms "$V" "USE src; $PROBE" >/dev/null || ko "$V: the probe table"
  info "$V: $(ms "$V" "SELECT @@VERSION" | head -1 | cut -c1-90)"
  ms_truststore "$V"
  out=$(ms_catalog "s${V}_untrusted" "$V" "$TLS_TRUST")
  info "$V: catalog without the certificate trusted: $(printf '%s' "$out" | head -c 120)"
  tq_refused "$V: a read without the certificate trusted" qd-quality "SHOW SCHEMAS FROM s${V}_untrusted" \
    'PKIX|certificat|SSL|TLS|trust'
  ms_catalog "s${V}_plain" "$V" "encrypt=false" >/dev/null
  tq_refused "$V: a read without encryption (forced by the server)" qd-quality "SHOW SCHEMAS FROM s${V}_plain"
  ms_catalog "s${V}_ignore" "$V" "$(tls_store "$V")" "\"unsupported-type-handling\" = 'IGNORE'" >/dev/null
  ms_catalog "s${V}" "$V" "$(tls_store "$V")" "\"unsupported-type-handling\" = 'CONVERT_TO_VARCHAR'" >/dev/null
  expect_match "$V: TLS with the certificate trusted, schemas listed" "ops" "$(tq qd-quality "SHOW SCHEMAS FROM s$V")"
  info "$V: schemas: $(tq qd-quality "SHOW SCHEMAS FROM s$V" | tr '\n' ' ')"
  expect "$V: tables of ops" "types_probe" "$(tq qd-quality "SHOW TABLES FROM s$V.ops")"
  for c in ignore varchar; do
    cat=s$V; [ $c = ignore ] && cat=s${V}_ignore
    tq qd-quality "DESCRIBE $cat.ops.types_probe" | cut -f1,2 | tr '\t' ' ' >"$D/cols-$V-$c"
    info "$V: columns under $c: $(paste -sd, "$D/cols-$V-$c")"
  done
  expect "$V: IGNORE leaves out what CONVERT_TO_VARCHAR converts" "" \
    "$(cut -d' ' -f1 "$D/cols-$V-ignore" | grep -vxFf <(cut -d' ' -f1 "$D/cols-$V-varchar"))"
  info "$V: left out under IGNORE: $(cut -d' ' -f1 "$D/cols-$V-varchar" | grep -vxFf <(cut -d' ' -f1 "$D/cols-$V-ignore") | paste -sd,)"
  tq_ok "$V: the probe row read whole (CONVERT_TO_VARCHAR)" qd-quality "SELECT * FROM s$V.ops.types_probe"
  tq_refused "$V: the reader cannot INSERT" qd-quality "INSERT INTO s$V.ops.types_probe (id) VALUES (2)"
  tq_refused "$V: the reader cannot CREATE TABLE" qd-quality "CREATE TABLE s$V.ops.t2 (a int)"
  tq_refused "$V: the reader cannot DELETE" qd-quality "DELETE FROM s$V.ops.types_probe"
  # stats: c_tiny's distinct values (from the index's statistics) and the row count.
  stats() { printf '%s' "$1" | awk -F'\t' '$1 == "c_tiny" { d = $3 } $1 == "" { r = $5 }
    END { printf "c_tiny distinct=%s rows=%s", (d == "" ? "-" : d), (r == "" ? "-" : r) }'; }
  info "$V: SHOW STATS as the reader: $(stats "$(tq qd-quality "SHOW STATS FOR s$V.ops.types_probe")")"
  ms "$V" "DBCC TRACEON (9485, -1)" >/dev/null 2>&1
  out=$(tq qd-quality "SHOW STATS FOR s$V.ops.types_probe")
  if [ $? -eq 0 ]; then ok "$V: SHOW STATS without DBCC rights (trace flag 9485) degrades: $(stats "$out")"; else
    ko "$V: SHOW STATS without DBCC rights fails: $(printf '%s' "$out" | head -c 160)"; fi
  ms "$V" "DBCC TRACEOFF (9485, -1)" >/dev/null 2>&1
  expect_match "$V: @@VERSION through system.query" "Microsoft SQL Server $V" \
    "$(tq qd-quality "SELECT * FROM TABLE(s$V.system.query(query => 'SELECT @@VERSION AS v'))")"
  expect "$V: sys.columns through system.query" "21" "$(tq qd-quality "SELECT count(*) FROM TABLE(s$V.system.query(
    query => 'SELECT c.name, t.name AS type FROM sys.columns c JOIN sys.types t ON c.user_type_id = t.user_type_id
    WHERE c.object_id = OBJECT_ID(''ops.types_probe'')'))")"

  sel="" cols=""
  while read -r name type; do
    b=$(bronze_type "$type")
    sel="$sel${sel:+, }CAST($name AS $b) AS $name"
    cols="$cols${cols:+, }$name $b"
    [ "$b" = "$type" ] || info "$V: $name $type -> $b"
  done <"$D/cols-$V-varchar"
  tq_ok "$V: bronze table from the type table" qd-quality "CREATE TABLE lake.src_bronze.probe_$V ($cols, _load varchar)"
  tq_ok "$V: extraction with explicit casts" qd-quality \
    "INSERT INTO lake.src_bronze.probe_$V SELECT $sel, 'job-$V' FROM s$V.ops.types_probe"
  tq_ok "$V: read back from bronze" qd-quality "SELECT c_dto, c_dt2, c_tiny, c_dec, c_money, c_guid, c_nvar
    FROM lake.src_bronze.probe_$V"
  docker rm -f -v "$P-ms$V" >/dev/null
done
[ "$FAILED" = 0 ] && echo "ALL OK" || echo "SOME CHECKS FAILED"
exit "$FAILED"
