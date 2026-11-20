import { afterEach, describe, expect, it, vi } from "vitest";
import { createApiClient } from "@knime-orchestador/api-client";
import { ADMIN_ACCESS_KEY, loadAdminKeys, saveAdminKeys } from "./lakeAdmin";
import { mainLine } from "./lakeMain";
import { machinesApiWith, type LakeAdminKeys, type MainLakeState } from "./machinesApi";

// Synthetic values: no real key.
const SECRET = "6f".repeat(32);
const KEPT: LakeAdminKeys = { set: true, access_key: ADMIN_ACCESS_KEY, set_at: 1_700_000_000 };
const ENDPOINT = "https://lake-1:9000";

const json = (body: unknown, status = 200) =>
  new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });

/** A client over a fetch that answers `answers` in turn and keeps each request. */
function client(...answers: Response[]) {
  const seen: Request[] = [];
  vi.stubGlobal("fetch", async (request: Request) => {
    seen.push(request);
    return answers.shift() ?? json({}, 500);
  });
  return { api: machinesApiWith(createApiClient({ baseUrl: "http://qd.test/api" })), seen };
}

/** A state as the API answers it: everything empty but what is given. */
const state = (given: Partial<MainLakeState>): MainLakeState => ({
  state: "no_address",
  endpoint: null,
  drives_online: null,
  drives_offline: null,
  code: null,
  detail: null,
  ...given,
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("the main lake", () => {
  it("is read from the API with a GET and no key", async () => {
    const answered = state({
      state: "answered",
      endpoint: ENDPOINT,
      drives_online: 4,
      drives_offline: 0,
    });
    const { api, seen } = client(json(answered));

    const read = await api.mainState();

    expect([seen[0].method, new URL(seen[0].url).pathname]).toEqual([
      "GET",
      "/api/lake/main/state",
    ]);
    expect(seen[0].headers.get("x-csrf-token")).toBeNull();
    const line = mainLine(read);
    expect(line.tone).toBe("ok");
    expect(line.text).toContain("4");
  });

  it("says what is missing, refused or not reached", () => {
    expect(mainLine(state({})).tone).toBe("error");
    const noKeys = mainLine(state({ state: "no_keys", endpoint: ENDPOINT }));
    expect([noKeys.tone, noKeys.text.includes(ENDPOINT)]).toEqual(["warn", true]);
    const refused = mainLine(state({ state: "keys_refused", code: "SignatureDoesNotMatch" }));
    expect([refused.tone, refused.text.includes("SignatureDoesNotMatch")]).toEqual(["error", true]);
    const away = mainLine(state({ state: "unreachable", endpoint: ENDPOINT, detail: "timed out" }));
    expect([away.tone, away.text.includes("timed out")]).toEqual(["error", true]);
  });

  it("keeps its admin keys apart from the replica's, put trimmed with the CSRF header", async () => {
    vi.stubGlobal("document", { cookie: "csrf=tok" });
    const { api, seen } = client(json({ set: false, access_key: null, set_at: null }), json(KEPT));

    await loadAdminKeys(api, "main");
    const kept = await saveAdminKeys(api, "main", ` ${ADMIN_ACCESS_KEY}`, `${SECRET}\n`);

    expect(seen.map((r) => [r.method, new URL(r.url).pathname])).toEqual([
      ["GET", "/api/lake/main/admin-keys"],
      ["PUT", "/api/lake/main/admin-keys"],
    ]);
    expect(seen[1].headers.get("x-csrf-token")).toBe("tok");
    expect(await seen[1].json()).toEqual({ access_key: ADMIN_ACCESS_KEY, secret_key: SECRET });
    expect(JSON.stringify(kept)).not.toContain(SECRET);
  });
});
