import { A } from "@solidjs/router";
import { Show, type Component, type JSX } from "solid-js";
import { GROUP } from "./icons-traced";

/**
 * Each icon is a function: in Solid JSX creates real DOM nodes, and a shared
 * node would move from one row to another instead of repeating.
 *
 * Line icons (24×24, `currentColor` stroke) and row action buttons. In our own
 * SVG so as not to bring a library for half a dozen icons, and so they take
 * the theme's color in light and dark.
 */

const PATHS = {
  play: () => <path d="M7 5v14l11-7z" fill="currentColor" stroke="none" />,
  stop: () => (
    <rect x="6" y="6" width="12" height="12" rx="1.5" fill="currentColor" stroke="none" />
  ),
  log: () => (
    <>
      <path d="M14 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8z" />
      <path d="M14 3v5h5M9 13h6M9 17h6M9 9h2" />
    </>
  ),
  clock: () => (
    <>
      <circle cx="12" cy="12" r="9" />
      <path d="M12 7v5l3 2" />
    </>
  ),
  details: () => (
    <>
      <circle cx="12" cy="12" r="9" />
      <path d="M12 11v6M12 7.5v.5" />
    </>
  ),
  pause: () => <path d="M9 6v12M15 6v12" />,
  resume: () => <path d="M8 5v14l11-7z" />,
  trash: () => <path d="M4 7h16M10 11v6M14 11v6M6 7l1 13h10l1-13M9 7V4h6v3" />,
  table: () => <path d="M4 5h16v14H4zM4 10h16M4 15h16M10 10v9" />,
  list: () => <path d="M9 6h11M9 12h11M9 18h11M4.5 6h.01M4.5 12h.01M4.5 18h.01" />,
  plus: () => <path d="M12 5v14M5 12h14" />,
  pencil: () => <path d="M4 20h4L19 9l-4-4L4 16v4zM14 6l4 4" />,
  upload: () => <path d="M12 16V4M7 9l5-5 5 5M4 16v4h16v-4" />,
  spinner: () => <path d="M12 3a9 9 0 1 0 9 9" />,
  layers: () => <path d="M12 3 2 8l10 5 10-5zM2 13l10 5 10-5M2 18l10 5 10-5" />,
  users: () => (
    <>
      <circle cx="9" cy="8" r="3.5" />
      <path d="M2.5 20a6.5 6.5 0 0 1 13 0M16 4.5a3.5 3.5 0 0 1 0 7M18 14.5a6.5 6.5 0 0 1 3.5 5.5" />
    </>
  ),
  mail: () => <path d="M3 6h18v12H3zM3 7l9 6 9-6" />,
  branch: () => (
    <path d="M6 3v12M6 21a3 3 0 1 0 0-6 3 3 0 0 0 0 6zM18 9a3 3 0 1 0 0-6 3 3 0 0 0 0 6zM18 9a9 9 0 0 1-9 9" />
  ),
  copy: () => <path d="M9 9h11v11H9zM5 15H4V4h11v1" />,
  check: () => <path d="M5 12l5 5 9-10" />,
  chevronLeft: () => <path d="M15 6l-6 6 6 6" />,
  chevronRight: () => <path d="M9 6l6 6-6 6" />,
  terminal: () => <path d="M4 5h16v14H4zM8 10l3 2-3 2M13 15h3" />,
  link: () => (
    <path d="M10 14a4 4 0 0 0 5.7 0l3-3a4 4 0 0 0-5.7-5.7l-1 1M14 10a4 4 0 0 0-5.7 0l-3 3a4 4 0 0 0 5.7 5.7l1-1" />
  ),
  /** Back: an arrow to the left. */
  back: () => <path d="M19 12H5M11 18l-6-6 6-6" />,
  /** Areas: two people shaking hands (with an outline that thickens the stroke). */
  areas: () => (
    <g
      transform={GROUP.transform}
      fill="currentColor"
      stroke="currentColor"
      stroke-width="45"
      stroke-linejoin="round"
    >
      {GROUP.paths.map((d) => (
        <path d={d} />
      ))}
    </g>
  ),
  /** The flows: a stem that opens into three paths. */
  flows: () => (
    <>
      <circle cx="12" cy="20.25" r="1.75" />
      <path d="M12 18.5V3M9.5 5.5 12 3l2.5 2.5" />
      <path d="M12 15c0-2.5-2-4-5-4H2.75M5.25 8.5 2.75 11l2.5 2.5" />
      <path d="M12 15c0-2.5 2-4 5-4h4.25M18.75 8.5l2.5 2.5-2.5 2.5" />
    </>
  ),
} satisfies Record<string, () => JSX.Element>;

export type IconName = keyof typeof PATHS;

export const Icon: Component<{ name: IconName; size?: number; stroke?: number }> = (props) => (
  <svg
    width={props.size ?? 16}
    height={props.size ?? 16}
    viewBox="0 0 24 24"
    fill="none"
    stroke="currentColor"
    stroke-width={props.stroke ?? 2}
    stroke-linecap="round"
    stroke-linejoin="round"
    aria-hidden="true"
  >
    {PATHS[props.name]()}
  </svg>
);

/** An icon as a glyph of the side menu (`NavItem.icon`), with a thinner
 * stroke so it weighs the same as the traced ones. */
export const navIcon =
  (name: IconName): Component<{ size?: number }> =>
  (props) => <Icon name={name} size={props.size} stroke={1.5} />;

type Tone = "default" | "primary" | "danger";

/**
 * A square button with an icon. The label goes in `aria-label` and in `title`
 * (tooltip): an icon alone tells a screen reader nothing.
 */
export const IconButton: Component<{
  icon: IconName;
  label: string;
  onClick?: () => void;
  href?: string;
  tone?: Tone;
  disabled?: boolean;
  /** In progress: shows a spinner instead of the icon and blocks the button. */
  busy?: boolean;
  /** A toggle button (e.g. the chosen view): marked and `aria-pressed`. */
  pressed?: boolean;
}> = (props) => {
  const cls = () =>
    `kn-iconbtn kn-iconbtn--${props.tone ?? "default"}${props.busy ? " kn-iconbtn--busy" : ""}${
      props.pressed ? " kn-iconbtn--pressed" : ""
    }`;
  return (
    <Show
      when={props.href && !props.disabled}
      fallback={
        <button
          type="button"
          class={cls()}
          aria-label={props.label}
          title={props.label}
          disabled={props.disabled || props.busy}
          aria-busy={props.busy ? "true" : undefined}
          aria-pressed={props.pressed === undefined ? undefined : props.pressed}
          onClick={() => props.onClick?.()}
        >
          <Show when={props.busy} fallback={<Icon name={props.icon} />}>
            <span class="kn-spin">
              <Icon name="spinner" />
            </span>
          </Show>
        </button>
      }
    >
      <A class={cls()} href={props.href!} aria-label={props.label} title={props.label}>
        <Icon name={props.icon} />
      </A>
    </Show>
  );
};

/** A group of actions at the end of a row. */
export const RowActions: Component<{ children: JSX.Element }> = (props) => (
  <div class="kn-rowactions">{props.children}</div>
);
