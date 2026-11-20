//! An evaluation's ICD as it is reported (decision 0057): each field's mean
//! beside its profile, and each entity by the HMAC-SHA-256 of its
//! normalized key with the tenant's entity key, so the key never leaves the
//! worker. Without the key (an older application) no entity is reported.

use hmac::{Hmac, Mac};
use quality::entity_key::EntityKey;
use quality::worker_api::{EntityIcdRow, FieldQuality};
use quality_rules_duckdb::EntityIcd;
use sha2::Sha256;

/// Each field's mean ICD into its profile.
pub(crate) fn merge(fields: &mut [FieldQuality], icd: &[(String, Option<f64>)]) {
    for f in fields {
        f.icd = icd
            .iter()
            .find(|(n, _)| *n == f.field)
            .and_then(|(_, v)| *v);
    }
}

/// The entities by their hashes, hex.
pub(crate) fn hashed(entities: &[EntityIcd], key: Option<&EntityKey>) -> Vec<EntityIcdRow> {
    let Some(key) = key else {
        return Vec::new();
    };
    entities
        .iter()
        .map(|e| EntityIcdRow(hash(&key.0, &e.key), e.icd, e.segment.clone()))
        .collect()
}

fn hash(key: &str, entity: &str) -> String {
    // HMAC takes a key of any length.
    let mut mac = Hmac::<Sha256>::new_from_slice(key.as_bytes()).expect("any key length");
    mac.update(entity.as_bytes());
    mac.finalize()
        .into_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entity(key: &str) -> EntityIcd {
        EntityIcd {
            key: key.into(),
            icd: Some(0.9),
            segment: Some("PN".into()),
        }
    }

    #[test]
    fn an_entity_is_reported_by_its_hash_the_same_for_the_same_key() {
        let key = EntityKey("k1".repeat(32));
        let rows = hashed(&[entity("1234"), entity("1234"), entity("999")], Some(&key));
        assert_eq!(rows[0], rows[1]);
        assert_ne!(rows[0].0, rows[2].0);
        assert_eq!(rows[0].0.len(), 64);
        assert!(!rows[0].0.contains("1234"));
        // RFC 4231's first case, so the hash is HMAC-SHA-256 indeed.
        assert_eq!(
            hash(&"\u{b}".repeat(20), "Hi There"),
            "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
        );
        let other = hashed(&[entity("1234")], Some(&EntityKey("k2".repeat(32))));
        assert_ne!(rows[0].0, other[0].0, "another tenant, another hash");
        assert!(hashed(&[entity("1234")], None).is_empty());
    }
}
