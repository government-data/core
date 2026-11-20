import "./quality.css";
import { type Component } from "solid-js";
import { useParams } from "@solidjs/router";
import { DatasetCharts } from "./DatasetCharts";
import { t } from "./i18n";

/**
 * A dataset's dashboard on a page of its own (decision 0031), reached from
 * its node in the explorer: the charts whose needs it meets, by cut.
 */
export const DashboardPage: Component = () => {
  const params = useParams<{ dataset: string }>();
  return (
    <section class="feature qd-page">
      <header class="qd-head">
        <div>
          <h1>{t("quality:charts.dashboardOf", { dataset: params.dataset })}</h1>
        </div>
      </header>
      <div class="card">
        <DatasetCharts dataset={params.dataset} />
      </div>
    </section>
  );
};
