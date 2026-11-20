import { statusIcon, type Tone } from "./status";

/**
 * A state as a chip: a dot of its tone, its icon and its label (the label
 * comes translated from the caller). Screen readers read the label only.
 */
export function Status(props: { tone: Tone; label: string }) {
  return (
    <span class={`fk-status fk-status--${props.tone}`}>
      <i class="fk-status__dot" aria-hidden="true" />
      <span aria-hidden="true">{statusIcon(props.tone)}</span>
      <span>{props.label}</span>
    </span>
  );
}
