# Quality Data

Data quality for organisations: its own ETLs, rule catalogs and scheduled
quality pipelines with their notices, on a platform with tenants and single
sign-on. The quality core is being built.

The documentation lives in [`apps/docs`](apps/docs) (Astro + Starlight), in
English and Spanish: the data quality guide (method, indicators, formulas,
rule catalog, third-party data) and a Charts section that shows how a quality
report reads, drawn with made-up figures (`src/charts/examples.ts`).

```sh
cd apps/docs
pnpm dev     # http://localhost:4321/docs/guide/overview/
```
