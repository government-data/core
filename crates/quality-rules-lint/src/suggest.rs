//! A suggestion for a name written almost right ("did you mean …?").

/// The known name closest to `name`, if it is close enough to be a typo:
/// at most two edits, and fewer than a third of its length. Case and
/// accents aside, an exact match is the best suggestion.
pub(crate) fn closest<'a>(name: &str, known: impl IntoIterator<Item = &'a str>) -> Option<&'a str> {
    let target = name.to_lowercase();
    known
        .into_iter()
        .map(|k| (distance(&target, &k.to_lowercase()), k))
        .filter(|(d, k)| *d <= 2 && *d * 3 < k.chars().count().max(3))
        .min_by_key(|(d, k)| (*d, k.len()))
        .map(|(_, k)| k)
}

/// Edits (insert, delete, replace) from `a` to `b`.
fn distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut previous = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != *cb);
            let next = (row[j + 1] + 1).min(row[j] + 1).min(previous + cost);
            previous = row[j + 1];
            row[j + 1] = next;
        }
    }
    row[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suggests_a_close_name_only() {
        let known = ["ActividadLaboral", "NombreEmpresa", "Celular"];
        assert_eq!(closest("ActividadLaborl", known), Some("ActividadLaboral"));
        assert_eq!(closest("celular", known), Some("Celular"));
        assert_eq!(closest("Telefono", known), None);
        assert_eq!(distance("kitten", "sitting"), 3);
    }
}
