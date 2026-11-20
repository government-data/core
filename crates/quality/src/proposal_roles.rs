//! The field roles a map job proposes (decision 0031, point 3): from a
//! column's name and its sample's aggregates (never a value), what it looks
//! like it is for in the charts. "Oficina" looks like a dimension,
//! "FechaUltimaActualizacion" like an update date. A steward confirms them
//! with the mapping. Whether a field's value expires (decision 0057) is
//! proposed the same way, by name. Pure.

use std::collections::BTreeSet;

use crate::proposal::ColumnProfile;

/// The most distinct values a grouping dimension has in a sample.
const DIMENSION_MAX: u64 = 500;
/// The most distinct values a segment has in a sample.
const SEGMENT_MAX: u64 = 12;

/// Parts of a name, lowercase and without accents, that say what it is.
const DIMENSION: &[&str] = &[
    "oficina", "office", "agencia", "sucursal", "branch", "asesor", "advisor", "producto",
    "product", "canal", "channel", "zona", "region", "regional",
];
const SEGMENT: &[&str] = &[
    "tipocliente",
    "clientetype",
    "clienttype",
    "segmento",
    "segment",
    "tipopersona",
];
const ENTITY_TYPE: &[&str] = &[
    "tipodoc",
    "tipoid",
    "tipoidentificacion",
    "documenttype",
    "doctype",
];
const UPDATED: &[&str] = &["actualiza", "update", "modifica", "modified"];
const AMOUNT: &[&str] = &[
    "ingreso", "income", "monto", "amount", "saldo", "balance", "valor",
];
// Short English words ("age", "rate") hide in others ("percentage"): left out.
const DRIFT: &[&str] = &["ingreso", "income", "edad", "tasa", "plazo"];

/// Values that lose validity over time: contact, address, income, work.
const EXPIRES: &[&str] = &[
    "correo",
    "email",
    "telefono",
    "celular",
    "phone",
    "mobile",
    "direccion",
    "address",
    "ingreso",
    "income",
    "salario",
    "salary",
    "ocupacion",
    "occupation",
    "actividad",
    "empresa",
    "employer",
    "cargo",
];

/// Whether a column's value looks like one that expires, by its name.
pub(crate) fn expires(name: &str) -> bool {
    says(&plain(name), EXPIRES)
}

/// A name as the lists above read it: lowercase letters and digits only,
/// accents folded.
fn plain(name: &str) -> String {
    name.chars()
        .filter_map(|c| {
            let c = match c {
                'á' | 'Á' => 'a',
                'é' | 'É' => 'e',
                'í' | 'Í' => 'i',
                'ó' | 'Ó' => 'o',
                'ú' | 'Ú' | 'ü' | 'Ü' => 'u',
                'ñ' | 'Ñ' => 'n',
                c => c,
            };
            c.is_ascii_alphanumeric().then(|| c.to_ascii_lowercase())
        })
        .collect()
}

fn says(name: &str, parts: &[&str]) -> bool {
    parts.iter().any(|p| name.contains(p))
}

/// Proposes roles column by column, in the table's order; a role one
/// field at most may hold goes to the first column that looks like it.
#[derive(Debug, Default)]
pub(crate) struct Roles {
    taken: BTreeSet<&'static str>,
}

impl Roles {
    /// The roles for a column of silver type `kind` and semantic type
    /// `semantic` (as the proposal typed it), by name.
    pub(crate) fn of(
        &mut self,
        c: &ColumnProfile,
        kind: &str,
        semantic: Option<&str>,
        sampled: u64,
    ) -> Vec<&'static str> {
        let name = plain(&c.name);
        let filled = sampled.saturating_sub(c.empty);
        let mut roles = Vec::new();
        let few = |max: u64| c.distinct > 1 && c.distinct <= max && c.distinct < filled;
        match kind {
            "text" if semantic == Some("document") || says(&name, ENTITY_TYPE) => {
                roles.push("entity");
            }
            "text" if says(&name, SEGMENT) && few(SEGMENT_MAX) => {
                roles.extend(self.once("segment"));
            }
            "text" if says(&name, DIMENSION) && few(DIMENSION_MAX) => roles.push("dimension"),
            "date" if says(&name, UPDATED) => roles.extend(self.once("updated")),
            "number" if !c.primary_key => {
                if says(&name, AMOUNT) {
                    roles.extend(self.once("outlier_amount"));
                }
                if says(&name, DRIFT) {
                    roles.push("drift");
                }
            }
            _ => {}
        }
        roles
    }

    /// The role if no column has it yet.
    fn once(&mut self, role: &'static str) -> Option<&'static str> {
        self.taken.insert(role).then_some(role)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proposal::Looks;

    fn column(name: &str, distinct: u64) -> ColumnProfile {
        ColumnProfile {
            name: name.into(),
            declared_type: String::new(),
            nullable: false,
            primary_key: false,
            empty: 0,
            distinct,
            min_length: None,
            max_length: None,
            looks: Looks::default(),
        }
    }

    #[test]
    fn names_and_counts_say_the_role() {
        let mut r = Roles::default();
        let mut of = |name: &str, distinct: u64, kind: &str, semantic: Option<&str>| {
            r.of(&column(name, distinct), kind, semantic, 1000)
        };
        assert_eq!(of("Oficina", 40, "text", Some("code")), ["dimension"]);
        assert_eq!(of("CodAsesor", 900, "text", None), Vec::<&str>::new());
        assert_eq!(of("TipoCliente", 2, "text", Some("code")), ["segment"]);
        assert_eq!(of("tipo_cliente_2", 3, "text", None), Vec::<&str>::new());
        assert_eq!(
            of("NumDocumento", 990, "text", Some("document")),
            ["entity"]
        );
        assert_eq!(of("TipoDoc", 3, "text", Some("code")), ["entity"]);
        assert_eq!(
            of("FechaÚltimaActualización", 300, "date", None),
            ["updated"]
        );
        assert_eq!(of("FechaNacimiento", 300, "date", None), Vec::<&str>::new());
        assert_eq!(
            of("TotalIngresos", 700, "number", Some("amount")),
            ["outlier_amount", "drift"]
        );
        assert_eq!(
            of("SaldoAhorro", 700, "number", Some("amount")),
            Vec::<&str>::new()
        );
        assert_eq!(of("Edad", 80, "number", Some("amount")), ["drift"]);
        assert_eq!(
            of("Oficina", 40, "number", Some("amount")),
            Vec::<&str>::new()
        );
    }

    #[test]
    fn contact_income_and_work_expire_a_document_does_not() {
        for name in [
            "Correo",
            "TeléfonoFijo",
            "DirecciónResidencia",
            "TotalIngresos",
            "Ocupación",
        ] {
            assert!(expires(name), "{name}");
        }
        for name in ["NumDocumento", "FechaNacimiento", "Oficina"] {
            assert!(!expires(name), "{name}");
        }
    }
}
