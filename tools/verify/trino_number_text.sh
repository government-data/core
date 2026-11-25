#!/bin/bash
# Verification for decision 0090 (the user's ruling of 2026-11-25 on a
# field's profile shape): which fixed formatter writes a DOUBLE as the same
# text on DuckDB 2.0.0 and Trino 484, so a number's shape can be taken from
# that text on both engines. Candidates: printf/format with %.17g, %.15g,
# %.17e and %.15e, and a cast through DECIMAL(38, 10); each over a
# synthetic corpus (integers, decimals, powers of ten, halves, large and
# small, subnormals, negative zero, random bit patterns); then, for ruling
# 26, the plain cast to text of integers (tinyint to bigint) and DECIMALs
# of several precisions and scales, value by value.
#
# One throwaway Trino 484 node (memory catalog only, its HTTP port on a
# random port of 127.0.0.1) on a network of its own; DuckDB in process.
# Synthetic values only; everything removed on exit. Run from the
# repository root:
#
#   bash tools/verify/trino_number_text.sh
#
# Needs docker and uv.
set -u
export LC_ALL=C
cd "$(dirname "$0")/../.."
P=qd-verify-t21
. tools/verify/trino_engine_lib.sh
TRINO_PUBLISH=1 te_trino || exit 1
TRINO_URL=$(te_trino_url) uv run -q --no-project --with duckdb==2.0.0 \
  python3 -I tools/verify/trino_number_text.py
