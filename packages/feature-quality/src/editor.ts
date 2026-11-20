/**
 * The rule editor's pure parts (decision 0019, _Rule_): where a finding
 * falls in the text, and the line a rule starts on.
 */

import type { FindingOut } from "./api";

/** A finding where the editor marks it: offsets into the text. */
export interface Mark {
  from: number;
  to: number;
  severity: "error" | "warning";
  message: string;
}

/** The offset each line starts at (line 1 at index 0). */
function lineStarts(text: string): number[] {
  const starts = [0];
  for (let i = 0; i < text.length; i++) if (text[i] === "\n") starts.push(i + 1);
  return starts;
}

/**
 * Each finding marked on `text`: from its column to the end of its line;
 * without a place, on the first line. Places past the text are clamped:
 * the text may have changed since it was checked.
 */
export function marks(text: string, findings: FindingOut[]): Mark[] {
  const starts = lineStarts(text);
  return findings.map((f) => {
    const line = Math.min(Math.max((f.line ?? 1) - 1, 0), starts.length - 1);
    const start = starts[line];
    const end = line + 1 < starts.length ? starts[line + 1] - 1 : text.length;
    const from = Math.min(start + Math.max((f.column ?? 1) - 1, 0), end);
    return {
      from,
      to: Math.max(end, from),
      severity: f.severity === "error" ? "error" : "warning",
      message: `${f.code}: ${f.message}`,
    };
  });
}

/** The line (from 1) a rule `id` starts on, by its `id:` key. */
export function ruleLine(text: string, id: string): number | undefined {
  const lines = text.split("\n");
  const at = lines.findIndex((l) => {
    const m = /^\s*(?:-\s+)?id:\s*["']?([^"'\s#]+)["']?\s*(?:#.*)?$/.exec(l);
    return m?.[1] === id;
  });
  return at < 0 ? undefined : at + 1;
}

/**
 * A finding about how the file only looks (decision 0041, `QR200`–`QR205`)
 * or where a rule is kept (0044, `QR210`); `QR206`, a value lost to a
 * comment, changes what it says.
 */
export const isStyle = (code: string) => /^QR(20[0-5]|210)$/.test(code);

/** The findings _Tidy_ takes away (decision 0041). */
const TIDIED = new Set(["QR201", "QR202", "QR203", "QR204", "QR205", "QR206", "QR208", "QR209"]);
export const tidies = (code: string) => TIDIED.has(code);
