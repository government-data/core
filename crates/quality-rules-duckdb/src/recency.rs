//! How long ago the records were updated (decision 0031's update recency):
//! the records of a cut by the age of its `updated` field at the cut, never
//! today, in fixed bands. Counts, never a value; the empty ones are the
//! field's `empty` in its profile.

use crate::{quoted, Error, Runner};

/// The bands' upper bounds in days, after the band of dates past the cut;
/// a last band takes the older ones.
pub const RECENCY_DAYS: [i64; 5] = [30, 90, 180, 365, 730];

impl Runner {
    /// The records of the loaded `dataset` by the age of `field` at `cut`:
    /// dated after the cut, then up to each of [`RECENCY_DAYS`], then older.
    pub fn recency(&self, dataset: &str, field: &str, cut: &str) -> Result<Vec<i64>, Error> {
        let age = format!(
            "date_diff('day', CAST({} AS DATE), CAST(? AS DATE))",
            quoted(field)?
        );
        let mut bands = vec![format!("count(*) FILTER (WHERE {age} < 0)")];
        let mut from = 0;
        for to in RECENCY_DAYS {
            bands.push(format!(
                "count(*) FILTER (WHERE {age} BETWEEN {from} AND {to})"
            ));
            from = to + 1;
        }
        bands.push(format!("count(*) FILTER (WHERE {age} >= {from})"));
        let sql = format!("SELECT {} FROM {}", bands.join(", "), quoted(dataset)?);
        let params = vec![cut; bands.len()];
        self.con
            .query_row(&sql, duckdb::params_from_iter(params.iter()), |r| {
                (0..bands.len()).map(|i| r.get(i)).collect()
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_records_fall_in_their_band_by_their_age_at_the_cut() {
        let runner = Runner::new().unwrap();
        // Synthetic dates: one per age, and a NULL left to the profile.
        runner
            .con
            .execute_batch(
                "CREATE VIEW loans AS SELECT * FROM (VALUES
                   (DATE '2026-03-02'), (DATE '2026-03-01'), (DATE '2026-01-30'),
                   (DATE '2026-01-29'), (DATE '2025-03-01'), (DATE '2025-02-28'),
                   (DATE '2024-03-01'), (DATE '2020-01-01'), (NULL)) t(Updated);",
            )
            .unwrap();
        assert_eq!(
            runner.recency("loans", "Updated", "2026-03-01").unwrap(),
            vec![1, 2, 1, 0, 1, 2, 1]
        );
        assert!(runner.recency("loans", "Ghost", "2026-03-01").is_err());
    }
}
