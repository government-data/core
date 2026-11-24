#!/bin/bash
# Verification for decision 0090, T1: where DuckDB 2.0.0 and Trino 484
# agree and differ on the SQL the rules compile to. Boolean to integer and
# IF; decimal literals against DOUBLE '…'; greatest with NULL; trim with
# one and two arguments and with BOTH set FROM, on space, tab, newline and
# U+00A0; CAST to varchar and TRY_CAST of a text corpus; substr on
# combining marks and emoji; to_hex's case; sha256 and HMAC against
# reference vectors (FIPS 180-2, RFC 4231); regexp_like on 'a\nb' and (?i)
# on U+212A, with Trino on RE2J and the Joni probe; DuckDB's quantile_cont
# and median against one portable window SQL on both engines, bit for bit;
# approx_percentile's error on 10^6 and 10^7 values and on a PSI.
#
# One throwaway Trino 484 node (2 GB heap, memory catalog only, its HTTP
# port on a random port of 127.0.0.1) on a network of its own; DuckDB in
# process. Synthetic values only; everything removed on exit. Run from the
# repository root:
#
#   bash tools/verify/trino_sql_semantics.sh
#
# Needs docker and uv.
set -u
export LC_ALL=C
cd "$(dirname "$0")/../.."
P=qs-verify-t1
. tools/verify/trino_engine_lib.sh
TRINO_PUBLISH=1 TRINO_CONFIG='regex-library=RE2J' te_trino || exit 1
TRINO_URL=$(te_trino_url) uv run -q --no-project --with duckdb==2.0.0 \
  python3 -I tools/verify/trino_sql_semantics.py
