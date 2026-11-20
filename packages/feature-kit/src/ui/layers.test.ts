import { describe, expect, it } from "vitest";
import { opened, takesEscape } from "./layers";

/** A keydown as the dialogs' listeners on `document` see it. */
const key = (k: string) => {
  const e = { key: k, defaultPrevented: false, preventDefault: () => (e.defaultPrevented = true) };
  return e;
};

describe("dialog layers", () => {
  it("lets only the top dialog take Escape, whichever listener runs first", () => {
    const below = opened();
    const above = opened();
    // The one below registered first: it runs first and leaves it.
    let e = key("Escape");
    expect(takesEscape(below, e)).toBe(false);
    expect(takesEscape(above, e)).toBe(true);
    // Had the top one run first, the one below sees it taken.
    e = key("Escape");
    expect(takesEscape(above, e)).toBe(true);
    above.closed();
    expect(takesEscape(below, e)).toBe(false);
    // The top closed: the next Escape is the one below's.
    expect(takesEscape(below, key("Escape"))).toBe(true);
    below.closed();
  });

  it("takes no other key, and forgets a dialog closed out of order", () => {
    const a = opened();
    const b = opened();
    expect(takesEscape(b, key("Enter"))).toBe(false);
    a.closed();
    expect(b.top()).toBe(true);
    b.closed();
    b.closed();
    const c = opened();
    expect(c.top()).toBe(true);
    c.closed();
  });
});
