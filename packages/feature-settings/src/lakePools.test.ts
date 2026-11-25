import { afterEach, describe, expect, it, vi } from "vitest";
import { createApiClient } from "@knime-orchestador/api-client";
import {
  defineMainPool,
  definable,
  growsOnlyByCopy,
  loadMainPool,
  nodeState,
  parseNodes,
  requestNodeJoin,
} from "./lakePools";
import { machinesApiWith, type JoinToken, type LakePool } from "./machinesApi";

// Synthetic values: no real token, address or server.
const TOKEN = "qdj_synthetic-test-token";
const POOL: LakePool = {
  name: "main",
  drives: 2,
  defined_at: 1_700_000_000,
  nodes: [
    { name: "lake-a", state: null, machine_id: null },
    { name: "lake-b", state: "pending", machine_id: "m-1" },
    { name: "lake-c", state: null, machine_id: null },
    { name: "lake-d", state: null, machine_id: null },
  ],
};

const json = (body: unknown, status = 200) =>
  new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });

function client(...answers: Response[]) {
  const seen: Request[] = [];
  vi.stubGlobal("fetch", async (request: Request) => {
    seen.push(request);
    return answers.shift() ?? json({}, 500);
  });
  vi.stubGlobal("document", { cookie: "csrf=c%2Bsrf" });
  return { api: machinesApiWith(createApiClient({ baseUrl: "http://qd.test/api" })), seen };
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("the lake's pool", () => {
  it("reads main and says whether its layout may change", async () => {
    const { api, seen } = client(json({ pools: [POOL] }), json({ pools: [] }));
    const pool = await loadMainPool(api);
    expect(pool?.nodes.map((n) => n.name)).toEqual(["lake-a", "lake-b", "lake-c", "lake-d"]);
    expect([seen[0].method, new URL(seen[0].url).pathname]).toEqual(["GET", "/api/lake/pools"]);
    expect(seen[0].headers.get("x-csrf-token")).toBeNull();
    expect(definable(pool)).toBe(false);
    expect(await loadMainPool(api)).toBeUndefined();
    expect(definable(undefined)).toBe(true);
    const fresh = { ...POOL, nodes: POOL.nodes.map((n) => ({ ...n, state: null })) };
    expect(definable(fresh)).toBe(true);
  });

  it("defines main with the nodes typed, in order, and the CSRF header", async () => {
    const { api, seen } = client(json(POOL));
    expect(parseNodes(" lake-a\nlake-b, lake-c  lake-d\n\n")).toEqual([
      "lake-a",
      "lake-b",
      "lake-c",
      "lake-d",
    ]);
    await defineMainPool(api, "lake-a\nlake-b\nlake-c\nlake-d", 2);
    const [request] = seen;
    expect([request.method, new URL(request.url).pathname]).toEqual([
      "PUT",
      "/api/lake/pools/main",
    ]);
    expect(request.headers.get("x-csrf-token")).toBe("c+srf");
    expect(await request.json()).toEqual({
      nodes: ["lake-a", "lake-b", "lake-c", "lake-d"],
      drives: 2,
    });
  });

  it("gives a refusal on its field", async () => {
    const refused = {
      message: "Validation failed",
      fields: { nodes: "A pool has 4 to 16 nodes." },
    };
    const { api } = client(json(refused, 422));
    await expect(defineMainPool(api, "lake-a", 1)).rejects.toMatchObject({
      status: 422,
      fields: { nodes: "A pool has 4 to 16 nodes." },
    });
  });

  it("says whether each node joined", () => {
    expect(nodeState(null)).not.toEqual(nodeState("pending"));
    expect(nodeState("approved")).toContain("Approved");
  });
});

describe("a pool node's command", () => {
  const answer: JoinToken = {
    token: TOKEN,
    role: "lake",
    pool: "main",
    node: "lake-c",
    expires_at: 1_700_003_600,
    install: {
      script_url: "https://qd.test/api/install/join.sh",
      script_sha256: "aaaa",
      ca_sha256: "bbbb",
      command: `sh qd-join.sh --role lake --pool 'main' --name 'lake-c' --token '${TOKEN}'`,
    },
  };

  it("names the node, with its drives, and is shown as the command", async () => {
    const { api, seen } = client(json(answer, 201));
    const outcome = await requestNodeJoin(api, "lake-c", "/srv/d1\n/srv/d2");
    expect(await seen[0].json()).toEqual({
      role: "lake",
      pool: "main",
      node: "lake-c",
      drives: ["/srv/d1", "/srv/d2"],
    });
    expect(outcome).toMatchObject({ kind: "command", addDrives: false, pool: "main" });
  });

  it("asks to add the drives by hand when none were given", async () => {
    const { api, seen } = client(json(answer, 201));
    const outcome = await requestNodeJoin(api, "lake-c", " ");
    expect(await seen[0].json()).toEqual({ role: "lake", pool: "main", node: "lake-c" });
    expect(outcome).toMatchObject({ kind: "command", addDrives: true });
  });
});

describe("growsOnlyByCopy", () => {
  it("holds for one server with one drive alone", () => {
    expect(growsOnlyByCopy(1, 1)).toBe(true);
    expect(growsOnlyByCopy(1, 4)).toBe(false);
    expect(growsOnlyByCopy(2, 1)).toBe(false);
    expect(growsOnlyByCopy(4, 2)).toBe(false);
  });
});
