/** Every icon of the profile menu, the same size (and stroke) as the sidebar's. */
export const MENU_ICON = 18;

const MENU_PATHS = {
  docs: "M2 4h6a4 4 0 0 1 4 4v13a3 3 0 0 0-3-3H2zM22 4h-6a4 4 0 0 0-4 4v13a3 3 0 0 1 3-3h7z",
  signOut: "M9 21H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h4M16 17l5-5-5-5M21 12H9",
} as const;

/** A line icon of the profile menu. */
export function MenuIcon(props: { name: keyof typeof MENU_PATHS }) {
  return (
    <svg
      width={MENU_ICON}
      height={MENU_ICON}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width="1.5"
      stroke-linecap="round"
      stroke-linejoin="round"
      aria-hidden="true"
    >
      <path d={MENU_PATHS[props.name]} />
    </svg>
  );
}

/** «Mi cuenta»: a person in a circle, the stroke of the sidebar's icons. */
export function AccountIcon(props: { size?: number }) {
  return (
    <svg
      width={props.size ?? 18}
      height={props.size ?? 18}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width="1.5"
      stroke-linecap="round"
      stroke-linejoin="round"
      aria-hidden="true"
    >
      <circle cx="12" cy="12" r="10" />
      <circle cx="12" cy="10" r="3" />
      <path d="M6.2 18.4a7 7 0 0 1 11.6 0" />
    </svg>
  );
}
