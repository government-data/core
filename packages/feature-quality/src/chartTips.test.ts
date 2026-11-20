// @vitest-environment jsdom
import { describe, expect, it } from "vitest";
import { chartTips } from "./chartTips";

describe("the charts' tooltip", () => {
  it("shows a mark's value and label on hover, hides off it, and is removed", () => {
    const root = document.createElement("div");
    root.innerHTML = `<svg><g class="viz-hit" data-l="O-1" data-v="12,5 %"><rect/></g></svg><p>off</p>`;
    document.body.append(root);
    const remove = chartTips(root);
    const tip = document.querySelector(".viz-tip") as HTMLElement;
    root
      .querySelector("rect")!
      .dispatchEvent(new MouseEvent("pointermove", { bubbles: true, clientX: 10, clientY: 50 }));
    expect(tip.hidden).toBe(false);
    expect(tip.textContent).toBe("12,5 %O-1");
    root.querySelector("p")!.dispatchEvent(new MouseEvent("pointermove", { bubbles: true }));
    expect(tip.hidden).toBe(true);
    remove();
    expect(document.querySelector(".viz-tip")).toBeNull();
  });
});
