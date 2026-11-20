import { afterEach, describe, expect, it, vi } from "vitest";
import { createApiClient } from "@knime-orchestador/api-client";
import { replicaLine } from "./lakeReplica";
import { machinesApiWith, type ReplicaState } from "./machinesApi";

const json = (body: unknown, status = 200) =>
  new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });

/** A client over a fetch that answers `answer` and keeps each request. */
function client(answer: Response) {
  const seen: Request[] = [];
  vi.stubGlobal("fetch", async (request: Request) => {
    seen.push(request);
    return answer;
  });
  return { api: machinesApiWith(createApiClient({ baseUrl: "http://qd.test/api" })), seen };
}

/** A state as the API answers it: everything empty but what is given. */
const state = (given: Partial<ReplicaState>): ReplicaState => ({
  state: "no_machine",
  machine: null,
  pending: null,
  machines: [],
  drives_online: null,
  drives_offline: null,
  code: null,
  detail: null,
  ...given,
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("the replica's state", () => {
  it("is read from the API with a GET and no key", async () => {
    const answered = state({
      state: "answered",
      machine: "replica-1",
      drives_online: 2,
      drives_offline: 0,
    });
    const { api, seen } = client(json(answered));

    const read = await api.replicaState();

    expect([seen[0].method, new URL(seen[0].url).pathname]).toEqual([
      "GET",
      "/api/lake/replica/state",
    ]);
    expect(seen[0].headers.get("x-csrf-token")).toBeNull();
    expect(read).toEqual(answered);
    const line = replicaLine(read);
    expect(line.tone).toBe("ok");
    expect(line.text).toContain("replica-1");
    expect(line.text).toContain("2");
  });

  it("says what is missing, refused or not reached, naming the machine", () => {
    const cases: [Partial<ReplicaState>, string, string][] = [
      [{}, "warn", ""],
      [{ pending: "replica-1" }, "warn", "replica-1"],
      [{ state: "several_machines", machines: ["replica-1", "replica-2"] }, "error", "replica-2"],
      [{ state: "no_keys", machine: "replica-1" }, "warn", "replica-1"],
      [
        { state: "keys_refused", machine: "replica-1", code: "SignatureDoesNotMatch" },
        "error",
        "SignatureDoesNotMatch",
      ],
      [
        { state: "unreachable", machine: "replica-1", detail: "connection refused" },
        "error",
        "connection refused",
      ],
    ];
    for (const [given, tone, shown] of cases) {
      const line = replicaLine(state(given));
      expect([line.tone, line.text.includes(shown)]).toEqual([tone, true]);
      expect(line.text).not.toBe("");
    }
  });

  it("is refused to a person without the action as the API refuses it", async () => {
    const { api } = client(json({}, 403));
    await expect(api.replicaState()).rejects.toMatchObject({ status: 403 });
  });
});
