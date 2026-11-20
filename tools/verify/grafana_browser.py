"""Drive this machine's own Chromium, headless, over the DevTools pipe, for
decision 0073's verification (grafana_embed.sh and the others): one host
mapped to Traefik's address on the throwaway network, its certificate
trusted by its key's hash alone. Standard library only. A scenario is a JSON
list of steps read from a file; the results go to stdout as JSON. Nothing
prints a cookie's value or a token.

    python3 -I grafana_browser.py HOST IP SPKI PROFILE STEPS.json

Steps: {"go": url} navigates and waits for the load event; {"wait": js,
"timeout": s, "as": name} polls until js is truthy and records the seconds;
{"eval": js, "as": name}; {"key": "Escape"} after focusing the first frame;
{"sleep": s}; {"mark": name} starts a count of requests; {"net": name} counts
what was requested since the mark; {"cookies": name} lists the cookies'
names and attributes. js runs in the top page, with `f` = the first frame's
document (or null when it cannot be read: another origin, an error page).
"""

import fcntl
import json
import os
import subprocess
import sys
import threading
import time

HOST, IP, SPKI, PROFILE, STEPS = sys.argv[1:6]
PRELUDE = "(() => { let f = null; try { f = document.querySelector('iframe').contentDocument; } catch (e) {} return (%s); })()"


class Browser:
    def __init__(self):
        to_r, to_w = os.pipe()
        from_r, from_w = os.pipe()
        hi_r = fcntl.fcntl(to_r, fcntl.F_DUPFD, 100)
        hi_w = fcntl.fcntl(from_w, fcntl.F_DUPFD, 100)

        def fds():
            os.dup2(hi_r, 3)
            os.dup2(hi_w, 4)

        args = ["chromium", "--headless=new", "--remote-debugging-pipe", f"--user-data-dir={PROFILE}",
                f"--host-resolver-rules=MAP {HOST} {IP}", f"--ignore-certificate-errors-spki-list={SPKI}",
                "--no-first-run", "--no-default-browser-check", "--disable-gpu", "--disable-extensions",
                "--disable-background-networking", "--window-size=1440,1000", "about:blank"]
        self.proc = subprocess.Popen(args, preexec_fn=fds, pass_fds=(3, 4, hi_r, hi_w),
                                     stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        with open(PROFILE + ".pid", "w") as f:
            f.write(str(self.proc.pid))
        for fd in (to_r, from_w, hi_r, hi_w):
            os.close(fd)
        self.out, self.inp = os.fdopen(to_w, "wb"), os.fdopen(from_r, "rb")
        self.next, self.replies, self.events = 0, {}, []
        self.cv = threading.Condition()
        threading.Thread(target=self.reader, daemon=True).start()

    def reader(self):
        buf = b""
        while True:
            chunk = self.inp.read1(65536)
            if not chunk:
                return
            buf += chunk
            while b"\0" in buf:
                msg, buf = buf.split(b"\0", 1)
                m = json.loads(msg)
                with self.cv:
                    if "id" in m:
                        self.replies[m["id"]] = m
                    else:
                        self.events.append((time.time(), m))
                    self.cv.notify_all()

    def call(self, method, params=None, session=None, timeout=60):
        with self.cv:
            self.next += 1
            n = self.next
        msg = {"id": n, "method": method, "params": params or {}}
        if session:
            msg["sessionId"] = session
        self.out.write(json.dumps(msg).encode() + b"\0")
        self.out.flush()
        with self.cv:
            if not self.cv.wait_for(lambda: n in self.replies, timeout):
                raise TimeoutError(method)
            r = self.replies.pop(n)
        if "error" in r:
            raise RuntimeError(f"{method}: {r['error']}")
        return r["result"]


def main():
    b = Browser()
    res = {}
    try:
        target = b.call("Target.createTarget", {"url": "about:blank"})["targetId"]
        s = b.call("Target.attachToTarget", {"targetId": target, "flatten": True})["sessionId"]
        for m in ("Page.enable", "Network.enable", "Runtime.enable"):
            b.call(m, session=s)
        b.call("Network.setCacheDisabled", {"cacheDisabled": False}, session=s)

        def ev(js):
            r = b.call("Runtime.evaluate", {"expression": PRELUDE % js, "returnByValue": True,
                                            "awaitPromise": True}, session=s)
            return r["result"].get("value")

        mark_t, mark_i = time.time(), 0
        for step in json.load(open(STEPS)):
            if "go" in step:
                with b.cv:
                    mark = len(b.events)
                b.call("Page.navigate", {"url": step["go"]}, session=s)
                with b.cv:
                    b.cv.wait_for(lambda: any(e[1].get("method") == "Page.loadEventFired" for e in b.events[mark:]),
                                  step.get("timeout", 60))
            elif "wait" in step:
                t0, val = time.time(), None
                while time.time() - t0 < step.get("timeout", 60):
                    val = ev(step["wait"])
                    if val:
                        break
                    time.sleep(0.25)
                res[step["as"]] = round(time.time() - t0, 2) if val else None
            elif "eval" in step:
                res[step["as"]] = ev(step["eval"])
            elif "key" in step:
                ev("document.querySelector('iframe').contentWindow.focus() || true")
                for kind in ("keyDown", "keyUp"):
                    b.call("Input.dispatchKeyEvent", {"type": kind, "key": step["key"], "code": step["key"],
                                                      "windowsVirtualKeyCode": 27}, session=s)
            elif "sleep" in step:
                time.sleep(step["sleep"])
            elif "mark" in step:
                with b.cv:
                    mark_t, mark_i = time.time(), len(b.events)
            elif "net" in step:
                with b.cv:
                    evs = b.events[mark_i:]
                reqs = {e[1]["params"]["requestId"]: e[1]["params"] for e in evs
                        if e[1].get("method") == "Network.requestWillBeSent"}
                done = {e[1]["params"]["requestId"]: e[1]["params"].get("encodedDataLength", 0) for e in evs
                        if e[1].get("method") == "Network.loadingFinished"}
                status = {e[1]["params"]["requestId"]: e[1]["params"]["response"]["status"] for e in evs
                          if e[1].get("method") == "Network.responseReceived"}
                urls = [r["request"]["url"].split("?")[0] for r in reqs.values()]
                kinds = {}
                for u in urls:
                    k = ("ds/query" if u.endswith("/api/ds/query") else "grafana api" if "/grafana/api/" in u
                         else "grafana build" if "/grafana/public/" in u else "other")
                    kinds[k] = kinds.get(k, 0) + 1
                orgs = {{k.lower(): v for k, v in r["request"]["headers"].items()}.get("x-grafana-org-id", "-")
                        for r in reqs.values()
                        if "/grafana/api/" in r["request"]["url"]}
                res[step["net"]] = {"requests": len(reqs), "kinds": kinds, "bytes": int(sum(done.values())),
                                    "failed": sorted({reqs[i]["request"]["url"].split("?")[0].split(HOST)[-1] + f" {c}"
                                                      for i, c in status.items() if c >= 400 and i in reqs}),
                                    "seconds": round(time.time() - mark_t, 2), "api_orgs": sorted(orgs)}
            elif "cookies" in step:
                cs = b.call("Network.getAllCookies", session=s)["cookies"]
                res[step["cookies"]] = sorted(f'{c["name"]} path={c["path"]} secure={c["secure"]} httponly={c["httpOnly"]} '
                                              f'samesite={c.get("sameSite", "-")}' for c in cs)
    finally:
        try:
            b.call("Browser.close", timeout=10)
        except Exception:
            pass
        try:
            b.proc.wait(10)
        except subprocess.TimeoutExpired:
            b.proc.kill()
    print(json.dumps(res))


main()
