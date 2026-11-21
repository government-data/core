import { afterEach, describe, expect, it, vi } from "vitest";
import { createApiClient } from "@knime-orchestador/api-client";
import { canAddMachine, joinOutcome, parseDrives, requestJoin } from "./joinRequest";
import { machinesApiWith, type JoinToken } from "./machinesApi";

// Synthetic values: no real token, address or hash.
const TOKEN = "qdj_synthetic-test-token";
const COMMAND = `curl -fsS --proto '=https' -o qd-join.sh 'https://qd.test/api/install/join.sh' \\
&& echo 'aaaa  qd-join.sh' | sha256sum -c - \\
&& sh qd-join.sh --url 'https://qd.test' --role worker --pool 'default' \\
     --ca-sha256 'bbbb' --token '${TOKEN}'`;

const answer = (over: Partial<JoinToken> = {}): JoinToken => ({
  token: TOKEN,
  role: "worker",
  pool: "default",
  expires_at: 1_700_003_600,
  install: {
    script_url: "https://qd.test/api/install/join.sh",
    script_sha256: "aaaa",
    ca_sha256: "bbbb",
    command: COMMAND,
  },
  ...over,
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

describe("offering to add a machine", () => {
  it("is offered to whoever may manage machines while there is a CA", () => {
    expect(canAddMachine(true, true)).toBe(true);
  });

  it("is not offered without admin:ManageMachines, nor without a CA", () => {
    expect(canAddMachine(false, true)).toBe(false);
    expect(canAddMachine(false, false)).toBe(false);
    expect(canAddMachine(true, false)).toBe(false);
  });
});

describe("asking for a join token", () => {
  it("posts the role and the pool with the CSRF header", async () => {
    vi.stubGlobal("document", { cookie: "lang=es; csrf=c%2Bsrf" });
    const { api, seen } = client(json(answer(), 201));

    await requestJoin(api, "worker", " default ");

    expect(seen).toHaveLength(1);
    const [request] = seen;
    expect([request.method, new URL(request.url).pathname]).toEqual([
      "POST",
      "/api/machines/join-tokens",
    ]);
    expect(request.headers.get("x-csrf-token")).toBe("c+srf");
    expect(await request.json()).toEqual({ role: "worker", pool: "default" });
  });

  it("sends no pool when it is left blank", async () => {
    const { api, seen } = client(json(answer({ role: "worker", pool: null, install: null }), 201));

    await requestJoin(api, "worker", "  ");

    expect(await seen[0].json()).toEqual({ role: "worker" });
  });

  it("gives the command to show when the API built one, and not the bare token", async () => {
    const { api } = client(json(answer(), 201));

    const outcome = await requestJoin(api, "worker", "default");

    expect(outcome).toEqual({
      kind: "command",
      command: COMMAND,
      addDrives: false,
      role: "worker",
      pool: "default",
      expiresAt: 1_700_003_600,
    });
  });

  it("gives the token alone when there is no command", () => {
    expect(joinOutcome(answer({ role: "worker", pool: null, install: null }))).toEqual({
      kind: "token",
      token: TOKEN,
      role: "worker",
      pool: undefined,
      expiresAt: 1_700_003_600,
    });
    expect(joinOutcome(answer({ install: undefined })).kind).toBe("token");
  });
});

describe("a lake replica's drives", () => {
  const replica = (command: string) =>
    answer({ role: "lake-replica", pool: null, install: { ...answer().install!, command } });

  it("reads one path per line or between spaces, blanks dropped", () => {
    expect(parseDrives(" /srv/d1\n\n/srv/d2  /srv/d3\n")).toEqual([
      "/srv/d1",
      "/srv/d2",
      "/srv/d3",
    ]);
    expect(parseDrives("  \n ")).toEqual([]);
  });

  it("posts the drives typed, and the command needs none added", async () => {
    const { api, seen } = client(json(replica("… --drive '/srv/d1' --drive '/srv/d2'"), 201));

    const outcome = await requestJoin(api, "lake-replica", "", "/srv/d1\n/srv/d2\n");

    expect(await seen[0].json()).toEqual({ role: "lake-replica", drives: ["/srv/d1", "/srv/d2"] });
    expect(outcome).toMatchObject({ kind: "command", addDrives: false });
  });

  it("sends no drives when none are typed, and says to add them by hand", async () => {
    const { api, seen } = client(json(replica("… --role lake-replica"), 201));

    const outcome = await requestJoin(api, "lake-replica", "", "  ");

    expect(await seen[0].json()).toEqual({ role: "lake-replica" });
    expect(outcome).toMatchObject({ kind: "command", addDrives: true });
  });

  it("sends no drives for a role that takes none, even if some were typed", async () => {
    const { api, seen } = client(json(answer(), 201));

    const outcome = await requestJoin(api, "worker", "default", "/srv/d1");

    expect(await seen[0].json()).toEqual({ role: "worker", pool: "default" });
    expect(outcome).toMatchObject({ kind: "command", addDrives: false });
  });

  it("gives the API's message for bad drives, on their field", async () => {
    const message = "Each drive is an absolute path.";
    const { api } = client(
      json({ code: "validation_failed", message, fields: { drives: message } }, 422),
    );
    await expect(requestJoin(api, "lake-replica", "", "srv/d1")).rejects.toMatchObject({
      status: 422,
      fields: { drives: message },
    });
  });
});

describe("a join token refused", () => {
  it("is refused to a person without the action as the API refuses it", async () => {
    const { api } = client(json({}, 403));
    await expect(requestJoin(api, "worker", "")).rejects.toMatchObject({ status: 403 });
  });

  it("gives the API's message for a bad pool, on its field", async () => {
    const { api } = client(
      json(
        {
          code: "validation_failed",
          message: "The request failed validation.",
          fields: { pool: "Pool: lowercase letters, digits and hyphens." },
        },
        422,
      ),
    );
    await expect(requestJoin(api, "worker", "Bad Pool")).rejects.toMatchObject({
      status: 422,
      code: "validation_failed",
      message: "Pool: lowercase letters, digits and hyphens.",
      fields: { pool: "Pool: lowercase letters, digits and hyphens." },
    });
  });

  it("gives the API's message without a CA", async () => {
    const { api } = client(json({ code: "no_ca", message: "No CA yet" }, 503));
    await expect(requestJoin(api, "worker", "")).rejects.toMatchObject({
      status: 503,
      code: "no_ca",
      message: "No CA yet",
    });
  });
});
