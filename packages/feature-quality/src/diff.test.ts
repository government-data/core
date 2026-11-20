import { describe, expect, it } from "vitest";
import { counts, hunks, inline, lineDiff, sideBySide } from "./diff";

const kinds = (before: string, after: string) =>
  lineDiff(before, after).map((l) => `${l.kind[0]} ${l.text}`);

describe("lineDiff", () => {
  it("keeps, takes out and puts in, in order, with line numbers", () => {
    expect(kinds("a\nb\nc\n", "a\nx\nc\n")).toEqual(["s a", "d b", "a x", "s c"]);
    const lines = lineDiff("a\nb\n", "a\nb\nc\n");
    expect(lines[2]).toEqual({ kind: "add", text: "c", after: 3 });
    expect(lines[1]).toEqual({ kind: "same", text: "b", before: 2, after: 2 });
  });

  it("reads a file made or removed", () => {
    expect(kinds("", "a\nb")).toEqual(["a a", "a b"]);
    expect(counts(lineDiff("a\nb\n", ""))).toEqual({ added: 0, removed: 2 });
  });
});

describe("hunks", () => {
  it("keeps a few lines around each change and drops the rest", () => {
    const before = Array.from({ length: 20 }, (_, i) => `l${i}`).join("\n");
    const after = before.replace("l2\n", "L2\n").replace("l17\n", "L17\n");
    const parts = hunks(lineDiff(before, after), 2);
    expect(parts).toHaveLength(2);
    expect(parts[0]!.map((l) => l.text)).toEqual(["l0", "l1", "l2", "L2", "l3", "l4"]);
  });
});

describe("sideBySide", () => {
  it("puts a kept line on both sides and a changed one beside its new text", () => {
    const rows = sideBySide(lineDiff("a\nb\nc\n", "a\nB\nX\nc\n"));
    expect(rows.map((r) => [r.left?.text, r.right?.text])).toEqual([
      ["a", "a"],
      ["b", "B"],
      [undefined, "X"],
      ["c", "c"],
    ]);
  });
});

describe("inline", () => {
  it("marks the words that changed inside a line", () => {
    const [left, right] = inline('    severity: "major"', "    severity: minor");
    expect(left.filter((p) => p.changed).map((p) => p.text)).toEqual(['"major"']);
    expect(right.filter((p) => p.changed).map((p) => p.text)).toEqual(["minor"]);
    expect(left.map((p) => p.text).join("")).toBe('    severity: "major"');
  });
});
