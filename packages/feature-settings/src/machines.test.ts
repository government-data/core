import { afterEach, describe, expect, it, vi } from "vitest";
import { createApiClient } from "@knime-orchestador/api-client";
import { actOn, machineActions, machinesApiWith, type Machine } from "./machinesApi";

const machine = (state: string): Machine => ({
  id: "m-7",
  name: "worker-7",
  role: "worker",
  pool: "default",
  state,
  version: "0.1.0",
  created_at: 1_700_000_000,
});

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

describe("what a person may do to a machine", () => {
  it("approves a pending one, and may retire it", () => {
    expect(machineActions("pending", true)).toEqual(["approve", "retire"]);
  });

  it("drains an approved one, retires a draining one, and nothing on a retired one", () => {
    expect(machineActions("approved", true)).toEqual(["drain", "retire"]);
    expect(machineActions("draining", true)).toEqual(["retire"]);
    expect(machineActions("retired", true)).toEqual([]);
  });

  it("is nothing without admin:ManageMachines, whatever the state", () => {
    for (const state of ["pending", "approved", "draining", "retired"]) {
      expect(machineActions(state, false)).toEqual([]);
    }
  });
});

describe("approving a machine", () => {
  it("posts to its route, then reads the list again", async () => {
    const { api, seen } = client(
      json(machine("approved")),
      json({ machines: [machine("approved")], can_join: true }),
    );
    const refresh = vi.fn(() => api.list());

    const notice = await actOn(api, machine("pending"), "approve", refresh);

    expect(seen.map((r) => [r.method, new URL(r.url).pathname])).toEqual([
      ["POST", "/api/machines/m-7/approve"],
      ["GET", "/api/machines"],
    ]);
    expect(refresh).toHaveBeenCalledOnce();
    expect(notice).toContain("worker-7");
  });

  it("gives the API's message when it is refused, and does not read the list", async () => {
    const { api } = client(json({ code: "transition_not_allowed", message: "Not pending" }, 409));
    const refresh = vi.fn();

    await expect(actOn(api, machine("approved"), "approve", refresh)).rejects.toMatchObject({
      status: 409,
      message: "Not pending",
      code: "transition_not_allowed",
    });
    expect(refresh).not.toHaveBeenCalled();
  });

  it("is refused to a person without the action as the API refuses it", async () => {
    const { api } = client(json({}, 403));
    await expect(api.act("m-7", "approve")).rejects.toMatchObject({ status: 403 });
  });
});
