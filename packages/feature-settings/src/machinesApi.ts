/**
 * Settings → Execution's calls (decisions 0029 and 0066), through the typed
 * client over the API's contract: the machines and their changes of state,
 * and Settings → Lake's main lake and replica and their admin keys, for whoever may do
 * `admin:ManageMachines`, which the API checks.
 */

import { createApiClient, type components } from "@knime-orchestador/api-client";
import { sessionExpired } from "@knime-orchestador/feature-kit";
import { SettingsApiError } from "./api";
import { t } from "./i18n";

export type Machine = components["schemas"]["MachineOut"];
export type MachinesList = components["schemas"]["MachinesOut"];
export type JoinToken = components["schemas"]["JoinTokenOut"];
export type LakeAdminKeys = components["schemas"]["LakeAdminKeysOut"];
export type ReplicaState = components["schemas"]["ReplicaStateOut"];
export type MainLakeState = components["schemas"]["MainLakeStateOut"];
export type Replication = components["schemas"]["ReplicationOut"];
export type WarehouseReplication = components["schemas"]["WarehouseReplicationOut"];
export type LakePool = components["schemas"]["LakePoolOut"];

/** What a person can do to a machine; each its own route. */
export type MachineAction = "approve" | "drain" | "retire";

type Answer<T> = { data?: T; error?: unknown; response: Response };

/** The `X-CSRF-Token` header the changes need, from the readable `csrf` cookie. */
function csrf(): Record<string, string> {
  const token = globalThis.document?.cookie
    .split("; ")
    .find((c) => c.startsWith("csrf="))
    ?.slice("csrf=".length);
  return token ? { "x-csrf-token": decodeURIComponent(token) } : {};
}

/** The data of a call, or a {@link SettingsApiError} with the API's message. */
async function done<T>(call: Promise<Answer<T>>): Promise<T> {
  const { data, error, response } = await call;
  if (response.status === 401) {
    sessionExpired();
    throw new SettingsApiError(401, t("settings:api.expired"));
  }
  if (!response.ok) {
    const body = (error ?? {}) as {
      message?: string;
      code?: string;
      fields?: Record<string, string>;
    };
    const fields = body.fields ?? {};
    // A validation error's own message is generic; its fields' are the API's.
    const message =
      response.status === 403
        ? t("settings:api.adminOnly")
        : (Object.values(fields)[0] ??
          body.message ??
          t("settings:api.error", { status: response.status }));
    throw new SettingsApiError(response.status, message, fields, body.code ?? "");
  }
  return data as T;
}

/** The calls, over a client (the tests give one of their own). */
export function machinesApiWith(client: ReturnType<typeof createApiClient>) {
  return {
    list: () => done(client.GET("/machines")),
    /**
     * A join token for one machine of `role`, shown once; no pool when blank.
     * A lake pool's node (role `lake`) is named by `node`.
     */
    issueJoinToken: (role: string, pool?: string, drives: string[] = [], node?: string) =>
      done(
        client.POST("/machines/join-tokens", {
          body: {
            role,
            pool: pool?.trim() || undefined,
            node: node || undefined,
            drives: drives.length > 0 ? drives : undefined,
          },
          headers: csrf(),
        }),
      ),
    /** What is kept of the lake replica's admin keys: never the secret. */
    replicaAdminKeys: () => done(client.GET("/lake/replica/admin-keys")),
    /** Keep the replica's admin keys as pasted; the answer has no secret. */
    setReplicaAdminKeys: (accessKey: string, secretKey: string) =>
      done(
        client.PUT("/lake/replica/admin-keys", {
          body: { access_key: accessKey, secret_key: secretKey },
          headers: csrf(),
        }),
      ),
    /** The lake replica's machine and what it answers now; no key. */
    replicaState: () => done(client.GET("/lake/replica/state")),
    /** What is kept of the main lake's admin keys: never the secret. */
    mainAdminKeys: () => done(client.GET("/lake/main/admin-keys")),
    /** Keep the main lake's admin keys as pasted; the answer has no secret. */
    setMainAdminKeys: (accessKey: string, secretKey: string) =>
      done(
        client.PUT("/lake/main/admin-keys", {
          body: { access_key: accessKey, secret_key: secretKey },
          headers: csrf(),
        }),
      ),
    /** Where the main lake is reached and what it answers now; no key. */
    mainState: () => done(client.GET("/lake/main/state")),
    /** The lake's pools, their nodes in order and which have joined. */
    lakePools: () => done(client.GET("/lake/pools")),
    /** Define the pool `name` as `nodes` in order, each with `drives` drives. */
    definePool: (name: string, nodes: string[], drives: number) =>
      done(
        client.PUT("/lake/pools/{name}", {
          params: { path: { name } },
          body: { nodes, drives },
          headers: csrf(),
        }),
      ),
    /** Whether the replication can run, and each warehouse's last run; no key. */
    replication: () => done(client.GET("/lake/replication")),
    act: (id: string, action: MachineAction) => {
      const options = { params: { path: { id } }, headers: csrf() };
      switch (action) {
        case "approve":
          return done(client.POST("/machines/{id}/approve", options));
        case "drain":
          return done(client.POST("/machines/{id}/drain", options));
        case "retire":
          return done(client.POST("/machines/{id}/retire", options));
      }
    },
  };
}

export type MachinesApi = ReturnType<typeof machinesApiWith>;

/** The calls over the application's own client. */
export const machinesApi = machinesApiWith(createApiClient());

/**
 * What a person may do to a machine in its state: approve a pending one,
 * drain an approved one, retire any not retired yet (0066 point 4); nothing
 * without `admin:ManageMachines`.
 */
export function machineActions(state: string, manages: boolean): MachineAction[] {
  if (!manages) return [];
  switch (state) {
    case "pending":
      return ["approve", "retire"];
    case "approved":
      return ["drain", "retire"];
    case "draining":
      return ["retire"];
    default:
      return [];
  }
}

/** Do `action` on a machine, then read the list again. */
export async function actOn(
  api: MachinesApi,
  machine: Machine,
  action: MachineAction,
  refresh: () => unknown,
): Promise<string> {
  await api.act(machine.id, action);
  await refresh();
  return t(`settings:machines.done.${action}`, { name: machine.name });
}
