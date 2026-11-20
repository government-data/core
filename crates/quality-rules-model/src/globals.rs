//! A table's global rules (decision 0045): `reglas_globales.yaml` declares
//! once what the dataset's other files share, and named rules a field's
//! rule `extends`. A field's file and the global rules become one file
//! before the tree is built, so every later stage reads one file as before.

use std::collections::BTreeMap;

use crate::at::{At, Position};
use crate::model::{Placeholders, RulesFile};
use crate::rule::Rule;
use crate::ParseError;

/// The table's global rules' file.
pub const GLOBAL_FILE: &str = "reglas_globales.yaml";

/// What did not merge, with its place in the field's file (or, for rules
/// in the global file, in that one).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unmerged {
    /// A name declared both by the file and by the global rules.
    Twice {
        kind: &'static str,
        name: String,
        at: Position,
    },
    /// An `extends` that names no named rule: the rule is left out.
    UnknownNamed {
        rule: String,
        name: String,
        at: Position,
    },
    /// The global rules have `rules:` of their own: they are not read.
    RulesInGlobals { at: Position },
}

/// `file` with the global rules: their declarations joined to its own,
/// and each rule that `extends` a named one made whole, what it takes from
/// the named rule placed at its `extends` (where lint says what it does).
pub fn with_globals(file: &RulesFile, globals: &RulesFile) -> (RulesFile, Vec<Unmerged>) {
    let mut out = file.clone();
    let mut problems = Vec::new();
    if let Some(rule) = globals.rules.first() {
        let at = rule.extends.as_ref().map_or(rule.id.at, |e| e.at);
        problems.push(Unmerged::RulesInGlobals { at });
    }
    join(
        &mut out.populations,
        &globals.populations,
        "population",
        |v| v.at,
        &mut problems,
    );
    join(
        &mut out.lists,
        &globals.lists,
        "list",
        |v| v.at,
        &mut problems,
    );
    join(
        &mut out.derived,
        &globals.derived,
        "derived",
        |v| v.at,
        &mut problems,
    );
    join(
        &mut out.references,
        &globals.references,
        "reference",
        |v| v.dataset.at,
        &mut problems,
    );
    join(
        &mut out.tables,
        &globals.tables,
        "table",
        |v| v.then.at,
        &mut problems,
    );
    join(
        &mut out.named,
        &globals.named,
        "named",
        |v| v.at,
        &mut problems,
    );
    placeholders(&mut out.placeholders, &globals.placeholders);
    out.rules = file
        .rules
        .iter()
        .filter_map(|rule| {
            let Some(extends) = &rule.extends else {
                return Some(rule.clone());
            };
            match out.named.get(&extends.value) {
                Some(named) => Some(extended(&named.value, rule, extends.at)),
                None => {
                    problems.push(Unmerged::UnknownNamed {
                        rule: rule.id.value.clone(),
                        name: extends.value.clone(),
                        at: extends.at,
                    });
                    None
                }
            }
        })
        .collect();
    (out, problems)
}

/// The global declarations of one kind added to the file's; one the file
/// also declares stays the file's, and is a problem.
fn join<T: Clone>(
    own: &mut BTreeMap<String, T>,
    global: &BTreeMap<String, T>,
    kind: &'static str,
    at: impl Fn(&T) -> Position,
    problems: &mut Vec<Unmerged>,
) {
    for (name, value) in global {
        match own.get(name) {
            Some(mine) => problems.push(Unmerged::Twice {
                kind,
                name: name.clone(),
                at: at(mine),
            }),
            None => {
                own.insert(name.clone(), value.clone());
            }
        }
    }
}

/// The fillers of each type: the global ones, then the file's.
fn placeholders(own: &mut Placeholders, global: &Placeholders) {
    for (mine, theirs) in [
        (&mut own.text, &global.text),
        (&mut own.number, &global.number),
        (&mut own.date, &global.date),
        (&mut own.email, &global.email),
    ] {
        let kept = std::mem::take(mine);
        *mine = theirs.iter().cloned().chain(kept).collect();
    }
}

/// The named rule's keys, then the rule's on top; `id`, `field` and
/// `for_each` the rule's. What it takes from the named rule is placed at
/// `at`, its `extends`.
fn extended(named: &Rule, rule: &Rule, at: Position) -> Rule {
    fn here<T: Clone>(v: &Option<At<T>>, at: Position) -> Option<At<T>> {
        v.as_ref().map(|v| At {
            value: v.value.clone(),
            at,
        })
    }
    macro_rules! overlay {
        ($($key:ident),*) => {
            Rule {
                id: rule.id.clone(),
                field: rule.field.clone(),
                for_each: rule.for_each.clone(),
                extends: rule.extends.clone(),
                origin: rule.origin.clone().or_else(|| named.origin.clone()),
                note: rule.note.clone().or_else(|| named.note.clone()),
                $($key: rule.$key.clone().or_else(|| here(&named.$key, at)),)*
            }
        };
    }
    overlay!(
        check,
        when,
        population,
        severity,
        cause,
        alert,
        dimension,
        on_empty,
        case,
        values,
        in_,
        not_in,
        min,
        max,
        min_exclusive,
        max_exclusive,
        pattern,
        named,
        max_age,
        parts,
        tolerance,
        key,
        normalize,
        ignore,
        table,
        if_,
        then,
        both_ways,
        from,
        fails,
        examples
    )
}

/// The keys `rule` writes with the very value of the named rule it
/// extends, each with its place in `rule`: they say nothing.
pub fn repeats(named: &Rule, rule: &Rule) -> Vec<(&'static str, Position)> {
    let mut out = Vec::new();
    macro_rules! same {
        ($($key:ident => $name:literal),*) => {
            $(if let (Some(mine), Some(theirs)) = (&rule.$key, &named.$key) {
                if mine.value == theirs.value {
                    out.push(($name, mine.at));
                }
            })*
        };
    }
    same!(
        check => "check", when => "when", population => "population", severity => "severity",
        cause => "cause", alert => "alert", dimension => "dimension", on_empty => "on_empty",
        case => "case", values => "values", in_ => "in", not_in => "not_in", min => "min",
        max => "max", min_exclusive => "min_exclusive", max_exclusive => "max_exclusive",
        pattern => "pattern", named => "named", max_age => "max_age", parts => "parts",
        tolerance => "tolerance", key => "key", normalize => "normalize", ignore => "ignore",
        table => "table", if_ => "if", then => "then", both_ways => "both_ways", from => "from",
        fails => "fails", examples => "examples"
    );
    out.sort_by_key(|(_, at)| *at);
    out
}

/// What `parse` refuses beyond the shape: a rule with no id, or with
/// neither a pattern nor a named rule to take one from; a named rule with
/// an id, a field, its own `extends`, or no pattern.
pub(crate) fn check_shape(file: &RulesFile) -> Result<(), ParseError> {
    let refuse = |at: Option<Position>, message: &str| {
        Err(ParseError {
            at,
            message: message.to_string(),
        })
    };
    for rule in &file.rules {
        let at = rule
            .check
            .as_ref()
            .map(|c| c.at)
            .or(rule.extends.as_ref().map(|e| e.at));
        if rule.id.value.is_empty() {
            return refuse(at, "a rule needs its id");
        }
        if rule.check.is_none() && rule.extends.is_none() {
            return refuse(
                Some(rule.id.at),
                "a rule needs check, or extends a named rule",
            );
        }
    }
    for named in file.named.values() {
        let at = Some(named.at);
        let rule = &named.value;
        if !rule.id.value.is_empty() || rule.field.is_some() || rule.for_each.is_some() {
            return refuse(
                at,
                "a named rule has no id or field: each rule that extends it has its own",
            );
        }
        if rule.extends.is_some() {
            return refuse(at, "a named rule does not extend another");
        }
        if rule.check.is_none() {
            return refuse(at, "a named rule needs check");
        }
    }
    Ok(())
}
