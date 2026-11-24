"""The percentile half of tools/verify/trino_sql_semantics.py (decision
0090, T1): DuckDB's quantile_cont and median against one portable window
SQL (distinct values, running counts, linear interpolation) run on both
engines, compared bit for bit; then Trino's approx_percentile against the
exact values on 10^6 and 10^7 generated values, and what it does to a PSI.
Synthetic values only.
"""

import math
import random
from fractions import Fraction

PS = [k / 100 for k in range(101)]

# The portable form: values must be in a table `v(x)`; percentiles in `p(p)`.
WINDOW = """
WITH d AS (SELECT x, count(*) AS c FROM {t} WHERE x IS NOT NULL GROUP BY x),
r AS (SELECT x, sum(c) OVER (ORDER BY x ASC ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW) - c AS lo,
             sum(c) OVER (ORDER BY x ASC ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW) AS hi FROM d),
n AS (SELECT sum(c) AS n FROM d),
h AS (SELECT p, (CAST(n AS DOUBLE) - CAST(1 AS DOUBLE)) * p AS h FROM {p}, n),
b AS (SELECT p, h, floor(h) AS i, ceil(h) AS j FROM h)
SELECT b.p, CASE WHEN b.i = b.j THEN a.x
                 ELSE a.x * (CAST(1 AS DOUBLE) - (b.h - b.i)) + z.x * (b.h - b.i) END AS q
  FROM b JOIN r a ON b.i >= a.lo AND b.i < a.hi JOIN r z ON b.j >= z.lo AND b.j < z.hi
 ORDER BY b.p ASC
"""


def vectors():
    rnd = random.Random(90)
    v = {f"n={n}": [rnd.uniform(-1e3, 1e3) for _ in range(n)] for n in (1, 2, 3, 4, 101, 1000)}
    v["ties"] = [float(rnd.choice([1, 1, 1, 2, 2, 3, 5, 8])) for _ in range(57)]
    v["negatives"] = [-abs(rnd.gauss(0, 1e6)) for _ in range(40)]
    v["tiny and huge"] = [1e-300, 5e-324, 1e300, -1e300, 0.0, -0.0, 0.1, 0.2, 0.3]
    v["NaN"] = [1.0, 2.0, float("nan"), 3.0, float("nan")]
    v["+Infinity"] = [1.0, float("inf"), 2.0, 3.0]
    v["-Infinity"] = [float("-inf"), 1.0, 2.0, float("-inf")]
    v["both infinities"] = [float("-inf"), float("inf")]
    return v


def lit(x):
    return f"CAST('{repr(x) if math.isfinite(x) else ('NaN' if x != x else ('Infinity' if x > 0 else '-Infinity'))}' AS DOUBLE)"


def load(trino, duck, name, xs):
    t = "v_" + "".join(c if c.isalnum() else "_" for c in name)
    rows = ", ".join(f"({lit(x)})" for x in xs)
    duck(f"CREATE OR REPLACE TABLE {t} AS SELECT * FROM (VALUES {rows}) s(x)")
    try:
        trino(f"DROP TABLE IF EXISTS {t}")
        trino(f"CREATE TABLE {t} AS SELECT * FROM (VALUES {rows}) s(x)")
    except RuntimeError as e:
        print(f"[FAIL] loading {name} in trino: {e}")
    return t


def canon(h):
    """NaN's sign bit differs between the engines; any NaN is one answer."""
    if h is None or len(h) != 16 or h.startswith("ERROR"):
        return h
    return "nan" if int(h, 16) & 0x7FFFFFFFFFFFFFFF > 0x7FF0000000000000 else h


def ulps(a, b):
    def key(h):
        i = int(h, 16)
        return i if i < 1 << 63 else (1 << 63) - i
    return abs(key(a) - key(b)) if "nan" not in (a, b) else (0 if a == b else -1)


def fused(xs, bits):
    """quantile_cont's value as lo * (1 - d) + hi * d with the last step fused (one rounding)."""
    s, n, out = sorted(xs), len(xs), []
    for p in PS:
        h = (n - 1) * p
        i, j = math.floor(h), math.ceil(h)
        if i == j or not all(math.isfinite(v) for v in (s[i], s[j])):
            out.append(None)
            continue
        d = h - i
        out.append(canon(bits(float(Fraction(s[i]) * Fraction(1 - d) + Fraction(s[j] * d)))))
    return out


def exact(trino, duck, bits, failed):
    print("== quantile_cont and median against the window SQL, bit for bit (NaN of either sign is one answer)")
    p = ", ".join(f"(CAST({k} AS DOUBLE) / CAST(100 AS DOUBLE))" for k in range(101))
    duck(f"CREATE OR REPLACE TABLE p AS SELECT * FROM (VALUES {p}) s(p)")
    trino("DROP TABLE IF EXISTS p")
    trino(f"CREATE TABLE p AS SELECT * FROM (VALUES {p}) s(p)")
    plist = "[" + ", ".join(repr(x) for x in PS) + "]"
    for name, xs in vectors().items():
        t = load(trino, duck, name, xs)
        qc = [canon(bits(x)) for x in duck(f"SELECT quantile_cont(x, {plist}) FROM {t}")[0][0]]
        med = canon(bits(duck(f"SELECT median(x) FROM {t}")[0][0]))
        dw = [canon(bits(r[1])) for r in duck(WINDOW.format(t=t, p="p"))]
        try:
            tw = [canon(r[0].lower()) for r in
                  trino(f"SELECT to_hex(to_ieee754_64(q)) FROM ({WINDOW.format(t=t, p='p')}) ORDER BY p")]
        except RuntimeError as e:
            tw = [f"ERROR {e}"]
        engines = [i for i in range(101) if i >= len(dw) or i >= len(tw) or dw[i] != tw[i]]
        tag = "OK  " if not engines else "FAIL"
        print(f"[{tag}] {name}: window SQL, duckdb = trino on 101 percentiles"
              f"{'' if not engines else f'; {len(engines)} differ, first p={PS[engines[0]]}'}")
        if engines:
            failed.append(f"window SQL across engines, {name}")
        off = [i for i in range(min(101, len(dw))) if qc[i] != dw[i]]
        fu = fused(xs, bits)
        explained = all(fu[i] == qc[i] for i in off if fu[i] is not None)
        worst = max((ulps(qc[i], dw[i]) for i in off), default=0)
        tag = "OK  " if not off and med == qc[50] else "FAIL"
        if tag == "FAIL":
            failed.append(f"quantile_cont against the window SQL, {name}")
        print(f"[{tag}] {name}: quantile_cont = window SQL on {101 - len(off)} of 101"
              f"{'' if not off else f' (largest gap {worst} ulp; quantile_cont equals the fused form on all of them: {explained})'}"
              f"; median = quantile_cont(0.5): {med == qc[50]}")


GEN = """SELECT CAST(((i * 2654435761) % 4294967296) AS DOUBLE) / CAST(4294967296 AS DOUBLE)
        * CAST(((i * 40503) % 65536) AS DOUBLE) AS x FROM {src}"""


def psi(duck, edges, ref="ref", cur="cur"):
    cases = " ".join(f"WHEN x <= {e!r} THEN {i}" for i, e in enumerate(edges))
    def share(t):
        rows = duck(f"SELECT CASE {cases} ELSE {len(edges)} END AS b, count(*) FROM {t} GROUP BY b")
        n = sum(c for _, c in rows)
        return {b: c / n for b, c in rows}
    a, b = share(ref), share(cur)
    return sum((b.get(k, 1e-6) - a.get(k, 1e-6)) * math.log(b.get(k, 1e-6) / a.get(k, 1e-6))
               for k in set(a) | set(b))


def approximate(trino, duck):
    print("== approx_percentile against exact percentiles, and a PSI over deciles")
    for n in (10**6, 10**7):
        duck(f"CREATE OR REPLACE TABLE ref AS {GEN.format(src=f'range(1, {n + 1}) s(i)')}")
        duck("CREATE OR REPLACE TABLE cur AS SELECT x * 1.05 + 0.1 AS x FROM ref")
        src = f"UNNEST(sequence(1, {n // 1000})) a(u) CROSS JOIN UNNEST(sequence(0, 999)) b(w) CROSS JOIN UNNEST(ARRAY[(u - 1) * 1000 + w + 1]) c(i)"
        plist = ", ".join(repr(x) for x in PS[1:-1])
        try:
            approx = trino(f"SELECT approx_percentile(x, ARRAY[{plist}]) FROM ({GEN.format(src=src)})")[0][0]
            tcount = trino(f"SELECT count(*), sum(x) FROM ({GEN.format(src=src)})")[0]
        except RuntimeError as e:
            print(f"[FAIL] n={n}: trino: {e}")
            continue
        ex = duck(f"SELECT quantile_cont(x, [{plist}]) FROM ref")[0][0]
        dcount = duck("SELECT count(*), sum(x) FROM ref")[0]
        print(f"[INFO] n={n}: same data in both engines: trino count {tcount[0]} sum {tcount[1]!r}, duckdb {dcount[0]} {dcount[1]!r}")
        rank = []
        for p, a in zip(PS[1:-1], approx):
            below = duck(f"SELECT count(*) FROM ref WHERE x <= {a!r}")[0][0]
            rank.append(abs(below / n - p))
        rel = max(abs(a - e) / max(abs(e), 1e-12) for a, e in zip(approx, ex))
        deciles = [9, 19, 29, 39, 49, 59, 69, 79, 89]
        d_exact, d_approx = psi(duck, [ex[i] for i in deciles]), psi(duck, [approx[i] for i in deciles])
        print(f"[INFO] n={n}: approx_percentile p1..p99: max rank error {max(rank):.5f}, "
              f"max relative value error {rel:.2e}; PSI exact {d_exact:.6f}, approx {d_approx:.6f}, "
              f"|dPSI| {abs(d_exact - d_approx):.2e}")


def run(trino, duck, bits, failed):
    exact(trino, duck, bits, failed)
    approximate(trino, duck)
