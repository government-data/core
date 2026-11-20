import { createResource, createSignal, Show, type Component } from "solid-js";
import { toast } from "@knime-orchestador/feature-kit/ui";
import { may, type RulesOut } from "./api";
import { DeployConfirm } from "./DeployConfirm";
import { who } from "./explorerCache";
import { t } from "./i18n";
import { notify, warn } from "./Notify";
import { changing, readUploaded, zipName, zipRules, type ToDeploy } from "./rulesArchive";

/** Saves `bytes` as a file the browser downloads. */
function download(name: string, bytes: Uint8Array) {
  const url = URL.createObjectURL(
    new Blob([bytes as Uint8Array<ArrayBuffer>], { type: "application/zip" }),
  );
  const a = Object.assign(document.createElement("a"), { href: url, download: name });
  a.click();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
}

/**
 * _Export_ a dataset's rules files, as `main` has them, in one `.zip`; and,
 * for whoever may deploy them, _Import_ `.yaml` files or such a `.zip`
 * (decision 0051): each file that would change `main` is read by the
 * linter and confirmed, one after another, as a deploy is; one cancelled
 * is left out.
 */
export const RulesIO: Component<{ dataset: string; rules: RulesOut }> = (props) => {
  const [me] = createResource(who);
  const [queue, setQueue] = createSignal<ToDeploy[]>([]);
  const [total, setTotal] = createSignal(0);
  const [deployed, setDeployed] = createSignal(0);
  const files = () => props.rules.files.map((f) => ({ file: f.file, yaml: f.yaml }));

  const exportZip = () =>
    download(zipName(props.dataset, new Date().toLocaleDateString("en-CA")), zipRules(files()));

  const importFiles = async (list: FileList | null) => {
    if (!list?.length) return;
    const uploaded = await Promise.all(
      [...list].map(async (f) => ({ name: f.name, bytes: new Uint8Array(await f.arrayBuffer()) })),
    );
    let read;
    try {
      read = readUploaded(uploaded);
    } catch {
      return warn(t("quality:rulesIo.unreadable"));
    }
    if (read.ignored.length) warn(t("quality:rulesIo.ignored", { files: read.ignored.join(", ") }));
    const todo = changing(read.files, files());
    if (!todo.length) return toast("info", t("quality:rulesIo.nothing"));
    setTotal(todo.length);
    setDeployed(0);
    setQueue(todo);
  };

  // One file done, deployed or left out: the next, or what came of all.
  const next = () => {
    const rest = queue().slice(1);
    setQueue(rest);
    if (!rest.length) notify(t("quality:rulesIo.done", { count: deployed(), total: total() }));
  };

  return (
    <div class="qd-actions">
      <button type="button" class="fk-btn" onClick={exportZip}>
        ⤓ {t("quality:rulesIo.export")}
      </button>
      <Show when={may(me(), "quality:ApproveRules") && may(me(), "quality:ManageDatasets")}>
        <label class="fk-btn">
          ⤒ {t("quality:rulesIo.import")}
          <input
            type="file"
            accept=".yaml,.yml,.zip"
            multiple
            hidden
            onChange={(e) => {
              void importFiles(e.currentTarget.files);
              e.currentTarget.value = "";
            }}
          />
        </label>
      </Show>
      <Show when={queue()[0]} keyed>
        {(f) => (
          <DeployConfirm
            deploying={{
              dataset: props.dataset,
              file: f.file,
              yaml: f.yaml,
              expected: f.expected,
              title: t("quality:rulesIo.title", {
                file: f.file,
                n: total() - queue().length + 1,
                total: total(),
              }),
            }}
            onClose={next}
            onDone={(text, ok) => {
              if (ok) setDeployed((n) => n + 1);
              else notify(`${f.file}: ${text}`, true);
              next();
            }}
          />
        )}
      </Show>
    </div>
  );
};
