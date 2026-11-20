"""Stand-ins for decision 0073's verification (grafana_*.sh): Quality Data's
OIDC provider and pages ("app") and its API's chart routes ("api"), behind
Traefik, which strips /api as the real api-chain does. Standard library only;
the people, datasets and rows are synthetic. Tokens are HS256 JWTs signed with
a throwaway key ($KEY_FILE); nothing here prints or logs a token, only header
names and what the check found.

    python3 grafana_quality_standins.py app|api

Environment: KEY_FILE, PEOPLE_FILE (JSON: person -> policy, grafana_orgs,
datasets), LOG_FILE (one JSON line per request), TOKEN_TTL (seconds),
CLIENT_SECRET_FILE, ISSUER, REDIRECT_URI.
"""

import base64
import hashlib
import hmac
import json
import os
import secrets
import sys
import threading
import time
import urllib.parse
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

ROLE = sys.argv[1]
KEY = open(os.environ["KEY_FILE"], "rb").read().strip()
ISSUER = os.environ.get("ISSUER", "https://qd.verify.example/api/oauth")
API_HOSTS = os.environ.get("API_HOSTS", "qd.verify.example").split(",")
LOCK = threading.Lock()
CODES = {}


def b64(data):
    return base64.urlsafe_b64encode(data).rstrip(b"=").decode()


def unb64(text):
    return base64.urlsafe_b64decode(text + "=" * (-len(text) % 4))


def sign(typ, claims):
    head = b64(json.dumps({"alg": "HS256", "typ": typ}).encode())
    body = b64(json.dumps(claims).encode())
    mac = hmac.new(KEY, f"{head}.{body}".encode(), hashlib.sha256).digest()
    return f"{head}.{body}.{b64(mac)}"


def check(token, typ="at+jwt"):
    """What a bearer token is: ok:<sub>, expired, bad or wrong-aud."""
    try:
        head, body, mac = token.split(".")
        good = hmac.new(KEY, f"{head}.{body}".encode(), hashlib.sha256).digest()
        if not hmac.compare_digest(good, unb64(mac)) or json.loads(unb64(head))["typ"] != typ:
            return "bad", None
        claims = json.loads(unb64(body))
    except Exception:
        return "bad", None
    if claims.get("aud") != "grafana" or claims.get("iss") != ISSUER:
        return "wrong-aud", None
    if claims["exp"] <= time.time():
        return "expired", claims
    return "ok:" + claims["sub"], claims


def people():
    return json.load(open(os.environ["PEOPLE_FILE"]))


def log(entry):
    with LOCK, open(os.environ["LOG_FILE"], "a") as f:
        f.write(json.dumps(entry) + "\n")


def rows(dataset, chart, cut):
    """Synthetic rows: typed values, a null that must stay a gap."""
    if chart == "trend_by_cut":
        return [{"cut": c, "records": 1000 + i, "conformity": 0.9 + i / 100, "igcd": None if i == 1 else 0.8}
                for i, c in enumerate(["2026-09-30", "2026-10-31"])]
    return [{"dimension": "office", "value": f"Office {i}", "records": 100 * i, "rate": i / 10, "cut": cut}
            for i in range(1, 4)]


DATASETS = {  # dimensions, drift variables and met charts of each synthetic dataset
    "loans": (["office", "product"], ["amount"], ["trend_by_cut", "largest_groups", "psi_by_decile", "sfc_view"]),
    "savings": (["office", "channel", "advisor"], [], ["trend_by_cut", "largest_groups", "update_recency"]),
    "bare": ([], [], ["trend_by_cut"]),
}


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *args):
        pass

    def send(self, status, body=b"", ctype="application/json", headers=()):
        if isinstance(body, (dict, list)):
            body = json.dumps(body).encode()
        elif isinstance(body, str):
            body = body.encode()
        self.send_response(status)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Cache-Control", "no-store")
        for k, v in headers:
            self.send_header(k, v)
        self.end_headers()
        self.wfile.write(body)
        return status

    def form(self):
        n = int(self.headers.get("Content-Length") or 0)
        return dict(urllib.parse.parse_qsl(self.rfile.read(n).decode())) if n else {}

    def cookie(self, name):
        for part in (self.headers.get("Cookie") or "").split(";"):
            k, _, v = part.strip().partition("=")
            if k == name:
                return v
        return None

    def do_GET(self):
        self.route("GET")

    def do_POST(self):
        self.route("POST")

    def do_PUT(self):
        self.route("PUT")

    def route(self, method):
        url = urllib.parse.urlsplit(self.path)
        q = dict(urllib.parse.parse_qsl(url.query))
        entry = {"t": round(time.time(), 3), "role": ROLE, "host": self.headers.get("Host"), "method": method,
                 "path": url.path, "query": url.query, "headers": sorted(h.lower() for h in self.headers.keys())}
        try:
            entry["status"] = (self.app if ROLE == "app" else self.api)(method, url.path, q, entry)
        finally:
            log(entry)

    # --- the provider and the application's pages ------------------------
    def app(self, method, path, q, entry):
        who = self.cookie("qd_session")
        entry["session"] = bool(who)
        if path == "/test/signin":  # stands for signing in to Quality Data
            # Expires as the gateway's (its absolute expiry), so it outlives the browser.
            c = f"qd_session={q['person']}; Path=/; Secure; HttpOnly; SameSite=Strict; Max-Age=86400"
            return self.send(302, headers=[("Set-Cookie", c), ("Location", q.get("back", "/"))])
        if path == "/test/signout":
            c = "qd_session=; Path=/; Secure; HttpOnly; SameSite=Strict; Max-Age=0"
            return self.send(302, headers=[("Set-Cookie", c), ("Location", "/signin")])
        if path == "/oauth/authorize":
            if q.get("client_id") != "grafana" or q.get("redirect_uri") != os.environ["REDIRECT_URI"] \
                    or q.get("code_challenge_method") != "S256":
                return self.send(400, "invalid request", "text/plain")
            if not who:
                back = urllib.parse.quote(self.path)
                return self.send(302, headers=[("Location", "/signin?back=/api" + back)])
            code = secrets.token_urlsafe(24)
            with LOCK:
                CODES[code] = (who, q.get("nonce"), q["code_challenge"], time.time())
            sep = "&" if "?" in q["redirect_uri"] else "?"
            loc = q["redirect_uri"] + sep + urllib.parse.urlencode({"code": code, "state": q.get("state", "")})
            return self.send(302, headers=[("Location", loc)])
        if path == "/oauth/token" and method == "POST":
            f = self.form()
            secret = open(os.environ["CLIENT_SECRET_FILE"]).read().strip()
            basic = self.headers.get("Authorization", "")
            pair = base64.b64decode(basic[6:]).decode() if basic.startswith("Basic ") else ""
            given = urllib.parse.unquote(pair.partition(":")[2]) if pair else f.get("client_secret", "")
            entry["client_auth"] = "basic" if pair else "post"
            with LOCK:
                rec = CODES.pop(f.get("code", ""), None)
            if not hmac.compare_digest(given, secret) or not rec or time.time() - rec[3] > 60:
                return self.send(400, {"error": "invalid_grant"})
            if b64(hashlib.sha256(f.get("code_verifier", "").encode()).digest()) != rec[2]:
                return self.send(400, {"error": "invalid_grant"})
            person, nonce = rec[0], rec[1]
            p = people()[person]
            now, ttl = int(time.time()), int(os.environ.get("TOKEN_TTL", "86400"))
            base = {"iss": ISSUER, "sub": person, "aud": "grafana", "iat": now, "exp": now + ttl}
            ident = {"email": f"{person}@verify.example", "name": person.title(),
                     "policy": p["policy"], "grafana_orgs": p["grafana_orgs"]}
            idt = dict(base, **ident, **({"nonce": nonce} if nonce else {}))
            at = dict(base, client_id="grafana", jti=secrets.token_hex(8), scope="openid email profile", **ident)
            entry["issued_to"], entry["ttl"] = person, ttl
            return self.send(200, {"access_token": sign("at+jwt", at), "id_token": sign("JWT", idt),
                                   "token_type": "Bearer", "expires_in": ttl, "scope": "openid email profile"})
        if path == "/oauth/userinfo":
            got, claims = check((self.headers.get("Authorization") or "")[7:])
            entry["auth"] = got
            if not got.startswith("ok:"):
                return self.send(401, {"error": "invalid_token"})
            keep = ("sub", "email", "name", "policy", "grafana_orgs")
            return self.send(200, {k: claims[k] for k in keep})
        if path == "/signin":
            return self.send(200, "<!doctype html><title>Sign in</title><h1 id=signin>Sign in</h1>", "text/html")
        if path == "/page":  # the application's page: frames of q's src1, src2…
            frames = "".join(f'<iframe id=f{i} src="{q[k]}" style="width:1280px;height:900px"></iframe>'
                             for i, k in enumerate(sorted(k for k in q if k.startswith("src"))))
            return self.send(200, f"<!doctype html><title>Quality Data</title><h1>Dashboard</h1>{frames}",
                             "text/html")
        return self.send(404, {"error": "not found"})

    # --- the API's chart routes -------------------------------------------
    def api(self, method, path, q, entry):
        entry["id_token"] = check(self.headers["X-Id-Token"], "JWT")[0] if "x-id-token" in entry["headers"] else "-"
        auth = self.headers.get("Authorization") or ""
        got, claims = check(auth[7:]) if auth.startswith("Bearer ") else ("none", None)
        entry["auth"] = got
        parts = path.strip("/").split("/")
        if parts[:2] != ["quality", "datasets"] or self.headers.get("Host") not in API_HOSTS:
            entry["other"] = True
            return self.send(404, {"error": "not a chart route"})
        if method != "GET" or not got.startswith("ok:"):
            return self.send(401, {"error": "unauthorized"})
        person = got[3:]
        allowed = set(people()[person]["datasets"])
        if len(parts) == 2:
            return self.send(200, [{"dataset": d} for d in sorted(allowed)])
        dataset = urllib.parse.unquote(parts[2])
        if dataset not in allowed or dataset not in DATASETS:
            entry["denied"] = dataset
            return self.send(403, {"error": "forbidden"})
        dims, drift, met = DATASETS[dataset]
        if len(parts) == 4 and parts[3] == "charts":
            return self.send(200, {"cuts": ["2026-10-31", "2026-09-30"], "dimensions": [{"dimension": d} for d in dims],
                                   "drift_variables": [{"variable": v} for v in drift],
                                   "meets": [{"chart": c} for c in met]})
        if len(parts) == 5 and parts[3] == "charts":
            entry["chart"] = parts[4]
            return self.send(200, rows(dataset, parts[4], q.get("cut", "")))
        return self.send(404, {"error": "no such route"})


ThreadingHTTPServer(("0.0.0.0", 8080), Handler).serve_forever()
