"""Decision 0090: which formatter writes a DOUBLE as the same text on
DuckDB and Trino (driven by trino_number_text.sh). Prints, per candidate,
how many of the corpus' values agree, and the first that differ."""

import json
import os
import random
import struct
import sys
import time
import urllib.request

import duckdb

TRINO = os.environ["TRINO_URL"]
DUCK = duckdb.connect()


def trino(sql):
    req = urllib.request.Request(f"{TRINO}/v1/statement", data=sql.encode(),
                                 headers={"X-Trino-User": "verify", "X-Trino-Catalog": "memory",
                                          "X-Trino-Schema": "default"})
    doc = json.load(urllib.request.urlopen(req, timeout=60))
    rows = []
    while True:
        rows += doc.get("data") or []
        if "error" in doc:
            raise RuntimeError(f"{doc['error'].get('errorName')}: {doc['error'].get('message')}")
        if "nextUri" not in doc:
            return rows
        time.sleep(0.02)
        doc = json.load(urllib.request.urlopen(doc["nextUri"], timeout=600))


def corpus():
    rnd = random.Random(90)
    vals = [0.0, -0.0, 1.0, -1.0, 0.1, 0.2, 0.3, 0.5, 1.5, 2.5, 1e-4, 1e-5, 123456.789,
            100.0, 1e15, 1e16, 1e17, 1e21, 1e22, 9007199254740993.0, 5e-324, 2.2250738585072014e-308,
            1.7976931348623157e308, 3.14159, 0.000123, 1234.5, 99.995, 0.045, 12.0]
    vals += [float(i) for i in range(-50, 51)]
    vals += [round(rnd.uniform(-1e6, 1e6), rnd.randint(0, 6)) for _ in range(400)]
    vals += [rnd.uniform(-1, 1) * 10 ** rnd.randint(-30, 30) for _ in range(300)]
    while len(vals) < 1300:
        v = struct.unpack(">d", rnd.getrandbits(64).to_bytes(8, "big"))[0]
        if v == v and abs(v) != float("inf"):
            vals.append(v)
    return vals


CANDIDATES = {
    "%.17g": ("printf('%.17g', x)", "format('%.17g', x)"),
    "%.15g": ("printf('%.15g', x)", "format('%.15g', x)"),
    "%.17e": ("printf('%.17e', x)", "format('%.17e', x)"),
    "%.15e": ("printf('%.15e', x)", "format('%.15e', x)"),
    "decimal(38,10)": ("CAST(TRY_CAST(x AS DECIMAL(38, 10)) AS VARCHAR)",
                       "CAST(TRY_CAST(x AS decimal(38, 10)) AS varchar)"),
    "cast varchar": ("CAST(x AS VARCHAR)", "CAST(x AS varchar)"),
}


def main():
    print("DuckDB", duckdb.__version__, "| Trino", trino("SELECT version()")[0][0])
    vals = corpus()
    hexes = [struct.pack(">d", v).hex() for v in vals]
    DUCK.execute("CREATE TABLE c (i INTEGER, x DOUBLE)")
    DUCK.executemany("INSERT INTO c VALUES (?, ?)", list(enumerate(vals)))
    back = [r[0] for r in DUCK.execute("SELECT x FROM c ORDER BY i").fetchall()]
    assert [struct.pack(">d", v) for v in back] == [struct.pack(">d", v) for v in vals]
    tvalues = ", ".join(f"({i}, from_ieee754_64(from_hex('{h}')))" for i, h in enumerate(hexes))
    agree_any = False
    for name, (dsql, tsql) in CANDIDATES.items():
        try:
            d = dict(DUCK.execute(f"SELECT i, {dsql} FROM c ORDER BY i").fetchall())
            t = dict(trino(f"SELECT i, {tsql} FROM (VALUES {tvalues}) AS v (i, x) ORDER BY i"))
        except Exception as e:  # noqa: BLE001 - a candidate one engine lacks
            print(f"{name}: refused: {str(e)[:200]}")
            continue
        diff = [i for i in range(len(vals)) if d.get(i) != t.get(i)]
        ok = len(vals) - len(diff)
        print(f"{name}: {ok}/{len(vals)} agree" + ("" if diff else "  ALL OK"))
        for i in diff[:5]:
            print(f"    {vals[i]!r}: duckdb {d.get(i)!r} trino {t.get(i)!r}")
        agree_any |= not diff
    # The shortest round-trip digits: each engine's CAST to text, read as a
    # decimal number (sign, digits, exponent), against Python's repr.
    import decimal
    d = dict(DUCK.execute("SELECT i, CAST(x AS VARCHAR) FROM c ORDER BY i").fetchall())
    t = dict(trino(f"SELECT i, CAST(x AS varchar) FROM (VALUES {tvalues}) AS v (i, x) ORDER BY i"))
    def shortest(text):
        return decimal.Decimal(text).normalize().as_tuple()
    diff = [i for i, v in enumerate(vals)
            if not (shortest(d[i]) == shortest(t[i]) == shortest(repr(v)))]
    print(f"shortest digits of CAST: {len(vals) - len(diff)}/{len(vals)} agree"
          + ("" if diff else "  ALL OK"))
    for i in diff[:5]:
        print(f"    {vals[i]!r}: duckdb {d[i]!r} trino {t[i]!r}")
    exact_ok = exact_casts()
    print("RESULT:", "a candidate agrees on every value" if agree_any else "no candidate agrees on every value")
    print("RESULT (integers and decimals):", "ALL OK" if exact_ok else "they differ")
    # Ruling 26: floating point has no shape; what must agree is the exact
    # types' cast.
    sys.exit(0 if exact_ok else 1)


EXACT_TYPES = ["tinyint", "smallint", "integer", "bigint", "decimal(5,0)", "decimal(9,2)",
               "decimal(18,4)", "decimal(38,10)", "decimal(38,0)", "decimal(4,4)"]


def exact_corpus(ty, rnd):
    if not ty.startswith("decimal"):
        bits = {"tinyint": 7, "smallint": 15, "integer": 31, "bigint": 63}[ty]
        lo, hi = -(2 ** bits), 2 ** bits - 1
        vals = [0, 1, -1, 10, -10, 100, lo, hi, lo + 1, hi - 1]
        vals += [rnd.randint(lo, hi) for _ in range(200)]
        return [str(v) for v in vals]
    p, sc = (int(x) for x in ty[8:-1].split(","))
    def make(ip, fp, neg):
        text = ip + ("." + fp if sc else "")
        return ("-" if neg else "") + text
    out = []
    for neg in (False, True):
        out.append(make("0", "0" * sc, neg))
        out.append(make("9" * (p - sc) if p > sc else "0", "9" * sc, neg))
        if sc:
            out.append(make("0", "0" * (sc - 1) + "1", neg))
            out.append(make("1" if p > sc else "0", "5" + "0" * (sc - 1), neg))
    for _ in range(200):
        ip = str(rnd.randint(0, 10 ** (p - sc) - 1)) if p > sc else "0"
        fp = str(rnd.randint(0, 10 ** sc - 1)).zfill(sc) if sc else ""
        out.append(make(ip, fp, rnd.random() < 0.5))
    return out


def exact_casts():
    """Integers and decimals: CAST to text on both engines, value by value."""
    rnd = random.Random(26)
    all_ok = True
    for ty in EXACT_TYPES:
        vals = exact_corpus(ty, rnd)
        dty = ty.upper()
        d = [r[0] for r in DUCK.execute(
            "SELECT CAST(CAST(v AS %s) AS VARCHAR) FROM (SELECT unnest(?) AS v, "
            "generate_subscripts(?, 1) AS i) ORDER BY i" % dty, [vals, vals]).fetchall()]
        rows = ", ".join(f"({i}, CAST('{v}' AS {ty}))" for i, v in enumerate(vals))
        t = [r[1] for r in trino(f"SELECT i, CAST(x AS varchar) FROM (VALUES {rows}) AS v (i, x) ORDER BY i")]
        diff = [i for i in range(len(vals)) if d[i] != t[i]]
        print(f"{ty}: {len(vals) - len(diff)}/{len(vals)} agree" + ("" if diff else "  ALL OK"))
        for i in diff[:5]:
            print(f"    {vals[i]}: duckdb {d[i]!r} trino {t[i]!r}")
        all_ok &= not diff
    return all_ok


if __name__ == "__main__":
    main()
