//! One rule over several fields, or repeated with `for_each`, becomes one
//! unit per field and per value (decision 0033, point 3).

use quality_rules_model::Position;
use quality_rules_model::Rule;

/// One concrete rule: its id, its field, and the values of `for_each`.
#[derive(Debug, Clone)]
pub(super) struct Unit {
    pub(super) id: String,
    pub(super) field: Option<(String, Position)>,
    vars: Vec<(String, String)>,
}

impl Unit {
    /// A text of the rule with `{name}` replaced by this unit's value.
    pub(super) fn subst(&self, text: &str) -> String {
        self.vars
            .iter()
            .fold(text.to_string(), |acc, (name, value)| {
                acc.replace(&format!("{{{name}}}"), value)
            })
    }
}

pub(super) fn expand(rule: &Rule) -> Vec<Unit> {
    let combos = combinations(rule);
    let fields: Vec<Option<(String, Position)>> = match &rule.field {
        Some(f) => f
            .value
            .names()
            .into_iter()
            .map(|n| Some((n.to_string(), f.at)))
            .collect(),
        None => vec![None],
    };
    let several = fields.len() > 1;
    let mut units = Vec::new();
    for vars in &combos {
        for field in &fields {
            let mut id = rule.id.value.clone();
            if several {
                id = format!("{id}.{}", field.as_ref().map_or("", |f| f.0.as_str()));
            }
            for (_, value) in vars {
                id = format!("{id}.{value}");
            }
            let mut unit = Unit {
                id,
                field: None,
                vars: vars.clone(),
            };
            unit.field = field.as_ref().map(|(name, at)| (unit.subst(name), *at));
            units.push(unit);
        }
    }
    units
}

/// Every combination of the `for_each` values, in the order of their names.
fn combinations(rule: &Rule) -> Vec<Vec<(String, String)>> {
    let mut combos = vec![Vec::new()];
    if let Some(each) = &rule.for_each {
        for (name, values) in &each.value {
            combos = combos
                .into_iter()
                .flat_map(|combo| {
                    values.iter().map(move |v| {
                        let mut c = combo.clone();
                        c.push((name.clone(), v.0.clone()));
                        c
                    })
                })
                .collect();
        }
    }
    combos
}
