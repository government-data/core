/**
 * Adding a machine from Settings → Execution (decisions 0029 and 0066 step
 * 11): which roles can be asked for, and what the API's answer lets the page
 * show. The token in it is a secret: whoever calls this keeps the outcome in
 * the component's state alone and drops it when the panel closes.
 */

import type { JoinToken, MachinesApi } from "./machinesApi";

/**
 * The roles Settings → Execution asks a join token for: every machine role
 * but the application VM's (`app`), which gets its certificate at
 * installation (0066 point 16), and a lake pool's node (`lake`), whose token
 * names it and is asked in Settings → Lake (0066 step 11, ruling 1).
 */
export const JOIN_ROLES = ["worker", "lake-replica"] as const;

export type JoinRole = (typeof JOIN_ROLES)[number] | "lake";

/**
 * Whether the form asks for the drives' mounted paths: a lake replica's or
 * a pool node's command can carry a `--drive` for each (0066 step 11). The
 * API refuses drives for any other role.
 */
export function takesDrives(role: JoinRole): boolean {
  return role === "lake-replica" || role === "lake";
}

/** The drives typed in the form, one per line (or separated by spaces), blanks dropped. */
export function parseDrives(text: string): string[] {
  return text.split(/\s+/).filter((d) => d !== "");
}

/**
 * Whether the page offers to add a machine: to whoever may do
 * `admin:ManageMachines`, while the installation has a CA (without one the
 * API issues no token, and the page says so).
 */
export function canAddMachine(manages: boolean, canJoin: boolean): boolean {
  return manages && canJoin;
}

/** What the page shows once, from the API's answer. */
export type JoinOutcome =
  /**
   * The command to run on the new server as root; it holds the token.
   * `addDrives`: the role takes drives and none were given, so the person
   * adds `--drive PATH` for each by hand.
   */
  | {
      kind: "command";
      command: string;
      addDrives: boolean;
      role: string;
      pool?: string;
      expiresAt: number;
    }
  /**
   * The token alone: the API built no command, as nothing installs this
   * role yet or the installation's public address is not HTTPS (the answer
   * does not say which).
   */
  | { kind: "token"; token: string; role: string; pool?: string; expiresAt: number };

/**
 * What the answer lets the page show: the command when there is one;
 * `drivesGiven` says whether the request carried drives.
 */
export function joinOutcome(answer: JoinToken, drivesGiven = false): JoinOutcome {
  const common = {
    role: answer.role,
    pool: answer.pool ?? undefined,
    expiresAt: answer.expires_at,
  };
  const addDrives = !drivesGiven && takesDrives(answer.role as JoinRole);
  return answer.install
    ? { kind: "command", command: answer.install.command, addDrives, ...common }
    : { kind: "token", token: answer.token, ...common };
}

/**
 * Ask for a join token of `role` (and `pool`, if not blank, the drives
 * typed, for a role that takes them, and a pool's `node`) and say what to
 * show.
 */
export async function requestJoin(
  api: MachinesApi,
  role: JoinRole,
  pool: string,
  drivesText = "",
  node?: string,
): Promise<JoinOutcome> {
  const drives = takesDrives(role) ? parseDrives(drivesText) : [];
  const answer = await api.issueJoinToken(role, pool, drives, node);
  return joinOutcome(answer, drives.length > 0);
}
