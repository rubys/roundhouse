//! A Duration literal in an `inclusion: { in: [...] }` list folds to its
//! Integer seconds, with Rails 8.1.4's fixed unit lengths (`N.unit` and
//! `N.unit.seconds` alike; a month is a year / 12). Anything else in the
//! list (a Float receiver, an unknown unit, a call with arguments) leaves
//! the rule the reported gap it was.

use roundhouse::dialect::ValidationRule;
use roundhouse::expr::Literal;

/// `(expression, Rails 8.1.4's to_i)`.
const UNITS: &[(&str, i64)] = &[
    ("1.second", 1),
    ("2.seconds", 2),
    ("1.minute", 60),
    ("5.minutes", 300),
    ("1.hour", 3600),
    ("2.hours", 7200),
    ("1.day", 86400),
    ("3.days", 259200),
    ("1.week", 604800),
    ("2.weeks", 1209600),
    ("1.fortnight", 1209600),
    ("1.month", 2629746),
    ("2.months", 5259492),
    ("1.year", 31556952),
    ("2.years", 63113904),
];

fn values(list: &str) -> (Option<Vec<Literal>>, usize) {
    let model = format!("class Widget < ApplicationRecord\n  ALLOWED = [{list}].freeze\n  validates :n, inclusion: {{ in: ALLOWED }}\nend\n");
    roundhouse::ingest::survey::activate();
    let model = roundhouse::ingest::prism::scope(|| {
        roundhouse::ingest::ingest_model(model.as_bytes(), "app/models/widget.rb", &roundhouse::schema::Schema::default(), &Default::default())
            .unwrap()
            .unwrap()
    })
    .0;
    let gaps = roundhouse::ingest::survey::drain().len();
    let folded = model.body.iter().find_map(|item| match item {
        roundhouse::dialect::ModelBodyItem::Validation { validation, .. } => validation.rules.iter().find_map(|r| match r {
            ValidationRule::Inclusion { values, .. } => Some(values.clone()),
            _ => None,
        }),
        _ => None,
    });
    (folded, gaps)
}

#[test]
fn every_unit_folds_to_rails_seconds() {
    for (expr, want) in UNITS {
        for spelled in [expr.to_string(), format!("{expr}.seconds")] {
            assert_eq!(values(&spelled), (Some(vec![Literal::Int { value: *want }]), 0), "{spelled}");
        }
    }
}

#[test]
fn anything_else_stays_a_gap() {
    for list in ["1.5.hours", "1.week.ago", "1.weeks(2)", "n.weeks", "1.week.to_s"] {
        assert_eq!(values(list), (None, 1), "{list}");
    }
}
