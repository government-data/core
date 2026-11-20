import { format } from "sql-formatter";

/** A statement laid out as DuckDB reads it; `undefined` if it cannot be read. */
export function formatted(text: string): string | undefined {
  try {
    return format(text, { language: "duckdb", keywordCase: "upper", tabWidth: 2 });
  } catch {
    return undefined;
  }
}

/** A lake table's records, as the console first asks them. */
export function tableQuery(namespace: string, table: string): string {
  const q = (n: string) => `"${n.replace(/"/g, '""')}"`;
  const plain = `SELECT * FROM lake.${q(namespace)}.${q(table)} LIMIT 100`;
  return formatted(plain) ?? plain;
}

/** A lake namespace's tables, as the console first asks them. */
export function schemaQuery(namespace: string): string {
  const plain = `SELECT table_name FROM duckdb_tables() WHERE database_name = 'lake' AND schema_name = '${namespace.replace(/'/g, "''")}' ORDER BY 1`;
  return formatted(plain) ?? plain;
}
