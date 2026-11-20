import { createEffect, createSignal, on } from "solid-js";
import { rulesApi, type CheckOut } from "./api";
import { tidies } from "./editor";

/** _Tidy_ (decision 0041): the text tidied, shown as one edit, undone like any other. */
export function createTidy(
  text: () => string | undefined,
  show: (yaml: string) => void,
  check: () => CheckOut | undefined,
) {
  // Whether the last _Tidy_ found nothing it could do (a file that does not
  // read for another reason); cleared by the next check.
  const [stuck, setStuck] = createSignal(false);
  createEffect(on(check, () => setStuck(false), { defer: true }));
  return {
    can: () => (check()?.findings ?? []).some((f) => tidies(f.code) || f.code === "QR001"),
    stuck,
    run: () => {
      const yaml = text();
      if (yaml === undefined) return;
      void rulesApi.tidy(yaml).then((out) => {
        if (out.yaml !== yaml) show(out.yaml);
        else setStuck(true);
      });
    },
  };
}
