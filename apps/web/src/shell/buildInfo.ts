/**
 * The version at the foot of the menu and the diagnostics it copies: the
 * first question when debugging is what runs (UI and API, with their commit)
 * and where.
 */

export interface Build {
  version: string;
  commit?: string | null;
}

/** The short commit, as git shows it. */
export function shortCommit(commit?: string | null): string {
  return commit ? commit.slice(0, 7) : "";
}

/** `1.0.3` or `1.0.3 (abc1234)`. */
function describe(b: Build): string {
  const c = shortCommit(b.commit);
  return c ? `${b.version} (${c})` : b.version;
}

/**
 * What shows at the foot: the version if UI and API match; otherwise both (an
 * old tab after an update, or an image left unchanged).
 */
export function versionLabel(web: Build, api?: Build): { text: string; mismatch: boolean } {
  if (!api || api.version === web.version) return { text: `v${web.version}`, mismatch: false };
  // In development the UI has no version: that is not a mismatch, and what
  // tells which code runs is the commit.
  if (web.version === "dev") {
    const c = shortCommit(web.commit || api.commit);
    return {
      text: c ? `dev ${c} · API ${api.version}` : `dev · API ${api.version}`,
      mismatch: false,
    };
  }
  return { text: `web ${web.version} · API ${api.version}`, mismatch: true };
}

/** The block to paste into a bug report. */
export function diagnostics(
  web: Build,
  api: Build | undefined,
  env: { host: string; userAgent: string; now: Date; role?: string },
): string {
  return [
    "Quality Data",
    `web: ${describe(web)}`,
    `api: ${api ? describe(api) : "no answer"}`,
    `host: ${env.host}`,
    ...(env.role ? [`role: ${env.role}`] : []),
    `browser: ${env.userAgent}`,
    `time: ${env.now.toISOString()}`,
  ].join("\n");
}
