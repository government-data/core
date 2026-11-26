//! The chart catalog's queries on gold (decision 0090, step 14): each of
//! `quality::charts`' queries over the core's index, written for Trino
//! over gold's current-run views (`gold.quality_gold.v_*`), which people
//! read with their own grants (decision 0089). The dataset and the cuts
//! are literals; numbers are rounded as the index's queries round them.

use quality_rules_sql::{quote_text, Cut};

use crate::{Error, Statement};

const V: &str = "\"gold\".\"quality_gold\"";

/// The charts written here, in the catalog's order. `bands_by_segment`
/// and `time_by_stage` are not: gold's `entity_icd` has no segment and no
/// job writes `job_stats` yet.
pub const CHARTS: &[&str] = &[
    "trend_by_cut",
    "failures_by_dimension",
    "failures_by_field",
    "lowest_icd",
    "icd_distribution",
    "band_transitions",
    "sfc_view",
    "groups_p_chart",
    "largest_groups",
    "groups_changed",
    "outliers_by_group",
    "profiling",
    "psi_by_decile",
    "remediation_queues",
    "update_recency",
    "conformity_by_rule",
];

fn r4(x: &str) -> String {
    format!("round(CAST({x} AS DOUBLE), 4)")
}

/// Chart `id`'s query for `dataset` at `cut` (and `previous`, the cut
/// before it, for the charts that compare); `None` for a chart not here.
pub fn chart(
    id: &str,
    dataset: &str,
    cut: &Cut,
    previous: Option<&Cut>,
) -> Result<Option<Statement>, Error> {
    let d = quote_text(dataset)?;
    let c = cut.literal();
    let p = previous.map_or("CAST(NULL AS DATE)".to_string(), Cut::literal);
    let at = format!("dataset = {d} AND cut = {c}");
    let sql = match results_chart(id, &d, &c, &p, &at).or_else(|| groups_chart(id, &d, &c, &p, &at))
    {
        Some(sql) => sql,
        None => return Ok(None),
    };
    Ok(Some(Statement { sql }))
}

/// The charts over rule results, field quality, entities and cells.
fn results_chart(id: &str, d: &str, c: &str, p: &str, at: &str) -> Option<String> {
    Some(match id {
        "trend_by_cut" => format!(
            "SELECT cut, records, failures, conformity, igcd FROM (\
             SELECT CAST(s.cut AS VARCHAR) AS cut, s.records, {igcd} AS igcd, \
             CAST(COALESCE(sum(r.failures), 0) AS BIGINT) AS failures, \
             CASE WHEN sum(r.applies) > 0 THEN {conf} END AS conformity, s.cut AS at \
             FROM {V}.\"v_run_summary\" s LEFT JOIN {V}.\"v_rule_results\" r \
             ON r.dataset = s.dataset AND r.cut = s.cut \
             WHERE s.dataset = {d} AND s.cut <= {c} GROUP BY s.cut, s.records, s.igcd \
             ORDER BY s.cut DESC LIMIT 12) recent ORDER BY at",
            igcd = r4("s.igcd"),
            conf = r4("1 - CAST(sum(r.failures) AS DOUBLE) / sum(r.applies)"),
        ),
        "failures_by_dimension" => format!(
            "SELECT dimension, severity, count(*) AS rules, CAST(sum(applies) AS BIGINT) AS applies, \
             CAST(sum(failures) AS BIGINT) AS failures FROM {V}.\"v_rule_results\" WHERE {at} \
             GROUP BY dimension, severity \
             ORDER BY failures DESC, dimension ASC NULLS LAST, severity ASC NULLS LAST"
        ),
        "failures_by_field" => format!(
            "SELECT field, count(*) AS rules, CAST(sum(applies) AS BIGINT) AS applies, \
             CAST(sum(failures) AS BIGINT) AS failures FROM {V}.\"v_rule_results\" WHERE {at} \
             GROUP BY field ORDER BY failures DESC, field ASC NULLS LAST"
        ),
        "conformity_by_rule" => format!(
            "SELECT rule, field, dimension, severity, applies, failures, \
             CASE WHEN applies > 0 THEN {} END AS conformity FROM {V}.\"v_rule_results\" \
             WHERE {at} ORDER BY conformity ASC NULLS LAST, rule",
            r4("1 - CAST(failures AS DOUBLE) / applies")
        ),
        "lowest_icd" => format!(
            "SELECT field, {} AS icd FROM {V}.\"v_field_quality\" \
             WHERE {at} AND mean_icd IS NOT NULL ORDER BY mean_icd, field",
            r4("mean_icd")
        ),
        "icd_distribution" => format!(
            "SELECT round(least(floor(icd * 20), 19) / 20, 2) AS from_icd, count(*) AS entities \
             FROM {V}.\"v_entity_icd\" WHERE {at} AND icd IS NOT NULL GROUP BY 1 ORDER BY 1"
        ),
        "band_transitions" => format!(
            "SELECT p.band AS from_band, c.band AS to_band, count(*) AS entities \
             FROM {V}.\"v_entity_icd\" c JOIN {V}.\"v_entity_icd\" p \
             ON p.dataset = c.dataset AND p.entity = c.entity AND p.cut = {p} \
             WHERE c.dataset = {d} AND c.cut = {c} AND c.band IS NOT NULL AND p.band IS NOT NULL \
             GROUP BY 1, 2 ORDER BY array_position(ARRAY['high', 'medium', 'low'], p.band), \
             array_position(ARRAY['high', 'medium', 'low'], c.band)"
        ),
        "sfc_view" => format!(
            "SELECT cell, ok, total, {p_} AS p, passes, missing, margin, \
             CASE WHEN total = 0 THEN NULL WHEN ok >= 0.92 * total THEN 0.2 \
             WHEN ok >= 0.90 * total THEN 0.5 WHEN ok >= 0.80 * total THEN 1.0 \
             WHEN ok >= 0.70 * total THEN 0.6 ELSE 0.3 END AS closeness, \
             {sfc} AS sfc FROM {V}.\"v_cells\" WHERE {at} \
             ORDER BY CAST(ok AS DOUBLE) / NULLIF(total, 0) DESC NULLS LAST, cell",
            p_ = r4("CAST(ok AS DOUBLE) / NULLIF(total, 0)"),
            sfc = r4("avg(CAST(passes AS INTEGER)) OVER ()"),
        ),
        _ => return None,
    })
}

/// The charts over groups, distributions, profiles, queues and recency.
fn groups_chart(id: &str, d: &str, c: &str, p: &str, at: &str) -> Option<String> {
    Some(match id {
        "groups_p_chart" => groups_p_chart(at),
        "largest_groups" => format!(
            "SELECT dimension, grp, records, serious, {share} AS serious_share, {icd} AS icd \
             FROM (SELECT *, row_number() OVER (PARTITION BY dimension ORDER BY records DESC, grp) AS n \
             FROM {V}.\"v_group_metrics\" WHERE {at}) g WHERE n <= 15 \
             ORDER BY dimension, records DESC, grp",
            share = r4("CAST(serious AS DOUBLE) / NULLIF(records, 0)"),
            icd = r4("mean_icd"),
        ),
        "groups_changed" => format!(
            "SELECT dimension, grp, before, now, {ch} AS change FROM (\
             SELECT c.dimension, c.grp, {b} AS before, {n} AS now, \
             row_number() OVER (PARTITION BY c.dimension ORDER BY \
             abs(CAST(c.serious AS DOUBLE) / c.records - CAST(p.serious AS DOUBLE) / p.records) DESC, \
             c.grp) AS n FROM {V}.\"v_group_metrics\" c JOIN {V}.\"v_group_metrics\" p \
             ON p.dataset = c.dataset AND p.dimension = c.dimension AND p.grp = c.grp AND p.cut = {p} \
             WHERE c.dataset = {d} AND c.cut = {c} AND c.records > 0 AND p.records > 0) g \
             WHERE n <= 15 ORDER BY dimension, abs(now - before) DESC, grp",
            ch = r4("now - before"),
            b = r4("CAST(p.serious AS DOUBLE) / p.records"),
            n = r4("CAST(c.serious AS DOUBLE) / c.records"),
        ),
        "outliers_by_group" => format!(
            "SELECT variable, dimension, grp, records, low, high, {} AS share FROM (\
             SELECT *, row_number() OVER (PARTITION BY variable, dimension \
             ORDER BY low + high DESC, records DESC, grp) AS n FROM {V}.\"v_distribution_checks\" \
             WHERE {at} AND dimension <> '') x WHERE n <= 15 \
             ORDER BY dimension, low + high DESC, records DESC, grp",
            r4("CAST(low + high AS DOUBLE) / NULLIF(records, 0)")
        ),
        "profiling" => format!(
            "SELECT f.field, s.records, f.empty, \
             CASE WHEN s.records > 0 THEN {es} END AS empty_share, f.distinct_values, f.pattern, \
             f.floating, CASE WHEN s.records > f.empty THEN {ps} END AS pattern_share \
             FROM {V}.\"v_field_quality\" f JOIN {V}.\"v_run_summary\" s \
             ON s.dataset = f.dataset AND s.cut = f.cut WHERE f.dataset = {d} AND f.cut = {c} \
             ORDER BY empty_share DESC NULLS LAST, f.field",
            es = r4("CAST(f.empty AS DOUBLE) / s.records"),
            ps = r4("CAST(f.pattern_count AS DOUBLE) / (s.records - f.empty)"),
        ),
        "psi_by_decile" => psi(d, c, p),
        "remediation_queues" => queues(d, c, at),
        "update_recency" => format!(
            "SELECT band, records FROM (SELECT 'b' || CAST(u.o AS VARCHAR) AS band, u.n AS records, \
             u.o AS o FROM {V}.\"v_field_quality\" f CROSS JOIN UNNEST(f.recency) WITH ORDINALITY \
             AS u (n, o) WHERE {at} AND f.recency IS NOT NULL UNION ALL \
             SELECT 'empty', f.empty, 99 FROM {V}.\"v_field_quality\" f \
             WHERE {at} AND f.recency IS NOT NULL) r ORDER BY o"
        ),
        _ => return None,
    })
}

fn groups_p_chart(at: &str) -> String {
    format!(
        "WITH g AS (SELECT dimension, grp, records, serious, CAST(serious AS DOUBLE) / records AS p \
         FROM {V}.\"v_group_metrics\" WHERE {at} AND grp <> '' AND records > 0), \
         pb AS (SELECT dimension, CAST(sum(serious) AS DOUBLE) / sum(records) AS p_bar FROM g GROUP BY 1), \
         z AS (SELECT g.dimension, g.grp, g.records, g.serious, g.p, pb.p_bar, \
         sqrt(pb.p_bar * (1 - pb.p_bar) / g.records) AS sigma FROM g JOIN pb ON pb.dimension = g.dimension), \
         zz AS (SELECT *, (p - p_bar) / NULLIF(sigma, 0) AS z FROM z), \
         mr AS (SELECT *, abs(z - lag(z) OVER (PARTITION BY dimension ORDER BY records, grp)) AS mr FROM zz), \
         sz AS (SELECT dimension, COALESCE(avg(mr) / 1.128, 1) AS sigma_z FROM mr GROUP BY 1), \
         l AS (SELECT mr.dimension, mr.grp, mr.records, mr.serious, mr.p, mr.p_bar, \
         greatest(0, mr.p_bar - 3 * mr.sigma * sz.sigma_z) AS lcl, \
         least(1, mr.p_bar + 3 * mr.sigma * sz.sigma_z) AS ucl \
         FROM mr JOIN sz ON sz.dimension = mr.dimension) \
         SELECT dimension, grp, records, serious, {p} AS p, {pb} AS p_bar, {lcl} AS lcl, \
         {ucl} AS ucl, CASE WHEN p > ucl THEN 'above' WHEN p < lcl THEN 'below' ELSE 'in' END \
         AS signal FROM l ORDER BY dimension, records DESC, grp",
        p = r4("p"),
        pb = r4("p_bar"),
        lcl = r4("lcl"),
        ucl = r4("ucl"),
    )
}

fn psi(d: &str, c: &str, p: &str) -> String {
    let at = |cut: &str| {
        format!(
            "FROM {V}.\"v_distribution_checks\" WHERE dataset = {d} AND cut = {cut} \
             AND dimension = '' AND percentiles IS NOT NULL"
        )
    };
    let share = |arr: &str| {
        format!(
            "CASE WHEN k = 0 THEN 0 WHEN k = 10 THEN 1 ELSE \
             greatest(cardinality(filter({arr}, x -> x <= element_at(p.prev, 10 * k + 1))) - 1, 0) \
             / DOUBLE '100' END"
        )
    };
    format!(
        "WITH c AS (SELECT variable, percentiles AS cur {cur}), \
         p AS (SELECT variable, percentiles AS prev {prev}), \
         f AS (SELECT c.variable, k, {fa} AS fa, {fe} AS fe FROM c JOIN p ON p.variable = c.variable \
         CROSS JOIN UNNEST(sequence(0, 10)) AS s (k)), \
         b AS (SELECT variable, k, greatest(fe - lag(fe) OVER w, 0.0001) AS e, \
         greatest(fa - lag(fa) OVER w, 0.0001) AS a FROM f WINDOW w AS (PARTITION BY variable ORDER BY k)) \
         SELECT variable, k AS decile, round(e, 4) AS base, round(a, 4) AS now, \
         round(sum((a - e) * ln(a / e)) OVER (PARTITION BY variable), 4) AS psi \
         FROM b WHERE k > 0 ORDER BY variable, k",
        cur = at(c),
        prev = at(p),
        fa = share("c.cur"),
        fe = share("p.prev"),
    )
}

fn queues(d: &str, c: &str, at: &str) -> String {
    // The dataset's SLAs of its latest `slas` run: an older run's rows
    // linger only until that job's last delete.
    let slas = format!(
        "(SELECT * FROM {V}.\"remediation_slas\" WHERE dataset = {d} AND run_id = \
         (SELECT max_by(run_id, updated_at) FROM {V}.\"remediation_slas\" WHERE dataset = {d}))"
    );
    format!(
        "SELECT dimension, grp, open, overdue, resolved, oldest_days, sla_days, {pr} AS priority \
         FROM (SELECT q.*, row_number() OVER (PARTITION BY dimension ORDER BY open DESC, grp) AS n \
         FROM (SELECT f.dimension, f.grp, CAST(sum(f.open) AS BIGINT) AS open, \
         CAST(coalesce(sum(f.open) FILTER (WHERE \
         date_add('day', coalesce(g.days, s.days, 30), f.since) < {c}), 0) AS BIGINT) AS overdue, \
         CAST(sum(f.resolved) AS BIGINT) AS resolved, \
         max(date_diff('day', f.since, {c})) FILTER (WHERE f.open > 0) AS oldest_days, \
         coalesce(max(g.days), max(s.days), 30) AS sla_days, sum(f.priority) AS priority \
         FROM {V}.\"v_queue_findings\" f \
         LEFT JOIN {slas} g ON g.dimension = f.dimension AND g.grp = f.grp \
         LEFT JOIN {slas} s ON s.dimension = f.dimension AND s.grp IS NULL \
         WHERE f.{at} GROUP BY f.dimension, f.grp) q) r WHERE n <= 15 \
         ORDER BY dimension, open DESC, grp",
        pr = r4("priority"),
        at = at.replace(" AND cut", " AND f.cut"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_chart_reads_gold_views_with_literals_in_trino_spelling() {
        let cut = Cut::parse("2026-10-31").unwrap();
        let before = Cut::parse("2026-09-30").unwrap();
        for id in CHARTS {
            let s = chart(id, "loans", &cut, Some(&before))
                .unwrap()
                .unwrap()
                .sql;
            assert!(s.contains("\"gold\".\"quality_gold\"."), "{id}");
            assert!(s.contains("DATE '2026-10-31'"), "{id}");
            for word in [
                "::",
                "USING (",
                "LATERAL",
                "generate_series",
                ":cut",
                ":dataset",
                "quality.",
            ] {
                assert!(!s.contains(word), "{id}: {word}");
            }
            assert!(!s.contains('\n'), "{id}");
        }
        let none = chart("band_transitions", "loans", &cut, None)
            .unwrap()
            .unwrap();
        assert!(none.sql.contains("p.cut = CAST(NULL AS DATE)"));
        assert!(chart("bands_by_segment", "loans", &cut, None)
            .unwrap()
            .is_none());
        assert!(chart("x", "lo\u{0}ans", &cut, None).is_err());
    }
}
