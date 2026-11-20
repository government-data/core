//! A dataset's mapping read from YAML (decision 0020). Synthetic names.

use quality::mapping::{Kind, Mapping, MappingError, Semantic};
use quality_rules::check::FieldType;

const MAPPING: &str = "dataset: clientes
source: fuente_clientes
key: [Documento]
fields:
  Documento: { column: Nit, type: text, semantic: document }
  ActividadLaboral: { column: ActLaboral, type: text }
  FechaNacimiento: { column: FecNacimiento, type: date }
  Correo: { column: Email, type: text, semantic: email }
  Ingresos: { column: TotIngresos, type: number, semantic: amount }
";

#[test]
fn reads_a_mapping_and_gives_the_rules_their_schema() {
    let m = Mapping::parse(MAPPING).unwrap();
    assert_eq!(
        (m.dataset.value.as_str(), m.source.value.as_str()),
        ("clientes", "fuente_clientes")
    );
    assert_eq!(m.fields["ActividadLaboral"].value.column, "ActLaboral");
    assert_eq!(m.fields["Correo"].value.semantic, Some(Semantic::Email));
    assert_eq!(m.fields["Ingresos"].value.kind, Kind::Number);
    let schema = m.schema();
    assert_eq!(schema.fields["Correo"], FieldType::Email);
    assert_eq!(schema.fields["Documento"], FieldType::Text);
    assert_eq!(schema.fields["FechaNacimiento"], FieldType::Date);
}

#[test]
fn refuses_what_does_not_hold_together() {
    let key = MAPPING.replace("key: [Documento]", "key: [Doc]");
    assert_eq!(
        Mapping::parse(&key),
        Err(MappingError::UnknownKey {
            field: "Doc".into()
        })
    );
    let same = MAPPING.replace("column: Email", "column: Nit");
    assert!(
        matches!(Mapping::parse(&same), Err(MappingError::SameColumn { column, .. }) if column == "Nit")
    );
    let kind = MAPPING.replace("type: date", "type: fecha");
    assert!(
        matches!(Mapping::parse(&kind), Err(MappingError::Parse(p)) if p.at.map(|a| a.line) == Some(7))
    );
    let unknown = MAPPING.replace("type: number,", "type: number, cde: true,");
    assert!(matches!(
        Mapping::parse(&unknown),
        Err(MappingError::Parse(_))
    ));
}

#[test]
fn documents_the_dataset_and_each_field() {
    let yaml = MAPPING
        .replace(
            "dataset: clientes\n",
            "dataset: clientes\ndescription: Clientes personas naturales\n",
        )
        .replace(
            "column: ActLaboral, type: text }",
            "column: ActLaboral, type: text, description: Actividad laboral declarada }",
        );
    let m = Mapping::parse(&yaml).unwrap();
    assert_eq!(
        m.description.as_deref(),
        Some("Clientes personas naturales")
    );
    assert_eq!(
        m.fields["ActividadLaboral"].value.description.as_deref(),
        Some("Actividad laboral declarada")
    );
}

#[test]
fn names_the_source_and_table_an_extraction_reads() {
    let yaml = MAPPING.replace(
        "source: fuente_clientes\n",
        "source: fuente_clientes\nfrom: { source: core, table: dbo.clientes }\n",
    );
    let from = Mapping::parse(&yaml).unwrap().from.unwrap().value;
    assert_eq!(
        (from.source.as_str(), from.table.as_str()),
        ("core", "dbo.clientes")
    );
    assert!(Mapping::parse(MAPPING).unwrap().from.is_none(), "optional");
    for bad in [
        "{ source: Core, table: clientes }",
        "{ source: core, table: \"clientes; drop\" }",
        "{ source: core, table: a.b.c.d }",
        "{ source: core, table: dbo. }",
    ] {
        let yaml = MAPPING.replace(
            "source: fuente_clientes\n",
            &format!("source: fuente_clientes\nfrom: {bad}\n"),
        );
        assert!(
            matches!(Mapping::parse(&yaml), Err(MappingError::ReadFrom { at }) if at.line == 3),
            "{bad}"
        );
    }
}

/// The schema editors use for mapping files: regenerate it with
/// `UPDATE_SCHEMA=1 cargo test -p quality`.
#[test]
fn the_published_mapping_schema_is_current() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/mapping.schema.json");
    let current = serde_json::to_string_pretty(&Mapping::json_schema()).unwrap() + "\n";
    if std::env::var_os("UPDATE_SCHEMA").is_some() {
        std::fs::write(path, &current).unwrap();
    }
    let published = std::fs::read_to_string(path).unwrap_or_default();
    assert!(
        published == current,
        "mapping.schema.json is stale: UPDATE_SCHEMA=1 cargo test -p quality"
    );
}

#[test]
fn a_fields_roles_hold_together() {
    use quality::mapping::FieldRole;
    let roles = MAPPING
        .replace(
            "semantic: document }",
            "semantic: document, roles: [entity] }",
        )
        .replace(
            "ActividadLaboral: { column: ActLaboral, type: text }",
            "ActividadLaboral: { column: ActLaboral, type: text, roles: [dimension, segment] }",
        )
        .replace(
            "semantic: amount }",
            "semantic: amount, roles: [outlier_amount, drift] }",
        );
    let m = Mapping::parse(&roles).unwrap();
    assert_eq!(
        m.roles()
            .into_iter()
            .map(FieldRole::name)
            .collect::<Vec<_>>(),
        ["entity", "dimension", "segment", "outlier_amount", "drift"]
    );
    assert!(Mapping::parse(MAPPING).unwrap().roles().is_empty());

    // One segment; an updated date is a date, a drift variable a number.
    let two = roles.replace(
        "Documento: { column: Nit, type: text, semantic: document, roles: [entity] }",
        "Documento: { column: Nit, type: text, semantic: document, roles: [segment] }",
    );
    assert!(matches!(
        Mapping::parse(&two),
        Err(MappingError::OneField { role: FieldRole::Segment, field, .. }) if field == "Documento"
    ));
    for (field, role) in [
        (
            "Correo: { column: Email, type: text, semantic: email",
            "updated",
        ),
        (
            "FechaNacimiento: { column: FecNacimiento, type: date",
            "drift",
        ),
    ] {
        let wrong = roles.replace(
            &format!("{field} }}"),
            &format!("{field}, roles: [{role}] }}"),
        );
        assert!(
            matches!(Mapping::parse(&wrong), Err(MappingError::RoleType { .. })),
            "{role}"
        );
    }
    let unknown = roles.replace("roles: [entity]", "roles: [office]");
    assert!(matches!(
        Mapping::parse(&unknown),
        Err(MappingError::Parse(_))
    ));
}

#[test]
fn a_field_weighs_by_its_priority_and_may_expire() {
    use quality::mapping::Priority;
    let yaml = MAPPING.replace(
        "semantic: email }",
        "semantic: email, priority: critical, expires: true }",
    );
    let m = Mapping::parse(&yaml).unwrap();
    let correo = &m.fields["Correo"].value;
    assert_eq!(
        (correo.priority, correo.expires),
        (Priority::Critical, true)
    );
    assert_eq!(correo.priority.weight(), 4);
    let documento = &m.fields["Documento"].value;
    assert_eq!(
        (documento.priority, documento.expires),
        (Priority::Medium, false)
    );
    assert_eq!(Priority::Low.weight(), 1);
    let unknown = MAPPING.replace("semantic: email }", "semantic: email, priority: urgent }");
    assert!(matches!(
        Mapping::parse(&unknown),
        Err(MappingError::Parse(_))
    ));
}

#[test]
fn the_active_field_lists_its_active_values() {
    use quality::mapping::FieldRole;
    let active = MAPPING.replace(
        "ActividadLaboral: { column: ActLaboral, type: text }",
        "ActividadLaboral: { column: ActLaboral, type: text, roles: [active], active_values: [A, Activo] }",
    );
    let m = Mapping::parse(&active).unwrap();
    assert!(m.roles().contains(&FieldRole::Active));
    assert_eq!(FieldRole::Active.name(), "active");
    assert_eq!(
        m.fields["ActividadLaboral"].value.active_values,
        ["A", "Activo"]
    );
    assert!(m.fields["Correo"].value.active_values.is_empty());

    let refused = |yaml: &str| matches!(Mapping::parse(yaml), Err(MappingError::ActiveValues { field, .. }) if field == "ActividadLaboral");
    // The role needs its values, none blank; the values need the role.
    assert!(refused(&active.replace(", active_values: [A, Activo]", "")));
    assert!(refused(&active.replace("[A, Activo]", "[A, ' ']")));
    assert!(refused(&active.replace("roles: [active], ", "")));
    // One field, text.
    let two = active.replace(
        "semantic: email }",
        "semantic: email, roles: [active], active_values: [S] }",
    );
    assert!(matches!(
        Mapping::parse(&two),
        Err(MappingError::OneField {
            role: FieldRole::Active,
            ..
        })
    ));
    let date = MAPPING.replace(
        "FechaNacimiento: { column: FecNacimiento, type: date }",
        "FechaNacimiento: { column: FecNacimiento, type: date, roles: [active], active_values: [A] }",
    );
    assert!(matches!(
        Mapping::parse(&date),
        Err(MappingError::RoleType {
            role: FieldRole::Active,
            ..
        })
    ));
}
