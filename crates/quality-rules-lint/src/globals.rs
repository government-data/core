//! A file linted with its table's global rules (decision 0045): merged
//! before it is compiled, what did not merge as findings at their place,
//! and each `extends` with its line, for whoever says what the rule does.

use quality_rules_model::{parse, repeats, with_globals, Position, RulesFile, Unmerged};

use crate::{Code, Finding};

/// A rule that extends a named one, and where it says so.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Extended {
    pub rule: String,
    pub named: String,
    pub at: Position,
}

/// `file` with the global rules `globals` (their YAML), the findings of
/// what did not merge or says nothing, and its `extends`. Global rules
/// that do not read leave the file as it is: its `extends` then have no
/// pattern, and compiling says so.
pub(crate) fn merge(
    file: RulesFile,
    globals: Option<&str>,
) -> (RulesFile, Vec<Finding>, Vec<Extended>) {
    let Some(globals) = globals.and_then(|g| parse(g).ok()) else {
        return (file, Vec::new(), Vec::new());
    };
    let (merged, unmerged) = with_globals(&file, &globals);
    let mut findings = Vec::new();
    for u in unmerged {
        match u {
            Unmerged::Twice { kind, name, at } => findings.push(
                Finding::new(Code::DeclaredTwice, None, Some(at))
                    .with("kind_key", format!("quality.lint.kind.{kind}"))
                    .with("name", name),
            ),
            Unmerged::UnknownNamed { rule, name, at } => findings
                .push(Finding::new(Code::UnknownNamed, Some(&rule), Some(at)).with("name", name)),
            // The global file's own finding, when it is linted.
            Unmerged::RulesInGlobals { .. } => {}
        }
    }
    let mut extended = Vec::new();
    for rule in &file.rules {
        let Some(extends) = &rule.extends else {
            continue;
        };
        let Some(named) = merged.named.get(&extends.value) else {
            continue;
        };
        for (key, at) in repeats(&named.value, rule) {
            findings.push(
                Finding::new(Code::RepeatsNamed, Some(&rule.id.value), Some(at))
                    .with("key", key)
                    .with("name", extends.value.clone()),
            );
        }
        extended.push(Extended {
            rule: rule.id.value.clone(),
            named: extends.value.clone(),
            at: extends.at,
        });
    }
    (merged, findings, extended)
}

/// The global rules' own finding: they have no `rules:` of their own.
pub(crate) fn rules_in_globals(file: &RulesFile) -> Option<Finding> {
    file.rules
        .first()
        .map(|rule| Finding::new(Code::RulesInGlobals, Some(&rule.id.value), Some(rule.id.at)))
}
