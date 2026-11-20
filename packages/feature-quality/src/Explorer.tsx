import "./quality.css";
import {
  createResource,
  createSignal,
  For,
  onCleanup,
  Show,
  type Component,
  type JSX,
} from "solid-js";
import {
  lakeApi,
  may,
  me,
  QualityError,
  rulesApi,
  sourcesApi,
  type DatasetOut,
  type ListingView,
} from "./api";
import { databaseOf, filterSchemas, lakeSchemas, sourceSchemas, type SchemaNode } from "./explorer";
import { Branch } from "./Branch";
import { ColumnHistoryHost } from "./ColumnHistory";
import { ConsoleHost, schemaMenu } from "./Console";
import { deleteMenu, DeleteHost } from "./DeleteDialog";
import { RuleDeleteHost } from "./RuleDelete";
import { openMenu, TreeMenuHost } from "./TreeMenu";
import { ScheduleItem } from "./ScheduleLeaves";
import { allSchedules, listings, mappings, revealed, scheduled } from "./explorerCache";
import { LakeDatasets } from "./DatasetBranch";
import { ExplorerTable } from "./ExplorerTable";
import { ListingStatus } from "./ListingStatus";
import { MapDialog } from "./MapDialog";
import { LakeIcon, ServerIcon, SourcesNavIcon } from "./NavIcon";
import { NewSource, SourceActions } from "./SourceActions";
import { t } from "./i18n";
import { LoadFailed } from "./Notify";
import { onItsWay } from "./sources";

/** How often a listing on its way is asked again. */
const POLL_MS = 2000;

/** A database's schemas, each with its tables; those the filter leaves out hidden. */
const Schemas: Component<{
  schemas: SchemaNode[];
  /** The schemas and `schema.table`s the filter keeps. */
  shown: Set<string>;
  source?: string;
  datasets: DatasetOut[];
  query: string;
  onMap?: (table: string) => void;
}> = (props) => (
  <For each={props.schemas}>
    {(schema) => (
      <Branch
        id={`${props.source ?? ""}/${schema.name}`}
        hidden={!props.shown.has(schema.name)}
        class="qd-tree-schema"
        open={!!props.query.trim()}
        onMenu={(e) =>
          !props.source &&
          openMenu(e, [
            ...schemaMenu(schema.name, schema.tables),
            ...deleteMenu({ namespace: schema.name, tables: schema.tables.length }),
          ])
        }
        label={<span class="qd-mono">{schema.name || "—"}</span>}
      >
        {() => (
          <For each={schema.tables}>
            {(table) => (
              <ExplorerTable
                table={table}
                source={props.source}
                hidden={!props.shown.has(`${schema.name}.${table.name}`)}
                datasets={props.datasets}
                onMap={props.onMap}
              />
            )}
          </For>
        )}
      </Branch>
    )}
  </For>
);

/** A source of the tree: its listing, asked for when opened and again while it is on its way. */
const SourceBranch: Component<{
  /** None for the lake. */
  source?: string;
  label: string;
  icon: JSX.Element;
  /** The database between the server and its schemas; none for the lake. */
  database?: string;
  /** What may be done to it beside listing it again. */
  actions?: JSX.Element;
  load: () => Promise<ListingView>;
  list: () => Promise<unknown>;
  schemas: (listing: ListingView) => SchemaNode[];
  datasets: DatasetOut[];
  query: string;
  manages: boolean;
  onMap?: (table: string) => void;
  /** Opened from elsewhere: a dataset asked to be seen (the lake). */
  open?: boolean;
  /** What it holds besides its listing: the lake's datasets (decision 0049). */
  extra?: () => JSX.Element;
}> = (props) => (
  <Branch
    id={props.source ?? ""}
    class="qd-tree-source"
    open={props.open}
    label={
      <span>
        <span class="qd-tree-icon">{props.icon}</span>
        {props.label}
      </span>
    }
  >
    {() => {
      const [listing, { refetch }] = createResource(listings, () => props.load());
      const timer = setInterval(() => onItsWay(listing()) && void refetch(), POLL_MS);
      onCleanup(() => clearInterval(timer));
      const [refused, setRefused] = createSignal(false);
      const again = async () => {
        setRefused(false);
        try {
          await props.list();
          void refetch();
        } catch {
          setRefused(true);
        }
      };
      const schemas = () => (listing() ? props.schemas(listing()!) : []);
      const shown = () => {
        const kept = filterSchemas(schemas(), props.query);
        return new Set(
          kept.flatMap((s) => [s.name, ...s.tables.map((t) => `${s.name}.${t.name}`)]),
        );
      };
      const tree = () => (
        <Schemas
          schemas={schemas()}
          shown={shown()}
          source={props.source}
          datasets={props.datasets}
          query={props.query}
          onMap={props.onMap}
        />
      );
      return (
        <>
          <li class="qd-tree-note">
            <LoadFailed when={listing.error} />
            <Show when={listing()}>{(l) => <ListingStatus listing={l()} />}</Show>
            <Show when={refused()}>
              <p class="qd-error">{t("quality:sources.tables.refused")}</p>
            </Show>
            <Show when={props.manages}>
              <button
                type="button"
                class="fk-btn fk-btn--sm"
                disabled={onItsWay(listing())}
                onClick={() => void again()}
              >
                {t("quality:explorer.listAgain")}
              </button>
              {props.actions}
            </Show>
          </li>
          <Show when={props.database} fallback={tree()}>
            {(database) => (
              <Branch
                class="qd-tree-schema"
                open
                label={
                  <span>
                    <span class="qd-tree-icon">
                      <SourcesNavIcon size={14} />
                    </span>
                    <span class="qd-mono">{database()}</span>
                  </span>
                }
              >
                {tree}
              </Branch>
            )}
          </Show>
          {props.extra?.()}
        </>
      );
    }}
  </Branch>
);

/** The schedules of no dataset, a group of their own at the end of the tree. */
const Unattached: Component = () => {
  const [schedules] = createResource(scheduled, allSchedules);
  return (
    <For
      each={(schedules() ?? []).filter((s) => !s.dataset)}
      fallback={<li class="qd-hint">{t("quality:schedules.empty")}</li>}
    >
      {(s) => <ScheduleItem schedule={s} />}
    </For>
  );
};

/** What the person may not view, as nothing: the rest of the tree is still there. */
const orNone = <T,>(call: Promise<T[]>): Promise<T[]> =>
  call.catch((e: unknown) =>
    e instanceof QualityError && e.status === 403 ? [] : Promise.reject(e),
  );
const sourcesOrNone = () => orNone(sourcesApi.list());

/**
 * The explorer (decisions 0043 and 0049), the quality panel in the
 * sidebar: every source, browsed as a database client shows its servers,
 * down to each column; and the lake, with each dataset a node holding its
 * fields and the rules that read them; kept as the pages change.
 */
export const Explorer: Component = () => {
  const [sources, { refetch: refetchSources }] = createResource(sourcesOrNone);
  const [asked] = createResource(mappings, () => orNone(rulesApi.datasets()));
  // The last answer while it is asked again (a table mapped): the tree stays.
  const datasets = () => asked.latest;
  const [who] = createResource(me);
  const [query, setQuery] = createSignal("");
  const [mapping, setMapping] = createSignal<{ source: string; table: string }>();
  const manages = () => may(who(), "quality:ManageSources");
  const pipelines = () => may(who(), "quality:ViewPipelines");

  return (
    <div class="qd-explorer">
      <ColumnHistoryHost />
      <ConsoleHost />
      <DeleteHost />
      <RuleDeleteHost />
      <TreeMenuHost />
      <NewSource manages={manages()} onSaved={() => void refetchSources()} />
      <input
        class="qd-explorer__filter"
        type="search"
        autocomplete="off"
        placeholder={t("quality:explorer.filter")}
        aria-label={t("quality:explorer.filter")}
        value={query()}
        onInput={(e) => setQuery(e.currentTarget.value)}
      />
      <LoadFailed when={sources.error || asked.error} />
      <Show when={sources() && datasets()}>
        <ul class="qd-tree" role="tree" aria-label={t("quality:explorer.title")}>
          <For each={sources()}>
            {(source) => (
              <SourceBranch
                source={source.name}
                label={source.name}
                icon={<ServerIcon size={14} />}
                database={databaseOf(source.fields)}
                actions={<SourceActions source={source} onChanged={() => void refetchSources()} />}
                load={() => sourcesApi.tables(source.name)}
                list={() => sourcesApi.listTables(source.name)}
                schemas={(l) => sourceSchemas(source.name, l, datasets()!)}
                datasets={datasets()!}
                query={query()}
                manages={manages()}
                onMap={(table) => setMapping({ source: source.name, table })}
              />
            )}
          </For>
          <SourceBranch
            label={t("quality:explorer.lake")}
            icon={<LakeIcon size={14} />}
            load={lakeApi.tables}
            list={lakeApi.listTables}
            schemas={(l) => lakeSchemas(l, datasets()!)}
            datasets={datasets()!}
            query={query()}
            manages={manages()}
            open={!!revealed()}
            extra={() => (
              <LakeDatasets datasets={datasets()!} query={query()} pipelines={pipelines()} />
            )}
          />
          <Show when={pipelines()}>
            <Branch class="qd-tree-source" label={<span>{t("quality:explorer.unattached")}</span>}>
              {() => <Unattached />}
            </Branch>
          </Show>
        </ul>
      </Show>
      <Show when={mapping()}>
        {(m) => (
          <MapDialog source={m().source} table={m().table} onClose={() => setMapping(undefined)} />
        )}
      </Show>
    </div>
  );
};
