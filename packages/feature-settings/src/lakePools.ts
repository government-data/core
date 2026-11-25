/**
 * Settings → Lake's pool (decision 0029, points 5 and 6; 0066 step 11, the
 * user's rulings 1 to 3 of 2026-11-14): the lake's first pool, `main`,
 * defined before any of its nodes exists, its nodes and which have joined,
 * and each node's command, which names it. The command holds a token:
 * whoever calls {@link requestNodeJoin} keeps it in a component's state
 * alone, as Settings → Execution does.
 */

import { t } from "./i18n";
import { requestJoin, type JoinOutcome } from "./joinRequest";
import type { LakePool, MachinesApi } from "./machinesApi";

/** The lake's first pool, the only one defined from here (ruling 3). */
export const MAIN_POOL = "main";

/** The nodes typed, one per line (or separated by spaces or commas), blanks dropped. */
export function parseNodes(text: string): string[] {
  return text.split(/[\s,]+/).filter((n) => n !== "");
}

/** The pool `main`, if it is defined. */
export async function loadMainPool(api: MachinesApi): Promise<LakePool | undefined> {
  const { pools } = await api.lakePools();
  return pools.find((p) => p.name === MAIN_POOL);
}

/** Whether the pool's layout may still change: none of its nodes has joined. */
export function definable(pool: LakePool | undefined): boolean {
  return !pool?.nodes.some((n) => n.state);
}

/** Define `main` as typed; the API says what is out of form, on its field. */
export async function defineMainPool(
  api: MachinesApi,
  nodesText: string,
  drives: number,
): Promise<LakePool> {
  return api.definePool(MAIN_POOL, parseNodes(nodesText), drives);
}

/**
 * Whether a lake of `nodes` servers with `drives` each grows only by a
 * copy into a new lake (decision 0091, point 3): one server with one drive
 * takes neither more drives nor a pool.
 */
export function growsOnlyByCopy(nodes: number, drives: number): boolean {
  return nodes === 1 && drives === 1;
}

/** One node's line: whether it joined, and in what state. */
export function nodeState(state: string | null | undefined): string {
  return state
    ? t("settings:lake.pool.joined", { state: t(`settings:machines.state.${state}`) })
    : t("settings:lake.pool.notJoined");
}

/** The command of `node` of `main`, with the drives typed or none. */
export function requestNodeJoin(
  api: MachinesApi,
  node: string,
  drivesText: string,
): Promise<JoinOutcome> {
  return requestJoin(api, "lake", MAIN_POOL, drivesText, node);
}
