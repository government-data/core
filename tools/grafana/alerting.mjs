// Builds Grafana's alerting provisioning of decision 0072 (points 5 to 8)
// from definition/alerts.json and crates/texts' catalogues: pure functions,
// no I/O; alerts.mjs reads the files and writes the result.
//
// The definition (alerts.json):
//   datasource     the uid of the Prometheus datasource the rules read (Mimir)
//   values         placeholder → what replaces `{{placeholder}}` in every
//                  rule's summary (Grafana's template: `{{ $labels.vm }}`)
//   groups         [{ name, interval, rules }], one rule group each, all in
//                  one folder (`Quality Data · <machines.alerts.folder>`)
//   rule           { uid, title, qd_alert, severity (all but the heartbeat),
//                  expr (PromQL, an instant query A), below | above (the
//                  threshold C on A), for, noData (default OK), values
//                  (more placeholders, or literal text such as "10 %") }
//   policy         the root route (receiver, group_by, group_wait,
//                  group_interval, repeat_interval) and its one child route,
//                  `heartbeat`, for qd_alert = alerting_heartbeat
//   contactPoints  `mail` (name, addresses), `webhook` (url: the API's
//                  POST /internal/machines/alerts, step 5) and `heartbeat`
//                  (name): the root's contact point mails and posts to the
//                  webhook, the heartbeat's posts only
// A rule's summaries, `summary_es` and `summary_en`, are the texts
// `machines.alerts.<qd_alert>` of crates/texts with its values in place;
// the mail's template is made of `machines.alerts.mail.*` and, for an alert
// Grafana makes itself when it cannot evaluate a rule (DatasourceError,
// DatasourceNoData: no `vm`), `machines.alerts.metrics_unavailable` with the
// rule's name. Grafana interpolates `$VAR` in contact points only (0072,
// V2), so an environment variable anywhere else is refused.

/** The languages of the mail, in its order: Spanish first (point 8). */
export const LANGUAGES = ["es", "en"];
const MAIL_KEYS = ["language", "firing", "resolved", "stateFiring", "stateResolved", "noMachine"];
const DATASOURCE_ALERTS = ["DatasourceError", "DatasourceNoData"];
const TEMPLATE = "qd-mail";

/** The `{{name}}` placeholders of a catalogue text, sorted. */
export function placeholders(text) {
  return [...new Set([...text.matchAll(/\{\{(\w+)\}\}/g)].map((m) => m[1]))].sort();
}

/** `text` with each `{{name}}` replaced by `values[name]`; fails on one missing. */
export function fill(text, values, where) {
  return text.replace(/\{\{(\w+)\}\}/g, (_, name) => {
    if (!(name in values)) throw new Error(`${where}: no value for {{${name}}}`);
    return values[name];
  });
}

/** The flat key → text map of a nested catalogue. */
export function flatten(catalogue, prefix = "", out = {}) {
  for (const [k, v] of Object.entries(catalogue)) {
    const key = prefix ? `${prefix}.${k}` : k;
    if (typeof v === "string") out[key] = v;
    else if (v && typeof v === "object") flatten(v, key, out);
  }
  return out;
}

/**
 * Fails unless every key the alerts use is in each catalogue with the same
 * placeholders in both languages (`pnpm run check:grafana`, point 8).
 */
export function checkTexts(def, texts) {
  const alerts = [...new Set(def.groups.flatMap((g) => g.rules.map((r) => r.qd_alert)))];
  const keys = [
    "machines.alerts.folder",
    "machines.alerts.metrics_unavailable",
    ...MAIL_KEYS.map((k) => `machines.alerts.mail.${k}`),
    ...alerts.map((a) => `machines.alerts.${a}`),
  ];
  for (const lang of LANGUAGES) {
    const missing = keys.filter((k) => typeof texts[lang][k] !== "string");
    if (missing.length)
      throw new Error(`crates/texts ${lang} has no text for ${missing.join(", ")}`);
  }
  for (const key of keys) {
    const [first, ...rest] = LANGUAGES.map((lang) => placeholders(texts[lang][key]).join(","));
    if (rest.some((p) => p !== first))
      throw new Error(`${key}: the languages differ in their placeholders`);
  }
}

function threshold(rule) {
  const [type, value] =
    "below" in rule ? ["lt", rule.below] : "above" in rule ? ["gt", rule.above] : [];
  if (type === undefined || typeof value !== "number")
    throw new Error(`rule ${rule.uid}: give a number as below or above`);
  return { type, params: [value] };
}

function annotations(def, rule, texts) {
  const values = { ...def.values, ...rule.values };
  const out = {};
  for (const lang of LANGUAGES) {
    const text = texts[lang][`machines.alerts.${rule.qd_alert}`];
    out[`summary_${lang}`] = fill(text, values, `rule ${rule.uid} (${lang})`);
    const unused = Object.keys(rule.values ?? {}).filter((v) => !placeholders(text).includes(v));
    if (unused.length) throw new Error(`rule ${rule.uid}: values not in its text: ${unused}`);
  }
  return out;
}

function rule(def, r, texts) {
  const heartbeat = r.qd_alert === "alerting_heartbeat";
  if (heartbeat === Boolean(r.severity))
    throw new Error(`rule ${r.uid}: a severity on every rule but the heartbeat (point 6)`);
  if (!/^[a-z0-9-]{1,40}$/.test(r.uid)) throw new Error(`rule ${r.uid}: a bad uid`);
  const labels = { qd_alert: r.qd_alert, ...(r.severity ? { severity: r.severity } : {}) };
  return {
    uid: r.uid,
    title: r.title,
    condition: "C",
    data: [
      {
        refId: "A",
        relativeTimeRange: { from: 600, to: 0 },
        datasourceUid: def.datasource,
        model: { refId: "A", expr: r.expr, instant: true, range: false },
      },
      {
        refId: "C",
        datasourceUid: "__expr__",
        model: {
          refId: "C",
          type: "threshold",
          expression: "A",
          conditions: [{ evaluator: threshold(r) }],
        },
      },
    ],
    noDataState: r.noData ?? "OK",
    execErrState: "Error",
    for: r.for,
    annotations: annotations(def, r, texts),
    labels,
    isPaused: false,
  };
}

/** provisioning/alerting/rules.yml's content. */
export function rules(def, texts) {
  const folder = `Quality Data · ${LANGUAGES.map((l) => texts[l]["machines.alerts.folder"]).join(" / ")}`;
  const titles = def.groups.flatMap((g) => g.rules.map((r) => r.title));
  const twice = titles.filter((t, i) => titles.indexOf(t) !== i);
  if (twice.length) throw new Error(`rule titles given twice: ${twice}`);
  return {
    apiVersion: 1,
    groups: def.groups.map((g) => ({
      orgId: 1,
      name: g.name,
      folder,
      interval: g.interval,
      rules: g.rules.map((r) => rule(def, r, texts)),
    })),
  };
}

/** provisioning/alerting/templates.yml's content: the bilingual mail. */
export function templates(texts) {
  const t = (lang, key) => texts[lang][`machines.alerts.mail.${key}`];
  const both = (key) => LANGUAGES.map((l) => t(l, key)).join(" / ");
  const datasource = DATASOURCE_ALERTS.map((a) => `(eq .Labels.alertname "${a}")`).join(" ");
  const lines = [];
  for (const lang of LANGUAGES) {
    const unavailable = fill(texts[lang]["machines.alerts.metrics_unavailable"], {
      rule: "{{ .Labels.rulename }}",
    });
    lines.push(
      `{{ define "qd.summary.${lang}" }}{{ if or ${datasource} }}${unavailable}{{ else }}{{ .Annotations.summary_${lang} }}{{ end }}{{ end }}`,
      `{{ define "qd.state.${lang}" }}{{ if eq .Status "firing" }}${t(lang, "stateFiring")}{{ else }}${t(lang, "stateResolved")}{{ end }}{{ end }}`,
    );
  }
  const summaries = LANGUAGES.map((l) => `{{ template "qd.summary.${l}" . }}`).join(" / ");
  lines.push(
    `{{ define "qd.machine" }}{{ if .CommonLabels.vm }}{{ .CommonLabels.vm }}{{ else }}${both("noMachine")}{{ end }}{{ end }}`,
    `{{ define "qd.subject" }}[{{ if eq .Status "firing" }}${both("firing")}{{ else }}${both("resolved")}{{ end }}] {{ template "qd.machine" . }}: {{ with index .Alerts 0 }}${summaries}{{ end }}{{ end }}`,
    `{{ define "qd.body" }}${LANGUAGES.map(
      (l) =>
        `${t(l, "language")}:\n{{ range .Alerts }}- {{ template "qd.summary.${l}" . }} ({{ template "qd.state.${l}" . }})\n{{ end }}`,
    ).join("\n")}{{ end }}`,
  );
  return {
    apiVersion: 1,
    templates: [{ orgId: 1, name: TEMPLATE, template: `${lines.join("\n")}\n` }],
  };
}

/** provisioning/alerting/contact-points.yml's content. */
export function contactPoints(def) {
  const { mail, webhook, heartbeat } = def.contactPoints;
  // Its TLS (the application VM's certificate, the CA) is added at each
  // start by grafana/tls/entrypoint.sh, as the datasources' (0072, point 10).
  const api = (uid) => ({
    uid,
    type: "webhook",
    settings: { url: webhook.url, httpMethod: "POST" },
    disableResolveMessage: false,
  });
  return {
    apiVersion: 1,
    contactPoints: [
      {
        orgId: 1,
        name: mail.name,
        receivers: [
          {
            uid: "qd-mail",
            type: "email",
            settings: {
              addresses: mail.addresses,
              singleEmail: true,
              subject: '{{ template "qd.subject" . }}',
              message: '{{ template "qd.body" . }}',
            },
            disableResolveMessage: false,
          },
          api("qd-api"),
        ],
      },
      { orgId: 1, name: heartbeat.name, receivers: [api("qd-api-heartbeat")] },
    ],
  };
}

/** provisioning/alerting/policies.yml's content: the root and the heartbeat's route. */
export function policies(def) {
  const { heartbeat, ...root } = def.policy;
  return {
    apiVersion: 1,
    policies: [
      {
        orgId: 1,
        ...root,
        routes: [{ ...heartbeat, object_matchers: [["qd_alert", "=", "alerting_heartbeat"]] }],
      },
    ],
  };
}

/** Fails if an environment variable (`$NAME`, `${NAME}`) appears in `value`. */
export function checkNoEnv(value, where) {
  const found = JSON.stringify(value).match(/\$\{?[A-Z][A-Z0-9_]*/g);
  if (found) throw new Error(`${where}: Grafana interpolates ${found} only in contact points`);
}
