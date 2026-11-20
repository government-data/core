import { onCleanup } from "solid-js";
import { t } from "@knime-orchestador/i18n";

/** How far an arrow key moves the edge. */
const STEP = 16;

/** The sidebar's edge: drag it to make the sidebar wider or narrower. */
export function SidebarResizer(props: {
  width: number | undefined;
  onResize: (px: number | undefined) => void;
}) {
  let stop = () => {};
  onCleanup(() => stop());
  const start = (e: PointerEvent) => {
    e.preventDefault();
    const sidebar = (e.currentTarget as HTMLElement).parentElement!;
    const left = sidebar.getBoundingClientRect().left;
    const move = (m: PointerEvent) => props.onResize(m.clientX - left);
    stop = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", stop);
      document.body.classList.remove("is-resizing");
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", stop);
    document.body.classList.add("is-resizing");
  };
  const current = (e: KeyboardEvent) =>
    props.width ?? (e.currentTarget as HTMLElement).parentElement!.offsetWidth;
  const key = (e: KeyboardEvent) => {
    if (e.key === "ArrowLeft") props.onResize(current(e) - STEP);
    else if (e.key === "ArrowRight") props.onResize(current(e) + STEP);
    else return;
    e.preventDefault();
  };
  return (
    <div
      class="sidebar__resize"
      role="separator"
      aria-orientation="vertical"
      aria-label={t("web:shell.resize")}
      title={t("web:shell.resize")}
      tabindex={0}
      onPointerDown={start}
      onKeyDown={key}
      onDblClick={() => props.onResize(undefined)}
    />
  );
}
