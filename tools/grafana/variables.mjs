// The dashboards' variables (lib.mjs describes the definition). A dashboard
// names a variable of the library, or gives { name, ...overrides } to change
// `multi`, `all`, `hide` or `query` for itself; a query may refer to the
// variables listed before it, which Grafana then refreshes when they change.
// A variable with `repeat: true` is one panels repeat by (lib.mjs): its All
// has no value of its own, so Grafana 13.2.5 draws a panel for each of its
// values and `${v:pipe}` is every value joined by `|`; one or several
// selected give a panel each, and the pipe of those.

/** A custom All value, inserted as is whatever the format (Grafana 13.2.5). */
export const ALL = "'__all'";

/** The name of a dashboard's variable entry. */
export const variableName = (entry) => (typeof entry === "string" ? entry : entry.name);

/** The library's spec with the dashboard's overrides on top. */
export function variableSpec(entry, library) {
  const name = variableName(entry);
  const base = library[name];
  if (!base) return undefined;
  return typeof entry === "string" ? base : { ...base, ...entry };
}

/** Fails on an override the generator does not know. */
export function checkVariable(entry, library, where) {
  const spec = variableSpec(entry, library);
  if (!spec) throw new Error(`${where}: unknown variable ${variableName(entry)}`);
  const known = new Set(["name", "multi", "all", "hide", "query"]);
  const extra = typeof entry === "string" ? [] : Object.keys(entry).filter((k) => !known.has(k));
  if (extra.length) throw new Error(`${where}: variable ${spec.name} cannot override ${extra}`);
}

/** One variable of a dashboard, its label in the catalogue's language. */
export function variable(entry, library, t, datasources) {
  const name = variableName(entry);
  const spec = variableSpec(entry, library);
  const label = t(`variables.${name}`);
  if (spec.kind === "textbox") {
    return {
      current: { text: "", value: "" },
      hide: 0,
      label,
      name,
      options: [],
      query: "",
      type: "textbox",
    };
  }
  const all = spec.all !== false;
  // A variable panels repeat by: All is every value, each a panel.
  const allValue = spec.repeat ? {} : { allValue: ALL };
  return {
    ...(all ? { ...allValue, current: { text: ["All"], value: ["$__all"] } } : { current: {} }),
    datasource: datasources[spec.datasource],
    definition: spec.query,
    hide: spec.hide ? 2 : 0,
    includeAll: all,
    label,
    multi: Boolean(spec.multi),
    name,
    options: [],
    query: spec.query,
    refresh: 1,
    regex: "",
    sort: 0,
    type: "query",
  };
}
