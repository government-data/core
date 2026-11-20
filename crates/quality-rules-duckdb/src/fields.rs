//! Each field's profile in an evaluated cut (decision 0031's
//! `field_quality`): empties as decision 0033 says, distinct values, and
//! the shape the values most often take, each digit a `9` and each letter
//! an `A`, cut at forty characters. A shape and counts, never a value.

use quality::worker_api::FieldQuality;

use crate::{quoted, Error, Runner};

/// The longest shape kept.
const SHAPE: usize = 40;

impl Runner {
    /// The profile of each of `fields` in the loaded `dataset`.
    pub fn field_quality(
        &self,
        dataset: &str,
        fields: &[&str],
    ) -> Result<Vec<FieldQuality>, Error> {
        let from = quoted(dataset)?;
        fields
            .iter()
            .map(|field| {
                let v = format!("CAST({} AS VARCHAR)", quoted(field)?);
                let sql = format!(
                    "WITH e AS (SELECT {v} AS v FROM {from}),
                     f AS (SELECT v FROM e
                            WHERE NOT (v IS NULL OR regexp_full_match(v, '[\\s\\x{{00A0}}]*'))),
                     p AS (SELECT left(regexp_replace(regexp_replace(v, '[0-9]', '9', 'g'),
                                                      '[[:alpha:]]', 'A', 'g'), {SHAPE}) AS p,
                                  count(*) AS n
                             FROM f GROUP BY 1 ORDER BY n DESC, p LIMIT 1)
                     SELECT (SELECT count(*) FROM e) - (SELECT count(*) FROM f),
                            (SELECT count(DISTINCT v) FROM f),
                            (SELECT p FROM p),
                            COALESCE((SELECT n FROM p), 0)"
                );
                self.con.query_row(&sql, [], |r| {
                    Ok(FieldQuality {
                        field: field.to_string(),
                        empty: r.get(0)?,
                        distinct: r.get(1)?,
                        pattern: r.get(2)?,
                        pattern_count: r.get(3)?,
                        icd: None,
                        recency: None,
                    })
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fields_profile_is_its_counts_and_commonest_shape() {
        let runner = Runner::new().unwrap();
        // Synthetic rows: codes of two shapes, blanks (U+00A0 too) and NULLs.
        runner
            .con
            .execute_batch(
                "CREATE VIEW loans AS SELECT
                   CASE WHEN g % 10 = 0 THEN NULL WHEN g % 10 = 1 THEN chr(160)
                        WHEN g % 10 = 2 THEN 'AB-' || g ELSE 'L' || (1000 + g) END AS Code,
                   CAST(NULL AS VARCHAR) AS Note
                 FROM range(0, 100) t(g);",
            )
            .unwrap();
        let profile = runner.field_quality("loans", &["Code", "Note"]).unwrap();
        assert_eq!(
            profile[0],
            FieldQuality {
                field: "Code".into(),
                empty: 20,
                distinct: 80,
                pattern: Some("A9999".into()),
                pattern_count: 70,
                icd: None,
                recency: None,
            }
        );
        assert_eq!(
            (
                profile[1].empty,
                profile[1].distinct,
                profile[1].pattern.as_deref(),
                profile[1].pattern_count
            ),
            (100, 0, None, 0)
        );
        assert!(runner.field_quality("loans", &["Ghost"]).is_err());
    }
}
