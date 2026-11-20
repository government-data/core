//! The values that take more than one shape (one or many, values or a
//! list). Serde's `untagged` would read them through a buffer that types
//! every scalar the YAML way: `08` becomes 8 and `NO` becomes false
//! (verified 2026-10-30 with serde-saphyr 2.0.0). These read lists and maps
//! value by value as text, as written, and refuse a lone scalar that YAML
//! reads as a number or a boolean instead of changing it.

use std::collections::BTreeMap;
use std::fmt;

use serde::de::{self, Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};

use crate::model::{Fields, OneOrMany, Text};
use crate::rule::{Records, ValueSet};

const QUOTE: &str =
    "a text: YAML reads this as a number or a boolean; write it in quotes or as a list";

/// Shared by the shapes: lists value by value, maps entry by entry, and a
/// lone scalar only when YAML read it as text.
trait Shape: Sized {
    const EXPECTING: &'static str;
    fn text<E: de::Error>(_: String) -> Result<Self, E> {
        Err(E::custom(format!("expected {}", Self::EXPECTING)))
    }
    fn list<'de, A: SeqAccess<'de>>(_: A) -> Result<Self, A::Error> {
        Err(de::Error::custom(format!("expected {}", Self::EXPECTING)))
    }
    fn map<'de, A: MapAccess<'de>>(_: A) -> Result<Self, A::Error> {
        Err(de::Error::custom(format!("expected {}", Self::EXPECTING)))
    }
}

struct ShapeVisitor<T>(std::marker::PhantomData<T>);

impl<'de, T: Shape> Visitor<'de> for ShapeVisitor<T> {
    type Value = T;
    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(T::EXPECTING)
    }
    fn visit_str<E: de::Error>(self, v: &str) -> Result<T, E> {
        T::text(v.to_string())
    }
    fn visit_string<E: de::Error>(self, v: String) -> Result<T, E> {
        T::text(v)
    }
    fn visit_u64<E: de::Error>(self, _: u64) -> Result<T, E> {
        Err(E::custom(QUOTE))
    }
    fn visit_i64<E: de::Error>(self, _: i64) -> Result<T, E> {
        Err(E::custom(QUOTE))
    }
    fn visit_f64<E: de::Error>(self, _: f64) -> Result<T, E> {
        Err(E::custom(QUOTE))
    }
    fn visit_bool<E: de::Error>(self, _: bool) -> Result<T, E> {
        Err(E::custom(QUOTE))
    }
    fn visit_seq<A: SeqAccess<'de>>(self, seq: A) -> Result<T, A::Error> {
        T::list(seq)
    }
    fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<T, A::Error> {
        T::map(map)
    }
}

fn read<'de, T: Shape, D: Deserializer<'de>>(d: D) -> Result<T, D::Error> {
    d.deserialize_any(ShapeVisitor::<T>(std::marker::PhantomData))
}

fn all<'de, T: Deserialize<'de>, A: SeqAccess<'de>>(mut seq: A) -> Result<Vec<T>, A::Error> {
    let mut items = Vec::new();
    while let Some(item) = seq.next_element()? {
        items.push(item);
    }
    Ok(items)
}

impl Shape for Fields {
    const EXPECTING: &'static str = "a field or a list of fields";
    fn text<E: de::Error>(v: String) -> Result<Self, E> {
        Ok(Self::One(v))
    }
    fn list<'de, A: SeqAccess<'de>>(seq: A) -> Result<Self, A::Error> {
        all(seq).map(Self::Many)
    }
}

impl Shape for OneOrMany {
    const EXPECTING: &'static str = "a value or a list of values";
    fn text<E: de::Error>(v: String) -> Result<Self, E> {
        Ok(Self::One(Text(v)))
    }
    fn list<'de, A: SeqAccess<'de>>(seq: A) -> Result<Self, A::Error> {
        all(seq).map(Self::Many)
    }
}

impl Shape for ValueSet {
    const EXPECTING: &'static str =
        "a list of values, { list: name } or { reference: dataset.column }";
    fn list<'de, A: SeqAccess<'de>>(seq: A) -> Result<Self, A::Error> {
        all(seq).map(Self::Values)
    }
    fn map<'de, A: MapAccess<'de>>(mut map: A) -> Result<Self, A::Error> {
        let (key, value): (String, String) = map
            .next_entry()?
            .ok_or_else(|| de::Error::custom(Self::EXPECTING))?;
        if map.next_key::<String>()?.is_some() {
            return Err(de::Error::custom(Self::EXPECTING));
        }
        match key.as_str() {
            "list" => Ok(Self::List { list: value }),
            "reference" => Ok(Self::Reference { reference: value }),
            other => Err(de::Error::unknown_field(other, &["list", "reference"])),
        }
    }
}

impl Shape for Records {
    const EXPECTING: &'static str = "a record or a list of records";
    fn list<'de, A: SeqAccess<'de>>(seq: A) -> Result<Self, A::Error> {
        all(seq).map(Self::Many)
    }
    fn map<'de, A: MapAccess<'de>>(mut map: A) -> Result<Self, A::Error> {
        let mut record = BTreeMap::new();
        while let Some((k, v)) = map.next_entry::<String, Option<Text>>()? {
            record.insert(k, v);
        }
        Ok(Self::One(record))
    }
}

macro_rules! deserialize_shape {
    ($($t:ty),*) => {$(
        impl<'de> Deserialize<'de> for $t {
            fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                read(d)
            }
        }
    )*};
}

deserialize_shape!(Fields, OneOrMany, ValueSet, Records);
