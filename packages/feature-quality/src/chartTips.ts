/**
 * The charts' tooltip in the application (decision 0031), as the guide's
 * `Chart.astro` shows it: each mark's value first, then what it is (its
 * `data-v` and `data-l`), on hover and on keyboard focus alike, inside the
 * element given. Returns what removes it.
 */
export function chartTips(root: HTMLElement): () => void {
  const tip = document.createElement("div");
  tip.className = "viz-tip";
  tip.hidden = true;
  const value = document.createElement("strong");
  const label = document.createElement("span");
  tip.append(value, label);
  document.body.append(tip);

  const show = (el: Element, x: number, y: number) => {
    const data = (el as HTMLElement | SVGElement).dataset;
    value.textContent = data.v ?? "";
    label.textContent = data.l ?? "";
    tip.hidden = false;
    const box = tip.getBoundingClientRect();
    let left = x + 14;
    if (left + box.width > innerWidth - 8) left = x - box.width - 14;
    tip.style.left = `${Math.max(8, left)}px`;
    tip.style.top = `${Math.max(8, y - box.height - 12)}px`;
  };
  const target = (e: Event) => (e.target instanceof Element ? e.target.closest(".viz-hit") : null);
  const move = (e: PointerEvent) => {
    const el = target(e);
    if (el) show(el, e.clientX, e.clientY);
    else tip.hidden = true;
  };
  const focus = (e: FocusEvent) => {
    const el = target(e);
    if (!el) return;
    const box = el.getBoundingClientRect();
    show(el, box.left + box.width / 2, box.top);
  };
  const hide = () => (tip.hidden = true);

  root.addEventListener("pointermove", move);
  root.addEventListener("pointerleave", hide);
  root.addEventListener("focusin", focus);
  root.addEventListener("focusout", hide);
  return () => {
    root.removeEventListener("pointermove", move);
    root.removeEventListener("pointerleave", hide);
    root.removeEventListener("focusin", focus);
    root.removeEventListener("focusout", hide);
    tip.remove();
  };
}
