import { afterEach, describe, expect, it, vi } from "vitest";
import { createApiClient } from "@knime-orchestador/api-client";
import { revealApiWith } from "./revealApi";

const json = (body: unknown, status = 200) =>
  new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });

/** The reveals over a fetch that answers `answer` and keeps each request. */
function client(answer: () => Response) {
  const seen: Request[] = [];
  vi.stubGlobal("fetch", async (request: Request) => {
    seen.push(request);
    return answer();
  });
  return { api: revealApiWith(createApiClient({ baseUrl: "http://qd.test/api" })), seen };
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("a reveal", () => {
  const reason = { purpose: "client_request" as const, reference: "CASO/2026/118" };

  it("posts the reason in its body, with the CSRF header, beside each view's read", async () => {
    vi.stubGlobal("document", { cookie: "lang=es; csrf=c%2Bsrf" });
    const { api, seen } = client(() => json({ state: "done", masked: false, revealable: true }));

    await api.records("core", "j1", reason);
    await api.test("j2", reason);
    await api.console("j3", { purpose: "audit", reference: null });

    expect(seen.map((r) => [r.method, new URL(r.url).pathname, r.url.includes("?")])).toEqual([
      ["POST", "/api/quality/sources/core/records/j1/reveal", false],
      ["POST", "/api/quality/rules/tests/j2/reveal", false],
      ["POST", "/api/quality/console/j3/reveal", false],
    ]);
    for (const r of seen) expect(r.headers.get("x-csrf-token")).toBe("c+srf");
    expect(await seen[0].json()).toEqual(reason);
    expect(await seen[2].json()).toEqual({ purpose: "audit", reference: null });
  });

  it("is refused with the API's code", async () => {
    vi.stubGlobal("document", { cookie: "" });
    const { api } = client(() => json({ error: "audit_unavailable" }, 503));
    await expect(api.console("j3", reason)).rejects.toMatchObject({
      status: 503,
      body: { error: "audit_unavailable" },
    });
  });
});
