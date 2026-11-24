# shellcheck shell=bash
# SQL Server helpers of decision 0090's extraction scripts (T18, T20),
# sourced after trino_engine_lib.sh: a throwaway SQL Server with TLS from a
# self-signed certificate made here (encryption forced), a database `src`
# with a read-only login, and Trino catalogs made with CREATE CATALOG
# (te_trino with TRINO_CONFIG=$DYNAMIC). Passwords are random, kept in
# files under "$D" and in the containers' environment, never printed.
# Not run by itself.

DYNAMIC=$'catalog.management=dynamic\ncatalog.store=memory'
# ms_start VERSION: SQL Server VERSION (2017, 2019) as "$P-ms<VERSION>".
ms_start() {
  local v=$1 name="$P-ms$1" dir="$D/ms$1"
  mkdir -p "$dir"
  printf 'MSSQL_SA_PASSWORD=Qd-%s!a1\nACCEPT_EULA=Y\nMSSQL_PID=Developer\n' "$(openssl rand -hex 12)" >"$dir/sa.env"
  printf 'Qr-%s!b2' "$(openssl rand -hex 12)" >"$dir/reader.pw"
  openssl req -x509 -newkey rsa:2048 -nodes -days 2 -subj "/CN=$name" -addext "subjectAltName=DNS:$name" \
    -addext "extendedKeyUsage=serverAuth" -keyout "$dir/tls.key" -out "$dir/tls.crt" 2>/dev/null
  printf '[network]\ntlscert = /certs/tls.crt\ntlskey = /certs/tls.key\ntlsprotocols = 1.2\nforceencryption = 1\n' \
    >"$dir/mssql.conf"
  chmod 755 "$dir"
  chmod 644 "$dir"/tls.* "$dir/mssql.conf"
  docker run -d --name "$name" --network "$P" --memory 2g --env-file "$dir/sa.env" \
    -v "$dir/tls.crt:/certs/tls.crt:ro" -v "$dir/tls.key:/certs/tls.key:ro" \
    -v "$dir/mssql.conf:/var/opt/mssql/mssql.conf:ro" \
    "mcr.microsoft.com/mssql/server:$v-latest" >/dev/null || return 1
  for _ in $(seq 1 90); do
    ms "$v" "SELECT 1" >/dev/null 2>&1 && return 0
    sleep 2
  done
  echo "SQL Server $v did not start:"; docker logs "$name" 2>&1 | tail -15
  return 1
}
# ms VERSION SQL: SQL as sa inside the container (sqlcmd of the image).
ms() {
  docker exec --env-file "$D/ms$1/sa.env" "$P-ms$1" sh -c '
    t=/opt/mssql-tools18/bin/sqlcmd; [ -x $t ] || t=/opt/mssql-tools/bin/sqlcmd
    SQLCMDPASSWORD=$MSSQL_SA_PASSWORD $t -C -S localhost -U sa -b -h -1 -W -s "|" -Q "SET NOCOUNT ON; $0"' "$2"
}
# ms_reader VERSION: database src, schema ops and the read-only login reader.
ms_reader() {
  ms "$1" "CREATE DATABASE src" &&
    ms "$1" "CREATE LOGIN reader WITH PASSWORD = '$(cat "$D/ms$1/reader.pw")', CHECK_POLICY = OFF" &&
    ms "$1" "USE src; EXEC('CREATE SCHEMA ops'); CREATE USER reader FOR LOGIN reader;
      ALTER ROLE db_datareader ADD MEMBER reader"
}
# ms_truststore VERSION: the certificate as a JKS in Trino's catalog folder.
ms_truststore() {
  cp "$D/ms$1/tls.crt" "$D/catalog/ms$1.crt"
  chmod 755 "$D" && chmod 777 "$D/catalog"
  docker run --rm -u root -v "$D/catalog:/work" --entrypoint keytool "$TRINO_IMAGE" -importcert -noprompt \
    -alias "ms$1" -file "/work/ms$1.crt" -keystore "/work/ms$1.jks" -storepass changeit >/dev/null 2>&1
  docker run --rm -u root -v "$D/catalog:/work" --entrypoint chmod "$TRINO_IMAGE" 644 "/work/ms$1.jks"
  chmod 755 "$D/catalog" && chmod 700 "$D"
}
# ms_catalog NAME VERSION URL_OPTIONS [PROPERTIES]: CREATE CATALOG as the reader.
ms_catalog() {
  local url="jdbc:sqlserver://$P-ms$2:1433;databaseName=src;$3"
  tq qd-quality "CREATE CATALOG $1 USING sqlserver WITH (\"connection-url\" = '$url',
    \"connection-user\" = 'reader', \"connection-password\" = '$(cat "$D/ms$2/reader.pw")'${4:+, $4})"
}
TLS_TRUST='encrypt=true;trustServerCertificate=false'
tls_store() { printf '%s;trustStore=/etc/trino/catalog/ms%s.jks;trustStorePassword=changeit' "$TLS_TRUST" "$1"; }
# te_dynamic_lake: the `lake` catalog made with CREATE CATALOG (with
# dynamic catalogs in memory Trino does not load the catalog folder).
te_dynamic_lake() {
  local props
  props=$(grep -v '^connector.name\|^$' "$D/catalog/lake.properties" |
    sed "s/^\([^=]*\)=\(.*\)$/\"\1\" = '\2'/" | paste -sd, | sed 's/,/, /g')
  tq qd-quality "CREATE CATALOG lake USING iceberg WITH ($props)"
}
