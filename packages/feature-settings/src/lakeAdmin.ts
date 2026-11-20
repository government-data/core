/**
 * Settings → Lake's admin keys (decision 0028, point 2; 0066 step 11, the
 * user's rulings of 2026-11-13): `join.sh` makes the platform's admin user
 * on the replica, `infra/rustfs/admin-user.sh` on the main lake, and each
 * shows its keys there once; the person pastes them here and the API keeps
 * them sealed. The secret lives only in
 * the form's state and is dropped once kept; the API never answers it.
 */

import type { LakeAdminKeys, MachinesApi } from "./machinesApi";

/** The access key `join.sh` shows for the platform's admin user. */
export const ADMIN_ACCESS_KEY = "qd-platform-admin";

/** Which lake's keys: the replica's, or the main lake's (`pool:main`). */
export type LakeKeys = "replica" | "main";

/** Keep the keys as pasted, spaces around each dropped. */
export function saveAdminKeys(
  api: MachinesApi,
  lake: LakeKeys,
  accessKey: string,
  secretKey: string,
): Promise<LakeAdminKeys> {
  const [access, secret] = [accessKey.trim(), secretKey.trim()];
  return lake === "main"
    ? api.setMainAdminKeys(access, secret)
    : api.setReplicaAdminKeys(access, secret);
}

/** Keep the replica's keys as pasted. */
export function saveReplicaAdminKeys(
  api: MachinesApi,
  accessKey: string,
  secretKey: string,
): Promise<LakeAdminKeys> {
  return saveAdminKeys(api, "replica", accessKey, secretKey);
}

/** What is kept of `lake`'s keys: never the secret. */
export function loadAdminKeys(api: MachinesApi, lake: LakeKeys): Promise<LakeAdminKeys> {
  return lake === "main" ? api.mainAdminKeys() : api.replicaAdminKeys();
}

/** What the page says is kept: nothing yet, or the access key and since when (ms). */
export type KeptState = { kind: "none" } | { kind: "kept"; accessKey: string; setAt: number };

export function keptState(kept: LakeAdminKeys | undefined): KeptState {
  return kept?.set && kept.access_key && kept.set_at
    ? { kind: "kept", accessKey: kept.access_key, setAt: kept.set_at * 1000 }
    : { kind: "none" };
}
