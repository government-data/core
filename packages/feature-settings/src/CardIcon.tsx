import type { Component, JSX } from "solid-js";

/**
 * The icons of the Settings cards: line icons (24×24, `currentColor` stroke
 * of 1.5), like those of the side menu. Each one is a function: in Solid JSX
 * creates real nodes and a shared one would move from card to card.
 */
const PATHS = {
  link: () => (
    <>
      <path d="M10 13.5a4.5 4.5 0 0 0 6.4 0l3.2-3.2a4.5 4.5 0 0 0-6.4-6.4l-1.2 1.2" />
      <path d="M14 10.5a4.5 4.5 0 0 0-6.4 0l-3.2 3.2a4.5 4.5 0 0 0 6.4 6.4l1.2-1.2" />
    </>
  ),
  users: () => (
    <>
      <circle cx="9" cy="8" r="3.5" />
      <path d="M2.5 20a6.5 6.5 0 0 1 13 0" />
      <path d="M16 4.6a3.5 3.5 0 0 1 0 6.8M18 14.2a6.5 6.5 0 0 1 3.5 5.8" />
    </>
  ),
  variables: () => (
    <>
      <path d="M8 4H7a2 2 0 0 0-2 2v4a2 2 0 0 1-2 2 2 2 0 0 1 2 2v4a2 2 0 0 0 2 2h1" />
      <path d="M16 4h1a2 2 0 0 1 2 2v4a2 2 0 0 0 2 2 2 2 0 0 0-2 2v4a2 2 0 0 1-2 2h-1" />
      <path d="m9.5 9 5 6M14.5 9l-5 6" />
    </>
  ),
  license: () => (
    <>
      <circle cx="12" cy="9" r="5.5" />
      <path d="M8.6 13.4 7.5 21l4.5-2.5 4.5 2.5-1.1-7.6" />
    </>
  ),
  updates: () => (
    <>
      <path d="M20 12a8 8 0 0 1-14.3 4.9M4 12a8 8 0 0 1 14.3-4.9" />
      <path d="M18.5 3v4.5H14M5.5 21v-4.5H10" />
    </>
  ),
  directory: () => (
    <>
      <rect x="9" y="3" width="6" height="5" rx="1" />
      <rect x="3" y="16" width="6" height="5" rx="1" />
      <rect x="15" y="16" width="6" height="5" rx="1" />
      <path d="M12 8v4M6 16v-4h12v4" />
    </>
  ),
  mail: () => (
    <>
      <rect x="3" y="5" width="18" height="14" rx="2" />
      <path d="m3.5 7 8.5 6 8.5-6" />
    </>
  ),
  shield: () => (
    <>
      <path d="M12 3 4.5 6v5.5c0 4.5 3.2 8.2 7.5 9.5 4.3-1.3 7.5-5 7.5-9.5V6z" />
      <path d="m9 12 2 2 4-4" />
    </>
  ),
  userPlus: () => (
    <>
      <circle cx="9" cy="8" r="3.5" />
      <path d="M2.5 20a6.5 6.5 0 0 1 13 0" />
      <path d="M19 8v6M16 11h6" />
    </>
  ),
  shieldPlus: () => (
    <>
      <path d="M12 3 4.5 6v5.5c0 4.5 3.2 8.2 7.5 9.5 4.3-1.3 7.5-5 7.5-9.5V6z" />
      <path d="M12 9v6M9 12h6" />
    </>
  ),
  // Users and access: a person with a shield.
  access: () => (
    <>
      <circle cx="9" cy="7.5" r="3.5" />
      <path d="M2.5 20a6.5 6.5 0 0 1 10-5.5" />
      <path d="M18 12.5 14.5 14v2.5c0 2.2 1.5 3.8 3.5 4.5 2-.7 3.5-2.3 3.5-4.5V14z" />
    </>
  ),
  // Groups: three people.
  group: () => (
    <>
      <circle cx="12" cy="7.5" r="3" />
      <circle cx="5" cy="10" r="2.2" />
      <circle cx="19" cy="10" r="2.2" />
      <path d="M6.5 20a5.5 5.5 0 0 1 11 0" />
      <path d="M1.5 19a3.6 3.6 0 0 1 4.6-3.5M22.5 19a3.6 3.6 0 0 0-4.6-3.5" />
    </>
  ),
  // Policies: a page with a check.
  policy: () => (
    <>
      <path d="M14 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8z" />
      <path d="M14 3v5h5" />
      <path d="m9 14 2 2 4-4" />
    </>
  ),
  // Service accounts: a key.
  key: () => (
    <>
      <circle cx="8" cy="15" r="4.5" />
      <path d="m11.2 11.8 9.3-9.3M17 6l2.5 2.5M14.5 8.5 16.5 10.5" />
    </>
  ),
  // Execution: a rack of two machines.
  server: () => (
    <>
      <rect x="3.5" y="4" width="17" height="6.5" rx="1.5" />
      <rect x="3.5" y="13.5" width="17" height="6.5" rx="1.5" />
      <path d="M7 7.25h.01M7 16.75h.01M11 7.25h6M11 16.75h6" />
    </>
  ),
} satisfies Record<string, () => JSX.Element>;

export type CardIconName = keyof typeof PATHS;

export const CardIcon: Component<{ name: CardIconName; size?: number }> = (props) => (
  <svg
    width={props.size ?? 22}
    height={props.size ?? 22}
    viewBox="0 0 24 24"
    fill="none"
    stroke="currentColor"
    stroke-width="1.5"
    stroke-linecap="round"
    stroke-linejoin="round"
    aria-hidden="true"
  >
    {PATHS[props.name]()}
  </svg>
);
