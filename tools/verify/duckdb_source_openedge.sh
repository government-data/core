#!/bin/bash
# Verification that DuckDB, at the engine the core pins, reads a Progress
# OpenEdge source directly (decision 0077 step 3). DuckDB has no OpenEdge
# extension, core or community; its one path is ODBC: the core extension
# `odbc_scanner` (MIT) over the unixODBC driver manager, with Progress's
# OpenEdge ODBC driver. That driver ships only inside a licensed OpenEdge
# installation, and Progress's database image (progresssoftware/prgs-oedb)
# needs a license code and the acceptance of its EULA on pull. Neither is
# downloaded here. So this script shows what can be shown without them, that
# `odbc_scanner` installs and loads at the pinned engine on linux_amd64, and
# then reports that the source itself cannot be verified, with exit code 2.
# Run alone (no build at the same time), from anywhere:
#
#   bash tools/verify/duckdb_source_openedge.sh
#
# Needs docker and network access (the duckdb package, the extension,
# Debian's unixodbc). Everything is named qs-verify-openedge… and removed on
# exit, also on failure.
set -u
SRC=openedge
# shellcheck source=tools/verify/duckdb_source_lib.sh
. "$(dirname "$0")/duckdb_source_lib.sh"

pinned_engine
net_up
echo "== DuckDB loads the ODBC path (odbc_scanner over unixODBC)"
duckdb_check -e EXT=odbc_scanner -e REPO=core -e ONLY_LOAD=1 \
  -e DEBIAN_FRONTEND=noninteractive -e PREP='apt-get update -qq >/dev/null && apt-get install -y -qq --no-install-recommends unixodbc >/dev/null'
[ "$FAILED" = 0 ] || verdict "OpenEdge's ODBC path"

echo "== CANNOT VERIFY: no OpenEdge server and no OpenEdge ODBC driver can be had" \
  "without a Progress license (the prgs-oedb image needs a license code and its EULA" \
  "accepted on pull; the ODBC driver ships only with a licensed OpenEdge)"
exit 2
