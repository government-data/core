/**
 * A state's tone and its icon (decision 0040): good, warning, serious,
 * critical, or neutral. A tone's color never carries the meaning alone: it
 * comes with its icon and a label, for color blindness, print and forced
 * colors. The colors are the theme's `--status-*` tokens.
 */
export type Tone = "good" | "warning" | "serious" | "critical" | "neutral";

const ICONS: Record<Tone, string> = {
  good: "✓",
  warning: "◐",
  serious: "⊘",
  critical: "✕",
  neutral: "•",
};

/** The icon shown with a tone. */
export function statusIcon(tone: Tone): string {
  return ICONS[tone];
}
