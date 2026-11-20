// What a panel's transformations take for granted (check-queries.mjs):
// followed over the columns its query returned, the columns each names
// must be there by then. Pure.

/** The columns a transformation names, and the columns it leaves. */
function step(t, present) {
  const o = t.options ?? {};
  const matcher = (m) => (m?.matcher?.id === "byName" ? [m.matcher.options] : []);
  switch (t.id) {
    case "convertFieldType":
      return [o.conversions.map((c) => c.targetField), present];
    case "filterByValue":
      return [o.filters.map((f) => f.fieldName), present];
    case "sortBy":
      return [o.sort.map((x) => x.field), present];
    case "limit":
      return [[], present];
    case "calculateField": {
      const named = [
        ...(o.reduce?.include ?? []),
        ...matcher(o.binary?.left),
        ...matcher(o.binary?.right),
      ];
      return [named, new Set([...present, o.alias])];
    }
    case "organize": {
      const named = [...Object.keys(o.renameByName ?? {}), ...Object.keys(o.excludeByName ?? {})];
      const left = [...present].filter((n) => !o.excludeByName?.[n]);
      return [named, new Set(left.map((n) => o.renameByName?.[n] ?? n))];
    }
    case "partitionByValues":
      return [o.fields, new Set([...present].filter((n) => o.keepFields || !o.fields.includes(n)))];
    case "groupingToMatrix":
      // What comes after is named by the rows' values.
      return [[o.rowField, o.columnField, o.valueField], null];
    case "reduce":
      return [[], null];
    default:
      throw new Error(`transformation ${t.id} is not handled by check-queries.mjs`);
  }
}

/** The columns a panel's transformations name that are not there by then. */
export function missingColumns(panel, names) {
  let present = new Set(names);
  const missing = [];
  for (const t of panel.transformations ?? []) {
    const [named, left] = step(t, present);
    missing.push(...named.filter((n) => !present.has(n)));
    if (!left) break;
    present = left;
  }
  return missing;
}
