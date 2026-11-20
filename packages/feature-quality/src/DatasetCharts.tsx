import "@knime-orchestador/charts/charts.css";
import {
  createResource,
  createSignal,
  For,
  onCleanup,
  onMount,
  Show,
  type Component,
} from "solid-js";
import { exists, lang } from "@knime-orchestador/i18n";
import { may } from "./api";
import { chartsApi } from "./dashboardApi";
import { chartTips } from "./chartTips";
import { chartHtml, type Row } from "./dashboard";
import { t } from "./i18n";
import { who } from "./explorerCache";
import { LoadFailed } from "./Notify";
import { QueueSlas, slasVersion } from "./QueueSlas";

const QUEUES = "remediation_queues";

const tr = (key: string) =>
  exists(`quality:charts.${key}`) ? t(`quality:charts.${key}`) : (key.split(".").pop() ?? key);

/** One chart for the cut chosen, drawn once its rows come; the queues again when their SLAs change. */
const ChartPanel: Component<{ dataset: string; chart: string; cut?: string }> = (props) => {
  const [rows] = createResource(
    () => ({
      dataset: props.dataset,
      chart: props.chart,
      cut: props.cut,
      slas: props.chart === QUEUES ? slasVersion() : 0,
    }),
    (k) => chartsApi.rows(k.dataset, k.chart, k.cut),
  );
  const html = () => chartHtml(props.chart, (rows()?.rows ?? []) as Row[], lang(), tr);
  return (
    <div class="qd-chart">
      <LoadFailed when={rows.error} />
      <Show when={rows() && !html()}>
        <p class="qd-hint">
          {tr(props.chart)}: {tr("noData")}
        </p>
      </Show>
      {/* The primitives escape every value they draw. */}
      <div innerHTML={html()} />
    </div>
  );
};

/**
 * A dataset's dashboard (decision 0031): the charts whose needs it meets,
 * for a cut chosen among its evaluated ones, the latest first; below them,
 * the remediation queues' SLAs for whoever may edit the pipeline (0059).
 */
export const DatasetCharts: Component<{ dataset: string }> = (props) => {
  const [dashboard] = createResource(() => props.dataset, chartsApi.list);
  const [cut, setCut] = createSignal<string>();
  const [me] = createResource(who);
  const slas = () =>
    may(me(), "quality:EditPipelines") && !!dashboard()?.charts.some((c) => c.id === QUEUES);
  let root!: HTMLDivElement;
  onMount(() => onCleanup(chartTips(root)));
  return (
    <>
      <div class="qd-dashboard" ref={root}>
        <LoadFailed when={dashboard.error} />
        <Show when={dashboard()}>
          {(d) => (
            <Show when={d().cuts.length} fallback={<p class="qd-hint">{tr("noCuts")}</p>}>
              <label class="qd-field">
                <span>{tr("cutChoice")}</span>
                <select
                  value={cut() ?? d().cuts[0]}
                  onChange={(e) => setCut(e.currentTarget.value)}
                >
                  <For each={d().cuts}>{(c) => <option value={c}>{c}</option>}</For>
                </select>
              </label>
              <For each={d().charts}>
                {(chart) => <ChartPanel dataset={props.dataset} chart={chart.id} cut={cut()} />}
              </For>
            </Show>
          )}
        </Show>
      </div>
      <Show when={slas()}>
        <QueueSlas dataset={props.dataset} />
      </Show>
    </>
  );
};
