import { hoverTooltip } from "@codemirror/view";
import { context } from "./completion";
import { help } from "./help";

const WORD = /[\w-]/;

/** What a key or a value is for, with the pointer over it. */
export const hoverHelp = hoverTooltip((view, pos) => {
  const line = view.state.doc.lineAt(pos);
  const text = line.text;
  let start = pos - line.from;
  let end = start;
  while (start > 0 && WORD.test(text[start - 1]!)) start--;
  while (end < text.length && WORD.test(text[end]!)) end++;
  if (start === end) return null;
  const word = text.slice(start, end);
  const ctx = context(view.state.doc.toString(), line.from + end);
  const info = ctx && ctx.word === word ? help(ctx, word) : undefined;
  if (!info) return null;
  return {
    pos: line.from + start,
    end: line.from + end,
    above: true,
    create: () => {
      const dom = document.createElement("div");
      dom.className = "qd-hover";
      dom.textContent = info;
      return { dom };
    },
  };
});
