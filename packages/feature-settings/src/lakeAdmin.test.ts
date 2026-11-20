import { afterEach, describe, expect, it, vi } from "vitest";
import { createApiClient } from "@knime-orchestador/api-client";
import { ADMIN_ACCESS_KEY, keptState, saveReplicaAdminKeys } from "./lakeAdmin";
import { machinesApiWith, type LakeAdminKeys } from "./machinesApi";

// Synthetic values: no real key.
const SECRET = "5e".repeat(32);
const KEPT: LakeAdminKeys = { set: true, access_key: ADMIN_ACCESS_KEY, set_at: 1_700_000_000 };

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

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("the replica's admin keys", () => {
  it("are read without a secret", async () => {
    const { api, seen } = client(json({ set: false, access_key: null, set_at: null }));

    const kept = await api.replicaAdminKeys();

    expect([seen[0].method, new URL(seen[0].url).pathname]).toEqual([
      "GET",
      "/api/lake/replica/admin-keys",
    ]);
    expect(keptState(kept)).toEqual({ kind: "none" });
    expect(keptState(KEPT)).toEqual({
      kind: "kept",
      accessKey: ADMIN_ACCESS_KEY,
      setAt: 1_700_000_000_000,
    });
    expect(keptState(undefined)).toEqual({ kind: "none" });
  });

  it("are put as pasted, trimmed, with the CSRF header", async () => {
    vi.stubGlobal("document", { cookie: "lang=es; csrf=c%2Bsrf" });
    const { api, seen } = client(json(KEPT));

    const kept = await saveReplicaAdminKeys(api, ` ${ADMIN_ACCESS_KEY} `, `\n${SECRET} \n`);

    expect(seen).toHaveLength(1);
    const [request] = seen;
    expect([request.method, new URL(request.url).pathname]).toEqual([
      "PUT",
      "/api/lake/replica/admin-keys",
    ]);
    expect(request.headers.get("x-csrf-token")).toBe("c+srf");
    expect(await request.json()).toEqual({ access_key: ADMIN_ACCESS_KEY, secret_key: SECRET });
    expect(kept).toEqual(KEPT);
    expect(JSON.stringify(kept)).not.toContain(SECRET);
  });

  it("give the API's message on the secret's field when it is out of form", async () => {
    const message = "The secret is the 64 hex digits join.sh showed on the replica, once.";
    const { api } = client(
      json({ code: "validation_failed", message, fields: { secret_key: message } }, 422),
    );
    await expect(saveReplicaAdminKeys(api, ADMIN_ACCESS_KEY, "short")).rejects.toMatchObject({
      status: 422,
      message,
      fields: { secret_key: message },
    });
  });

  it("are refused to a person without the action as the API refuses them", async () => {
    const { api } = client(json({}, 403));
    await expect(saveReplicaAdminKeys(api, ADMIN_ACCESS_KEY, SECRET)).rejects.toMatchObject({
      status: 403,
    });
  });
});
