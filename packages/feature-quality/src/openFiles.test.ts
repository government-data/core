import { createEffect, createRoot, createSignal } from "solid-js";
import { describe, expect, it } from "vitest";
import { closed, keyOf, openFiles, opened, pathOf, type OpenFile } from "./openFiles";

const a: OpenFile = { dataset: "loans", file: "amounts.yaml" };
const b: OpenFile = { dataset: "loans", file: "amounts.yaml", draft: "d-1" };
const c: OpenFile = { dataset: "cards", file: "numbers.yaml" };

describe("the work area's tabs", () => {
  it("tells a file of main from the same file on a draft", () => {
    expect(keyOf(a)).not.toBe(keyOf(b));
    expect(pathOf(b)).toBe("/quality/rules/loans/amounts.yaml?draft=d-1");
  });

  it("opens a file in place of the others, keeping only those with something unsaved", () => {
    expect(opened([a], b)).toEqual([b]);
    expect(opened([a, c], a)).toEqual([a]);
    const unsavedC = (o: OpenFile) => keyOf(o) === keyOf(c);
    expect(opened([a, c], b, unsavedC)).toEqual([c, b]);
    expect(opened([c, b], c, unsavedC)).toEqual([c]);
  });

  it("closing one shows the one after it, else the one before, else none", () => {
    expect(closed([a, b, c], b)).toEqual({ open: [a, c], next: c });
    expect(closed([a, b, c], c)).toEqual({ open: [a, b], next: b });
    expect(closed([a], a)).toEqual({ open: [], next: undefined });
  });

  it("a tab closed stays closed while the page that opened it is still on it", () => {
    const [shown] = createSignal(b);
    createRoot(() => createEffect(() => openFiles.open(shown())));
    openFiles.open(c);
    openFiles.close(b);
    expect(openFiles.tabs()).toEqual([c]);
    openFiles.close(c);
  });

  it("keeps what was typed in a tab until it is saved, restored or closed", () => {
    openFiles.open(a);
    openFiles.type(a, "dataset: loans\n");
    expect(openFiles.unsaved(a)).toBe("dataset: loans\n");
    expect(openFiles.isUnsaved(a)).toBe(true);
    expect(openFiles.isUnsaved(b)).toBe(false);
    openFiles.type(a, undefined);
    expect(openFiles.isUnsaved(a)).toBe(false);
    openFiles.type(a, "x");
    openFiles.close(a);
    expect(openFiles.unsaved(a)).toBeUndefined();
    expect(openFiles.tabs()).toEqual([]);
  });
});
