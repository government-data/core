import { createSignal, onCleanup } from "solid-js";
import { rulesApi, type CheckOut, type FindingOut } from "./api";

/** How long typing must pause before the text is checked. */
const PAUSE_MS = 400;

/**
 * A rules file checked as it is typed (decision 0019): `now` checks at
 * once, `typed` once typing pauses; an answer to an older text is dropped.
 * With the file's name, it also says which rules are not in their field's
 * file (decision 0044); on a draft, with its global rules (0045).
 * Call within a component: its timer ends with it.
 */
export function createChecker(at: () => { dataset: string; file?: string; draft?: string }) {
  const [check, setCheck] = createSignal<CheckOut>();
  const [checking, setChecking] = createSignal(false);
  let timer: ReturnType<typeof setTimeout> | undefined;
  let asked = 0;
  onCleanup(() => clearTimeout(timer));

  const now = async (yaml: string) => {
    clearTimeout(timer);
    const mine = ++asked;
    setChecking(true);
    try {
      const { dataset, file, draft } = at();
      const out = await rulesApi.check(dataset, yaml, file, draft);
      if (mine === asked) setCheck(out);
    } finally {
      if (mine === asked) setChecking(false);
    }
  };

  return {
    check,
    checking,
    now,
    typed: (yaml: string) => {
      clearTimeout(timer);
      timer = setTimeout(() => void now(yaml), PAUSE_MS);
    },
    /** The errors the API refused a save with, shown as the check. */
    refused: (findings: FindingOut[]) =>
      setCheck({ rules: check()?.rules ?? [], fields: check()?.fields ?? [], findings }),
    blocked: () => (check()?.findings ?? []).some((f) => f.severity === "error"),
  };
}
