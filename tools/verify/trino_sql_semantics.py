"""The checks of tools/verify/trino_sql_semantics.sh (decision 0090, T1).

Runs each expression on DuckDB (in process) and on the throwaway Trino at
TRINO_URL (HTTP, user `verify`), and prints [SAME] when both engines give
the same answer, [DIFF] when they do not, [OK  ] or [FAIL] where the record
states what must hold. Text is compared as UTF-8 hex, doubles as their IEEE
754 bits. Synthetic values only. Exits 1 if a [FAIL] was printed.
"""

import hashlib
import hmac
import json
import os
import struct
import sys
import time
import urllib.request

import duckdb

TRINO = os.environ["TRINO_URL"]
DUCK = duckdb.connect()
FAILED = []


def trino(sql):
    """Rows of a Trino statement, or raises RuntimeError with its message."""
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


def duck(sql):
    return [list(r) for r in DUCK.execute(sql).fetchall()]


def bits(v):
    return None if v is None else struct.pack(">d", float(v)).hex()


# Per kind, the wrapper each engine needs and how its value is normalised.
WRAP = {
    "text": ("to_hex(to_utf8({}))", "hex(CAST({} AS VARCHAR))", lambda v: v and v.lower()),
    "double": ("to_hex(to_ieee754_64(CAST({} AS double)))", "CAST({} AS DOUBLE)", None),
    "plain": ("CAST({} AS varchar)", "CAST({} AS VARCHAR)", lambda v: v),
    "type": ("typeof({})", "typeof({})", lambda v: v and v.lower()),
}


def one(engine, kind, expr, tsql=None):
    """The normalised answer of one engine, or 'ERROR …'."""
    t, d, norm = WRAP[kind]
    try:
        if engine == "trino":
            v = trino(f"SELECT {t.format(tsql or expr)}")[0][0]
            return v.lower() if kind == "double" and v else (norm(v) if norm else v)
        v = duck(f"SELECT {d.format(expr)}")[0][0]
        return bits(v) if kind == "double" else norm(v)
    except Exception as e:  # noqa: BLE001 - an engine error is an answer here
        return "ERROR " + str(e).split("\n")[0][:140]


def compare(name, kind, expr, tsql=None, want=None):
    """Both engines; want='same' fails on a difference, a value fails on another."""
    a, b = one("duckdb", kind, expr), one("trino", kind, expr, tsql)
    tag = "SAME" if a == b else "DIFF"
    if want == "same" and a != b:
        tag = "FAIL"
    if want not in (None, "same") and (a, b) != (want, want):
        tag = "FAIL"
    if tag == "FAIL":
        FAILED.append(name)
    print(f"[{tag}] {name}: duckdb={a} trino={b}")
    return a, b


def section(title):
    print(f"== {title}")


def versions():
    print("[INFO] duckdb", duckdb.__version__, "| trino", trino("SELECT node_version FROM system.runtime.nodes")[0][0])


def booleans_and_numbers():
    section("boolean to integer, IF, decimal literals against DOUBLE '…', greatest")
    for v in ("true", "false", "CAST(NULL AS BOOLEAN)"):
        compare(f"CAST({v} AS INTEGER)", "plain", f"CAST({v} AS INTEGER)", want="same")
        compare(f"IF({v}, DOUBLE '1', DOUBLE '0')", "double", f"IF({v}, CAST('1' AS DOUBLE), CAST('0' AS DOUBLE))",
                f"IF({v}, DOUBLE '1', DOUBLE '0')", want="same")
    compare("DOUBLE '0.1' typed literal", "double", "DOUBLE '0.1'")
    for e in ("0.1 + 0.2", "1 / 3.0", "0.6745 * 2.5", "365.0 * 0.10", "3.5 / 7", "1.0 - 0.9"):
        compare(f"type of {e}", "type", e)
        compare(f"{e} as a double", "double", e)
        d = " ".join(f"CAST('{w}' AS DOUBLE)" if w[0].isdigit() else w for w in e.split())
        compare(f"{e} with DOUBLE literals", "double", d, want="same")
    compare("greatest(NULL, 0)", "plain", "greatest(NULL, 0)")
    compare("greatest(CAST(NULL AS DOUBLE), DOUBLE 0)", "plain", "greatest(CAST(NULL AS DOUBLE), CAST(0 AS DOUBLE))")


TRIMS = {"space": "' a '", "tab": "chr(9) || 'a' || chr(9)", "newline": "chr(10) || 'a' || chr(10)",
         "nbsp": "chr(160) || 'a' || chr(160)", "mixed": "' ' || chr(160) || ' a' || chr(160) || ' '"}


def trims():
    section("trim: one argument, two arguments, BOTH set FROM")
    for name, v in TRIMS.items():
        compare(f"trim(v) on {name}", "text", f"trim({v})")
        compare(f"trim(v, ' ') on {name}", "text", f"trim({v}, ' ')")
        compare(f"trim(BOTH ' ' FROM v) on {name}", "text", f"trim(BOTH ' ' FROM {v})")
        compare(f"trim(BOTH ' '||chr(160) FROM v) on {name}", "text", f"trim(BOTH concat(' ', chr(160)) FROM {v})")
    compare("trim(LEADING '0' FROM '000120')", "text", "trim(LEADING '0' FROM '000120')")


def casts():
    section("CAST to varchar; TRY_CAST of a text corpus")
    for v in ("0.1", "1.0", "1e20", "1e-7", "123456789.123", "-0.0", "NaN", "Infinity", "100.0", "1e16", "0.5"):
        compare(f"CAST(double {v} AS varchar)", "text", f"CAST(CAST('{v}' AS DOUBLE) AS VARCHAR)")
    compare("CAST(date AS varchar)", "text", "CAST(DATE '2026-01-05' AS VARCHAR)", want="same")
    compare("CAST(true AS varchar)", "text", "CAST(true AS VARCHAR)")
    corpus = ["2026-01-05", "2026-1-5", "20260105", "05/01/2026", " 2026-01-05 ", "2026-02-30", "2026-01-05 10:00",
              "1.5", "1,5", " 1.5", "1e3", "NaN", "nan", "inf", "Infinity", "-Infinity", "", "+1", ".5", "0x10",
              "true", "TRUE", "t", "yes", "1", "0", "false ", "off"]
    for target in ("DATE", "DOUBLE", "BOOLEAN"):
        same = diff = 0
        for text in corpus:
            a, b = (one(e, "plain", f"TRY_CAST('{text}' AS {target})") for e in ("duckdb", "trino"))
            if target == "DOUBLE":
                a, b = (one(e, "double", f"TRY_CAST('{text}' AS DOUBLE)") for e in ("duckdb", "trino"))
            if a == b:
                same += 1
            else:
                diff += 1
                print(f"[DIFF] TRY_CAST('{text}' AS {target}): duckdb={a} trino={b}")
        print(f"[INFO] TRY_CAST to {target}: {same} same, {diff} different of {len(corpus)}")


def strings_and_hashes():
    section("substr on combining marks and emoji; to_hex; sha256 and HMAC")
    for name, v in {"e + U+0301": "'e' || chr(769) || 'x'", "thumbs + skin tone": "chr(128077) || chr(127997) || 'x'",
                    "flag": "chr(127466) || chr(127480) || 'x'", "a emoji b": "'a' || chr(128512) || 'b'"}.items():
        compare(f"length on {name}", "plain", f"length({v})", want="same")
        compare(f"substr(v, 1, 1) on {name}", "text", f"substr({v}, 1, 1)", want="same")
        compare(f"substr(v, 2, 2) on {name}", "text", f"substr({v}, 2, 2)", want="same")
    compare("to_hex of bytes (case)", "plain", "to_hex(CAST('z' AS BLOB))", "to_hex(to_utf8('z'))")
    want = hashlib.sha256(b"abc").hexdigest()
    a = one("duckdb", "plain", "sha256('abc')")
    b = one("trino", "plain", "", "lower(to_hex(sha256(to_utf8('abc'))))")
    for name, got in (("duckdb sha256", a), ("trino sha256 (lower)", b)):
        (print if got == want else fail)(f"[{'OK  ' if got == want else 'FAIL'}] {name} against the reference: {got}")
    key, msg = b"Jefe", b"what do ya want for nothing?"  # RFC 4231, case 2
    want = hmac.new(key, msg, hashlib.sha256).hexdigest()
    b = one("trino", "plain", "", "lower(to_hex(hmac_sha256(to_utf8('what do ya want for nothing?'), to_utf8('Jefe'))))")
    (print if b == want else fail)(f"[{'OK  ' if b == want else 'FAIL'}] trino hmac_sha256 against RFC 4231 case 2: {b}")
    print("[INFO] duckdb hmac:", one("duckdb", "plain", "hmac_sha256('Jefe', 'what do ya want for nothing?')"))


def fail(line):
    FAILED.append(line)
    print(line)


def regexes():
    section("regular expressions (Trino with regex-library=RE2J)")
    b = one("trino", "plain", "", "regexp_like('aa', '(a)\\1')")
    (print if b.startswith("ERROR") else fail)(f"[{'OK  ' if b.startswith('ERROR') else 'FAIL'}] the Joni probe refused: {b}")
    nl = "'a' || chr(10) || 'b'"
    for p in ("^a.b$", "^(?:a)$", "a$", "(?s)^a.b$", "(?m)^b$", "b"):
        compare(f"regexp_like on 'a\\nb', '{p}'", "plain", f"regexp_matches({nl}, '{p}')", f"regexp_like({nl}, '{p}')")
    for v, p in (("chr(8490)", "(?i)^k$"), ("'k'", "(?i)^\\x{212A}$"), ("chr(8490)", "^[A-Za-z]$"), ("'É'", "(?i)^é$")):
        compare(f"regexp on {v}, '{p}'", "plain", f"regexp_matches({v}, '{p}')", f"regexp_like({v}, '{p}')")
    print("[INFO] duckdb regexp_like:", one("duckdb", "plain", "regexp_like('abc', 'b')"))


if __name__ == "__main__":
    versions()
    booleans_and_numbers()
    trims()
    casts()
    strings_and_hashes()
    regexes()
    sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
    import trino_sql_quantiles  # noqa: E402 - the second half, in its own file
    trino_sql_quantiles.run(trino, duck, bits, FAILED)
    print(f"== {len(FAILED)} failed" + (": " + "; ".join(FAILED) if FAILED else ""))
    sys.exit(1 if FAILED else 0)
