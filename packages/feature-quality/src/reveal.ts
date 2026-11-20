/**
 * The reason for a view of personal data (decision 0069, points 5 to 7),
 * pure but for the tab's storage: the purposes, the reference's check, the
 * reason kept per scope for the tab's life, and why a reveal was refused.
 */

import { createSignal } from "solid-js";
import type { components } from "@knime-orchestador/api-client";
import { QualityError } from "./apiCore";

export type ViewPurpose = components["schemas"]["ViewPurpose"];

/** Why values are shown: a purpose and, for a ticket or case, a reference. */
export interface Reason {
  purpose: ViewPurpose;
  reference: string | null;
}

// Every key of the client's enum, and only those: a key added there fails
// the build here until it is offered (and its texts written).
const OFFERED: Record<ViewPurpose, true> = {
  quality_review: true,
  rule_writing: true,
  remediation: true,
  client_request: true,
  audit: true,
  other: true,
};

/** The purposes offered, in the order the dialog lists them. */
export const PURPOSES = Object.keys(OFFERED) as ViewPurpose[];

/** Where a kept reason applies: a source's pages, a dataset's tests, or the console. */
export const scopes = {
  source: (name: string) => `source/${name}`,
  dataset: (dataset: string) => `pipeline/${dataset}`,
  lake: "lake",
};

/** A ticket or case number (point 7): at most 32 ASCII characters, at least one letter. */
const REFERENCE = /^(?=.*[A-Za-z])[A-Za-z0-9][A-Za-z0-9_\-/#.]{0,31}$/;

/**
 * What is wrong with `reference` for `purpose`: `reference` when it has not
 * the form, `required` when it is empty and the purpose is `other`.
 */
export function referenceProblem(
  purpose: ViewPurpose,
  reference: string,
): "reference" | "required" | undefined {
  const given = reference.trim();
  if (!given) return purpose === "other" ? "required" : undefined;
  return REFERENCE.test(given) ? undefined : "reference";
}

/** The reason as it is sent: an empty reference is absent. */
export const reasonOf = (purpose: ViewPurpose, reference: string): Reason => ({
  purpose,
  reference: reference.trim() || null,
});

const PREFIX = "qd.reveal.";
// Where the tab's storage cannot be used, the reason lasts while the app is open.
const memory = new Map<string, Reason>();
// What reads a kept reason follows each change.
const [changes, changed] = createSignal(0);

const valid = (r: Partial<Reason> | null): r is Reason =>
  !!r &&
  PURPOSES.includes(r.purpose as ViewPurpose) &&
  referenceProblem(r.purpose as ViewPurpose, r.reference ?? "") === undefined;

/** The reason kept for `scope` in this tab, if any. */
export function kept(scope: string): Reason | undefined {
  changes();
  const known = memory.get(scope);
  if (known) return known;
  try {
    const stored = JSON.parse(sessionStorage.getItem(PREFIX + scope) ?? "null") as Reason | null;
    return valid(stored) ? stored : undefined;
  } catch {
    return undefined;
  }
}

/** Keeps `reason` for `scope` until the tab closes, _Mask again_ or sign-out. */
export function keep(scope: string, reason: Reason) {
  memory.set(scope, reason);
  try {
    sessionStorage.setItem(PREFIX + scope, JSON.stringify(reason));
  } catch {
    // Kept in memory only: a reload asks again.
  }
  changed((n) => n + 1);
}

/** Forgets the reason of `scope` (_Mask again_). */
export function forget(scope: string) {
  memory.delete(scope);
  try {
    sessionStorage.removeItem(PREFIX + scope);
  } catch {
    // Nothing was kept there.
  }
  changed((n) => n + 1);
}

/** Forgets every reason kept in this tab: at sign-out. */
export function forgetAll() {
  memory.clear();
  try {
    for (let i = sessionStorage.length - 1; i >= 0; i--) {
      const key = sessionStorage.key(i);
      if (key?.startsWith(PREFIX)) sessionStorage.removeItem(key);
    }
  } catch {
    // Nothing was kept there.
  }
  changed((n) => n + 1);
}

const CODES = ["forbidden", "not_done", "purpose", "reference", "audit_unavailable", "unavailable"];

/** Why the API refused a reveal, as the screen names it (`reveal.refused.<code>`). */
export function revealRefusal(e: unknown): string {
  if (!(e instanceof QualityError)) return "other";
  if (e.status === 404) return "not_found";
  const error = (e.body as { error?: string } | undefined)?.error;
  return error && CODES.includes(error) ? error : "other";
}

/**
 * Posts a reveal with `reason`: the values whole, or why not. A reason the
 * API refuses (422) is forgotten, so the dialog asks again.
 */
export async function revealWith<T>(
  scope: string,
  reason: Reason,
  post: (reason: Reason) => Promise<T>,
): Promise<{ out: T } | { refused: string }> {
  try {
    return { out: await post(reason) };
  } catch (e) {
    const refused = revealRefusal(e);
    if (refused === "purpose" || refused === "reference") forget(scope);
    return { refused };
  }
}

/** A finished job as a view shows it: its read, masked, and what is shown now. */
export interface Revealing<T> {
  job: string;
  masked: T;
  out: T;
  busy?: boolean;
  refused?: string;
}

/**
 * Reveals `view` with `reason` (the one kept for `scope`, if not given)
 * when its read says it may be: one post, its answer or refusal given to
 * `set`. Without a reason, or when it may not, nothing is posted.
 */
export async function reveal<T extends { revealable: boolean }>(
  scope: string,
  view: Revealing<T>,
  post: (reason: Reason) => Promise<T>,
  set: (view: Revealing<T>) => void,
  reason = kept(scope),
) {
  if (!reason || !view.masked.revealable) return;
  set({ ...view, busy: true, refused: undefined });
  const res = await revealWith(scope, reason, post);
  set("out" in res ? { ...view, out: res.out } : { ...view, refused: res.refused });
}

/**
 * What the bar above values shows (decision 0069, point 5, and the ruling of
 * 2026-11-13): `showing` ("Showing values for: …", _Change_, _Mask again_)
 * only while the values are whole and a reason is kept; `ask` (masked,
 * _Show values_) while they are masked and may be shown; `retry` (masked,
 * _Change_ to try again or give another reason, _Forget reason_) while they
 * are masked with a reason kept, as after a refused reveal; else nothing.
 */
export function barState(
  reason: boolean,
  revealable: boolean,
  masked: boolean,
): "showing" | "ask" | "retry" | "none" {
  if (!masked) return reason ? "showing" : "none";
  if (!revealable) return "none";
  return reason ? "retry" : "ask";
}

/** `view` masked again (_Mask again_). */
export const masked = <T>(view: Revealing<T>): Revealing<T> => ({
  ...view,
  out: view.masked,
  busy: false,
  refused: undefined,
});
