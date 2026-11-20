import { afterEach, describe, expect, it, vi } from "vitest";
import { createApiClient } from "@knime-orchestador/api-client";
import { readinessLine, warehouseLine } from "./lakeReplication";
import { machinesApiWith, type Replication, type WarehouseReplication } from "./machinesApi";

const json = (body: unknown, status = 200) =>
  new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });

const W = "qd-tenant-00000000000000000000000000000001";

const replication = (given: Partial<Replication>): Replication => ({
  readiness: "no_address",
  lake: null,
  source: null,
  replica: null,
  proxy: null,
  retention_days: 30,
  noncurrent_days: 7,
  warehouses: [],
  ...given,
});

const warehouse = (given: Partial<WarehouseReplication>): WarehouseReplication => ({
  warehouse: W,
  state: "configured",
  step: null,
  lake: null,
  detail: null,
  configured_at: 1_700_000_000,
  checked_at: 1_700_003_600,
  ...given,
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("the replication", () => {
  it("is read with a GET and no key", async () => {
    const seen: Request[] = [];
    const ready = replication({
      readiness: "ready",
      source: "https://lake-1:9000",
      replica: "replica-1",
      proxy: "lake-1:9100",
      warehouses: [warehouse({})],
    });
    vi.stubGlobal("fetch", async (request: Request) => {
      seen.push(request);
      return json(ready);
    });
    const api = machinesApiWith(createApiClient({ baseUrl: "http://qd.test/api" }));

    const read = await api.replication();

    expect([seen[0].method, new URL(seen[0].url).pathname]).toEqual([
      "GET",
      "/api/lake/replication",
    ]);
    expect(seen[0].headers.get("x-csrf-token")).toBeNull();
    const line = readinessLine(read);
    expect(line.tone).toBe("ok");
    expect(line.text).toContain("replica-1");
    expect(line.text).toContain("lake-1:9100");
  });

  it("says what it waits for", () => {
    expect(readinessLine(replication({})).tone).toBe("error");
    expect(readinessLine(replication({ readiness: "no_replica" })).tone).toBe("warn");
    const main = readinessLine(replication({ readiness: "no_keys", lake: "main" }));
    const replica = readinessLine(replication({ readiness: "no_keys", lake: "replica" }));
    expect(main.tone).toBe("warn");
    expect(main.text).not.toBe(replica.text);
  });

  it("says each warehouse's state, a failure with its step and what happened", () => {
    expect(warehouseLine(warehouse({})).tone).toBe("ok");
    const failed = warehouseLine(
      warehouse({
        state: "failed",
        step: "remote_target",
        detail: "HTTP 502",
        configured_at: null,
      }),
    );
    expect(failed.tone).toBe("error");
    expect(failed.text).toContain("remote_target");
    expect(failed.text).toContain("HTTP 502");
    expect(warehouseLine(warehouse({ state: "no_warehouse" })).tone).toBe("warn");
    const made = warehouseLine(warehouse({ state: "made", configured_at: null }));
    expect(made.tone).toBe("warn");
    expect(made.text).not.toBe(warehouseLine(warehouse({ state: "no_warehouse" })).text);
    const notMade = warehouseLine(
      warehouse({ state: "failed", step: "warehouse_tables", detail: "HTTP 400" }),
    );
    expect(notMade.text).toContain("warehouse_tables");
  });
});
