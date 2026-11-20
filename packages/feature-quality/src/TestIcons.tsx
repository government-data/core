import { Match, Switch, type Component, type JSX } from "solid-js";

/** What a rule did with a record. */
export type Outcome = "fails" | "passes" | "skips";

/** A 16-pixel line icon, as the explorer's (`NavIcon`). */
const Icon: Component<{ children: JSX.Element }> = (props) => (
  <svg
    width="16"
    height="16"
    viewBox="0 0 24 24"
    fill="none"
    stroke="currentColor"
    stroke-width="2"
    stroke-linecap="round"
    stroke-linejoin="round"
    aria-hidden="true"
  >
    {props.children}
  </svg>
);

/** A filter: the usual funnel. */
export const FilterIcon: Component = () => (
  <Icon>
    <path d="M3 4h18l-7 8.5V19l-4 2v-8.5L3 4Z" />
  </Icon>
);

/**
 * An outcome of a rule on a record (decision 0043): fails (a cross),
 * passes (a check), does not apply (a dash), or every one (a list).
 */
export const OutcomeIcon: Component<{ of?: Outcome }> = (props) => (
  <Icon>
    <Switch fallback={<path d="M4 6h16M4 12h16M4 18h16" />}>
      <Match when={props.of === "fails"}>
        <circle cx="12" cy="12" r="9" />
        <path d="m9 9 6 6M15 9l-6 6" />
      </Match>
      <Match when={props.of === "passes"}>
        <circle cx="12" cy="12" r="9" />
        <path d="m8 12 3 3 5-6" />
      </Match>
      <Match when={props.of === "skips"}>
        <circle cx="12" cy="12" r="9" />
        <path d="M8 12h8" />
      </Match>
    </Switch>
  </Icon>
);

/** A branch, as git draws one: a history, its drafts (decision 0050). */
export const BranchIcon: Component = () => (
  <Icon>
    <circle cx="6" cy="5" r="2" />
    <circle cx="6" cy="19" r="2" />
    <circle cx="18" cy="7" r="2" />
    <path d="M6 7v10M18 9c0 5-6 4-12 8" />
  </Icon>
);
