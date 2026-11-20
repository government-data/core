/**
 * A dataset's rules files in and out of the explorer (decision 0051),
 * pure: `main`'s files as one `.zip`, what a person uploads (`.yaml` files
 * or that `.zip`) read back, and which of them would change `main`, each
 * then deployed as any other, read by the linter and confirmed.
 */

import { strFromU8, strToU8, unzipSync, zipSync } from "fflate";

/** A rules file: its name and its text. */
export interface RulesText {
  file: string;
  yaml: string;
}

/** A rules file's name: letters, digits, `_` and `-`, then `.yaml` (as the API asks). */
const NAME = /^[A-Za-z0-9_-]{1,64}\.yaml$/;

/** The files as one `.zip`, each at its name. */
export function zipRules(files: RulesText[]): Uint8Array {
  return zipSync(Object.fromEntries(files.map((f) => [f.file, strToU8(f.yaml)])));
}

/** The name a `.zip` export downloads as. */
export const zipName = (dataset: string, day: string) => `${dataset}-reglas-${day}.zip`;

/**
 * The rules files in what was uploaded: each `.yaml`, and each `.yaml` inside
 * a `.zip` (by its name, folders left aside); anything else is named in
 * `ignored`.
 */
export function readUploaded(uploaded: { name: string; bytes: Uint8Array }[]): {
  files: RulesText[];
  ignored: string[];
} {
  const files: RulesText[] = [];
  const ignored: string[] = [];
  const take = (name: string, bytes: Uint8Array) => {
    const file = name.split("/").pop() ?? name;
    if (NAME.test(file)) files.push({ file, yaml: strFromU8(bytes) });
    else ignored.push(name);
  };
  for (const u of uploaded) {
    if (u.name.toLowerCase().endsWith(".zip")) {
      for (const [name, bytes] of Object.entries(unzipSync(u.bytes))) {
        if (!name.endsWith("/")) take(name, bytes);
      }
    } else take(u.name, u.bytes);
  }
  return { files, ignored };
}

/** What `main` has of a file now; `null` where it has none. */
export interface ToDeploy extends RulesText {
  expected: string | null;
}

/** The uploaded files that would change `main`, in name order; the same file uploaded twice, its last. */
export function changing(uploaded: RulesText[], onMain: RulesText[]): ToDeploy[] {
  const main = new Map(onMain.map((f) => [f.file, f.yaml]));
  const last = new Map(uploaded.map((f) => [f.file, f.yaml]));
  return [...last]
    .filter(([file, yaml]) => main.get(file) !== yaml)
    .map(([file, yaml]) => ({ file, yaml, expected: main.get(file) ?? null }))
    .sort((a, b) => a.file.localeCompare(b.file));
}
