//! Tidy (decision 0041, point 3): the mechanical style warnings go, and the
//! file reads to the same rules, on synthetic files and on every rules file
//! of `examples/rules` (rules, not data).

use std::path::Path;

use quality_rules_compile::{FieldType, Schema};
use quality_rules_lint::{lint, tidy, Code};

fn schema() -> Schema {
    let mut s = Schema::default();
    for name in ["Name", "Kind"] {
        s.fields.insert(name.into(), FieldType::Text);
    }
    s
}

fn codes(yaml: &str) -> Vec<Code> {
    lint(yaml, &schema())
        .findings
        .into_iter()
        .map(|f| f.code)
        .collect()
}

/// What the YAML reads, to compare before and after.
fn reads(yaml: &str) -> serde_json::Value {
    serde_saphyr::from_str(yaml).expect("it reads")
}

const UNTIDY: &str = "dataset: d
rules:
  - id: R-01
    field: \"Name\"   
    check: 'required'
    severity: major
    cause: capture
    alert: \"Campo vacío\"
    origin: { process: KNIME, node: #1651 }
  # The second rule.
  - id: R-02
    field: Kind
    check: expression
    fails: \"CASE WHEN Kind = 'A' THEN Name WHEN Kind = 'B' THEN Kind ELSE Name END = 'X' OR Name BETWEEN 'A' AND 'B' OR Kind = 'C'\"
    severity: minor
    cause: capture
    alert: ':cut stays quoted'
";

#[test]
fn an_untidy_file_is_tidied_and_reads_the_same() {
    let tidied = tidy(UNTIDY);
    let expected = "dataset: d
rules:
  - id: R-01
    field: Name
    check: required
    severity: major
    cause: capture
    alert: Campo vacío
    origin: { process: KNIME, node: \"#1651\" }

  # The second rule.
  - id: R-02
    field: Kind
    check: expression
    fails: >-
      CASE
      WHEN Kind = 'A'
      THEN Name
      WHEN Kind = 'B'
      THEN Kind
      ELSE Name
      END = 'X'
      OR Name BETWEEN 'A' AND 'B'
      OR Kind = 'C'
    severity: minor
    cause: capture
    alert: ':cut stays quoted'
";
    assert_eq!(tidied, expected);
    // `#1651` was lost to a comment (the file did not read): tidy gives it
    // back, the only change in what the file says.
    let meant = UNTIDY.replace("node: #1651", "node: \"#1651\"");
    assert_eq!(reads(&tidied), reads(&meant));
    assert_eq!(tidy(&tidied), tidied, "tidy twice is tidy once");
}

#[test]
fn a_file_that_does_not_read_is_left_as_it_is() {
    let broken = "rules: [\n  x:\t1  \n";
    assert_eq!(tidy(broken), broken);
}

#[test]
fn every_example_tidies_to_the_same_rules_without_tidied_warnings() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/rules");
    let mut files = 0;
    for dataset in std::fs::read_dir(root).unwrap() {
        let dataset = dataset.unwrap().path();
        if !dataset.is_dir() {
            continue;
        }
        for file in std::fs::read_dir(dataset).unwrap() {
            let path = file.unwrap().path();
            let yaml = std::fs::read_to_string(&path).unwrap();
            let tidied = tidy(&yaml);
            assert_eq!(reads(&tidied), reads(&yaml), "{}", path.display());
            assert_eq!(tidy(&tidied), tidied, "{}", path.display());
            let left: Vec<Code> = codes(&tidied)
                .into_iter()
                .filter(|c| {
                    matches!(
                        c,
                        Code::Tab | Code::TrailingSpace | Code::NeedlessQuotes | Code::RuleSpacing
                    )
                })
                .collect();
            assert_eq!(left, [], "{}", path.display());
            files += 1;
        }
    }
    assert!(files >= 17, "{files}");
}

#[test]
fn a_value_continued_at_its_keys_indent_is_folded_as_meant() {
    let continued = "dataset: d\nderived:\n  Total: CASE WHEN Kind = 'A'  \n  THEN 1 +\n  2\n  END\nrules: []\n";
    let tidied = tidy(continued);
    assert_eq!(
        tidied,
        "dataset: d\nderived:\n  Total: >-\n    CASE WHEN Kind = 'A'\n    THEN 1 +\n    2\n    END\nrules: []\n"
    );
    assert_eq!(
        reads(&tidied)["derived"]["Total"],
        "CASE WHEN Kind = 'A' THEN 1 + 2 END"
    );
}

/// A map made one text by `>-` (QR209), its names continued at their
/// indent (QR208), deeper lines and trailing spaces: as written by hand.
const BLOCK_ON_MAP: &str = r#"dataset: clientes_juridicas
derived: >-
  TotalIngresos: CASE WHEN TipoPersonaJuridica = 'FINANCIERA'
  THEN InterDescuentos +
   OtrosIntereses + 
    Valorizaciones + 
    RendimientoInv + 
    Comisiones + 
    UtilVentaInversion + 
    RecupOperacionales +
    Diversos + 
    IngresosServicios 
  WHEN TipoPersonaJuridica = 'NO FINANCIERA' 
  THEN IngVentasNetas + IngresosFinanc 
  END

  TotalEgresos: CASE WHEN TipoPersonaJuridica = 'FINANCIERA' 
  THEN TotalEgresosFuente 
  WHEN TipoPersonaJuridica = 'NO FINANCIERA' 
  THEN TotEgresosNoFin 
  END
  
  Activo: CASE WHEN TipoPersonaJuridica = 'FINANCIERA' 
  THEN ActivosTotales 
  WHEN TipoPersonaJuridica = 'NO FINANCIERA' 
  THEN TotACtivosNoFin 
  END
  
  Pasivo: CASE WHEN TipoPersonaJuridica = 'FINANCIERA' 
  THEN PasivosTotal WHEN TipoPersonaJuridica = 'NO FINANCIERA' 
  THEN TotPasivosNoFin 
  END
  
  Patrimonio: CASE WHEN TipoPersonaJuridica = 'FINANCIERA' 
  THEN Patriminio 
  WHEN TipoPersonaJuridica = 'NO FINANCIERA' 
  THEN TotPatrimoNoFin 
  END
rules: []
"#;

#[test]
fn a_map_made_one_text_is_given_back_its_names_each_folded() {
    let tidied = tidy(BLOCK_ON_MAP);
    let derived = &reads(&tidied)["derived"];
    assert_eq!(
        derived["TotalEgresos"],
        "CASE WHEN TipoPersonaJuridica = 'FINANCIERA' THEN TotalEgresosFuente \
         WHEN TipoPersonaJuridica = 'NO FINANCIERA' THEN TotEgresosNoFin END"
    );
    let ingresos = derived["TotalIngresos"].as_str().unwrap();
    assert!(ingresos.starts_with("CASE WHEN TipoPersonaJuridica = 'FINANCIERA' THEN InterDescuentos + OtrosIntereses + Valorizaciones"));
    assert!(ingresos.ends_with("THEN IngVentasNetas + IngresosFinanc END"));
    assert_eq!(derived.as_object().unwrap().len(), 5);
    assert!(!tidied.lines().any(|l| l.ends_with(' ')), "{tidied}");
    assert!(
        tidied.contains(
            "derived:
  TotalIngresos: >-
    CASE WHEN"
        ),
        "{tidied}"
    );
    assert_eq!(tidy(&tidied), tidied);
}
