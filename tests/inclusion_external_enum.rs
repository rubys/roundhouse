use roundhouse::dialect::ValidationRule;
use roundhouse::expr::Literal;
use std::path::PathBuf;

#[path = "support/emit_and_run.rs"]
mod emit_and_run;
#[path = "support/inclusion_external_enum.rs"]
mod contract;

fn ingest(form: &str, extra: Option<(&str, &str)>) -> (roundhouse::App, Vec<String>) {
    ingest_with_extras(form, &extra.into_iter().collect::<Vec<_>>())
}

fn ingest_with_extras(form: &str, extras: &[(&str, &str)]) -> (roundhouse::App, Vec<String>) {
    let mut tree: std::collections::HashMap<PathBuf, Vec<u8>> = [
        ("app/models/application_record.rb", contract::BASE),
        ("app/models/pty_session.rb", contract::PROVIDER),
        ("app/models/session_start.rb", form),
        ("db/schema.rb", contract::SCHEMA),
    ].into_iter().map(|(path, source)| (PathBuf::from(path), source.as_bytes().to_vec())).collect();
    for (path, source) in extras { tree.insert(PathBuf::from(path), source.as_bytes().to_vec()); }
    roundhouse::ingest::survey::activate();
    let (app, diagnostics) = roundhouse::ingest::prism::scope(|| roundhouse::ingest::ingest_app_from_tree(tree));
    let mut messages: Vec<_> = diagnostics.into_iter().map(|d| d.message).collect();
    messages.extend(roundhouse::ingest::survey::drain().into_iter().map(|error| error.to_string()));
    (app.expect("ingest"), messages)
}

#[test]
fn external_enum_keys_and_a_constant_snapshot_become_string_sets() {
    let (app, diagnostics) = ingest(contract::FORM, None);
    assert!(!diagnostics.iter().any(|d| d.contains("validation option not lowered")), "{diagnostics:?}");
    let model = app.models.iter().find(|m| m.name.0.as_str() == "SessionStart").unwrap();
    let sets: Vec<_> = model.validations().flat_map(|v| &v.rules).filter_map(|rule| match rule {
        ValidationRule::Inclusion { values, range: None } => Some(values.clone()),
        _ => None,
    }).collect();
    let strings = |values: &[&str]| values.iter().map(|value| Literal::Str { value: value.to_string() }).collect::<Vec<_>>();
    assert_eq!(sets, vec![strings(&["terminal", "protocol"]), strings(&["bypass", "ask"])]);
}

#[test]
fn external_enum_inclusion_executes_like_rails() {
    contract::overlay(contract::FORM).run_ruby(&format!("def messages(record)\n  record.errors\nend\n{}", contract::ASSERTIONS)).assert_passes();
}

#[test]
fn external_enum_keys_also_preserve_exclusion_membership() {
    let form = contract::FORM.replace("inclusion:", "exclusion:");
    contract::overlay(&form).run_ruby(r#"
allowed = SessionStart.new(driver: "custom", approval_policy: "custom")
raise "nonmembers rejected" unless allowed.valid?
blocked = SessionStart.new(driver: "protocol", approval_policy: "ask")
raise "members accepted" if blocked.valid?
raise "wrong errors" unless blocked.errors == ["Driver is reserved", "Approval policy is reserved"]
"#).assert_passes();
}

#[test]
fn mutated_snapshot_and_shadowed_enum_methods_stay_reported() {
    let mutated = contract::FORM.replace("keys.freeze", "keys").replace("  attribute :driver", "  APPROVAL_POLICIES << \"custom\"\n  attribute :driver");
    let (_, diagnostics) = ingest(&mutated, None);
    assert!(diagnostics.iter().any(|d| d.contains("inclusion: in")), "{diagnostics:?}");
    let (_, diagnostics) = ingest(contract::FORM, Some(("app/models/other.rb", "class PtySession\n  def self.drivers = { other: \"other\" }\nend\n")));
    assert!(diagnostics.iter().any(|d| d.contains("inclusion: in")), "{diagnostics:?}");
    for patch in [
        "SessionStart.const_set(:APPROVAL_POLICIES, [\"custom\"])\n",
        "SessionStart::APPROVAL_POLICIES = [\"custom\"]\n",
        "SNAPSHOT_ALIAS = SessionStart::APPROVAL_POLICIES\nSNAPSHOT_ALIAS << \"custom\"\n",
    ] {
        let form = contract::FORM.replace("keys.freeze", "keys");
        let (_, diagnostics) = ingest(&form, Some(("config/initializers/snapshot_patch.rb", patch)));
        assert!(diagnostics.iter().any(|d| d.contains("inclusion: in") || d.contains("enum-derived inclusion list changed")), "{patch}: {diagnostics:?}");
    }
}

#[test]
fn consumed_enum_overrides_keep_validation_reported() {
    for patch in [
        "def PtySession.drivers = {}\n",
        "PtySession.class_eval do\n  def self.approval_policies = {}\nend\n",
    ] {
        let (_, diagnostics) = ingest(contract::FORM, Some(("config/initializers/enum_patch.rb", patch)));
        assert!(diagnostics.iter().any(|d| d.contains("enum-derived inclusion list changed") || d.contains("inclusion: in")), "{patch}: {diagnostics:?}");
    }
}

#[test]
fn consumed_constant_mutation_invalidates_a_merged_enum_list() {
    let form = contract::FORM.replace(
        "in: PtySession.drivers.keys",
        "in: PtySession.drivers.merge(ExtraChoices::OTHER).keys",
    );
    let other = (
        "app/models/extra_choices.rb",
        "class ExtraChoices\n  OTHER = { \"other\" => \"other\" }\nend\n",
    );
    let (app, diagnostics) = ingest_with_extras(&form, &[other]);
    assert!(!diagnostics.iter().any(|d| d.contains("inclusion: in")), "{diagnostics:?}");
    let model = app.models.iter().find(|m| m.name.0.as_str() == "SessionStart").unwrap();
    assert!(model.validations().flat_map(|v| &v.rules).any(|rule| {
        matches!(rule, ValidationRule::Inclusion { values, .. } if values.contains(&Literal::Str { value: "other".into() }))
    }));
    let (_, diagnostics) = ingest_with_extras(&form, &[other, (
        "app/controllers/snapshot_patch_controller.rb",
        "class SnapshotPatchController\n  ExtraChoices::OTHER[\"custom\"] = \"custom\"\nend\n",
    )]);
    assert!(diagnostics.iter().any(|d| d.contains("enum-derived inclusion list changed")), "{diagnostics:?}");
}

#[test]
fn consumed_alias_mutation_invalidates_an_enum_snapshot() {
    let form = contract::FORM.replace("keys.freeze", "keys");
    for patch in [
        "class SnapshotPatchController\n  SNAPSHOT_ALIAS = SessionStart::APPROVAL_POLICIES\n  SNAPSHOT_ALIAS << \"custom\"\nend\n",
        "class SnapshotPatchController\n  if true\n    SNAPSHOT_ALIAS = SessionStart::APPROVAL_POLICIES\n    SNAPSHOT_ALIAS << \"custom\"\n  end\nend\n",
    ] {
        let (_, diagnostics) = ingest(&form, Some(("app/controllers/snapshot_patch_controller.rb", patch)));
        assert!(diagnostics.iter().any(|d| d.contains("enum-derived inclusion list changed")), "{patch}: {diagnostics:?}");
    }
}

#[test]
fn an_enum_recipient_keeps_its_existing_boundary() {
    let form = "class SessionStart < ApplicationRecord\n  enum :driver, { terminal: 0, protocol: 1 }\n  validates :driver, inclusion: { in: PtySession.drivers.keys }\nend\n";
    let (_, diagnostics) = ingest(form, None);
    assert!(diagnostics.iter().any(|d| d.contains("inclusion: in")), "{diagnostics:?}");
    let form = "class SessionStart < PtySession\n  validates :driver, inclusion: { in: PtySession.drivers.keys }\nend\n";
    let (_, diagnostics) = ingest(form, None);
    assert!(diagnostics.iter().any(|d| d.contains("inclusion: in")), "{diagnostics:?}");
}

#[test]
#[ignore = "requires Spinel; run in its CI lane"]
fn external_enum_inclusion_executes_natively() {
    contract::overlay(contract::FORM).run_spinel_with_rbs(&format!(
        "Db.configure(\":memory:\")\nSchema.statements.each {{ |sql| Db.exec(sql) }}\nActiveRecord.adapter = SqliteAdapter\ndef messages(record)\n  record.errors\nend\n{}", contract::ASSERTIONS
    )).assert_passes();
}
