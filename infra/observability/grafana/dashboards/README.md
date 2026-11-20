# Grafana dashboards

The dashboards of decision 0026 (point 8), as Grafana JSON, provisioned with
the installation by `../provisioning/dashboards/files.yml`. This directory is
mounted read-only in Grafana at `/var/lib/grafana-dashboards`; each
subdirectory becomes a folder, and Grafana reads the files again every
minute.

**Every subdirectory is generated; do not edit it.** The dashboards are
defined once in `../definition/` (`dashboards.json`, the SQL in `queries/`,
the Spanish and English texts in `locales/`) and written here, one folder per
language, by `pnpm run gen:grafana` (`tools/grafana/dashboards.mjs`, which
rewrites every subdirectory and keeps this file). `pnpm run check:grafana`
fails when the files here are not what the definition gives;
`tools/grafana/check-queries.mjs` runs their queries against a running
Grafana.

A dashboard is changed in the repository, never in Grafana's UI: the
provider does not let the UI save over a file.
