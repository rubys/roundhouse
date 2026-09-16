//! `has_json :settings, key: default` (ActiveModel::SchematizedJson) —
//! `lower::has_json` synthesizes one typed accessor triple per schema
//! key over the serialized column, replaces the column writer with the
//! casting one, and rewrites the two-hop `x.settings.key?` call sites
//! that Rails answers through a `method_missing` accessor object.

use roundhouse::dialect::MethodReceiver;
use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::lower::lower_model_to_library_class;

fn app_with(model_body: &str, extra: Vec<(&str, &str)>) -> roundhouse::App {
    let mut files: Vec<(String, String)> = vec![
        (
            "db/schema.rb".into(),
            "ActiveRecord::Schema.define(version: 1) do\n  create_table :accounts do |t|\n    t.string :name\n    t.json :settings\n  end\nend\n".into(),
        ),
        (
            "app/models/account.rb".into(),
            format!("class Account < ApplicationRecord\n{model_body}end\n"),
        ),
    ];
    files.extend(extra.into_iter().map(|(p, c)| (p.to_string(), c.to_string())));
    let tree = files
        .into_iter()
        .map(|(p, c)| (std::path::PathBuf::from(p), c.into_bytes()))
        .collect();
    ingest_app_from_tree(tree).expect("ingest tree")
}

fn campfire_like_app() -> roundhouse::App {
    app_with(
        "  has_json :settings, restrict_room_creation_to_administrators: false, max_invites: 10, greeting: \"Hello!\"\n",
        vec![],
    )
}

fn account_source(app: &roundhouse::App) -> String {
    roundhouse::emit::ruby::emit_lowered_models(app)
        .iter()
        .find(|f| f.path.to_string_lossy().ends_with("models/account.rb"))
        .map(|f| f.content.clone())
        .expect("account.rb emitted")
}

#[test]
fn has_json_keys_synthesize_flat_accessors() {
    let app = campfire_like_app();
    let account =
        app.models.iter().find(|m| m.name.0.as_str() == "Account").expect("Account model");
    let lc = lower_model_to_library_class(account, &app.schema);
    let find = |name: &str| {
        lc.methods
            .iter()
            .find(|m| m.name.as_str() == name && m.receiver == MethodReceiver::Instance)
    };

    // The accessor name carries the COLUMN, so two has_json columns can
    // name the same key and nothing collides with a real attribute.
    assert!(find("settings_restrict_room_creation_to_administrators").is_some());
    assert!(find("settings_restrict_room_creation_to_administrators?").is_some());
    let writer =
        find("settings_restrict_room_creation_to_administrators=").expect("writer synthesized");
    assert!(writer.mutates_self, "writer mutates the column");

    // A string key's `?` is Rails' `present?` — the non-empty test.
    assert!(find("settings_greeting?").is_some());
    // An integer's `present?` is unconditionally true in Ruby, so no
    // predicate is synthesized rather than one that always agrees.
    assert!(find("settings_max_invites").is_some());
    assert!(find("settings_max_invites=").is_some());
    assert!(find("settings_max_invites?").is_none());
}

#[test]
fn accessors_render_over_the_serialized_column() {
    let src = account_source(&campfire_like_app());
    assert!(
        src.contains(
            r#"SchematizedJson.read_boolean(@settings, "restrict_room_creation_to_administrators", false)"#
        ),
        "reader routes through the JSON seam carrying the declared default:\n{src}"
    );
    assert!(
        src.contains(r#"SchematizedJson.read_integer(@settings, "max_invites", 10)"#),
        "integer key reads through the integer seam:\n{src}"
    );
    assert!(
        src.contains(r#"SchematizedJson.read_string(@settings, "greeting", "Hello!") != """#),
        "a string key's predicate is the non-empty test:\n{src}"
    );
    // The writer is the coercion boundary — a form sends `"0"`/`"true"`
    // where the schema says boolean, exactly as for typed_store.
    assert!(
        src.contains(
            r#"SchematizedJson.write_boolean(@settings, "restrict_room_creation_to_administrators", value.to_s != "0" && value.to_s != "" && value.to_s != "false")"#
        ),
        "boolean writer casts rather than storing the form value verbatim:\n{src}"
    );
    // The column accessor pair is NOT replaced: `@settings` stays the
    // serialized text every synthesized path already moves (hydration,
    // `[]`, the adapter's escape).
    assert!(
        src.contains("def settings\n    @settings\n  end"),
        "the column reader stays the storage read:\n{src}"
    );
}

#[test]
fn a_user_defined_method_wins_over_synthesis() {
    let app = app_with(
        "  has_json :settings, beta: false\n\n  def settings_beta\n    true\n  end\n",
        vec![],
    );
    let account =
        app.models.iter().find(|m| m.name.0.as_str() == "Account").expect("Account model");
    let lc = lower_model_to_library_class(account, &app.schema);
    let reader = lc
        .methods
        .iter()
        .find(|m| {
            m.name.as_str() == "settings_beta" && m.receiver == MethodReceiver::Instance
        })
        .expect("reader present");
    let body = format!("{:?}", reader.body);
    assert!(
        !body.contains("SchematizedJson"),
        "the model's own method must win over synthesis: {body}"
    );
}

#[test]
fn an_unexpandable_schema_entry_leaves_the_declaration_unclaimed() {
    // `staff: :boolean` declares the type and leaves the value nil —
    // a value none of the typed readers can return. Half a schema is
    // not a schema: nothing is synthesized, and the DSL keeps warning.
    let app = app_with("  has_json :settings, beta: false, staff: :boolean\n", vec![]);
    let account =
        app.models.iter().find(|m| m.name.0.as_str() == "Account").expect("Account model");
    let lc = lower_model_to_library_class(account, &app.schema);
    // (`settings_changed?` / `settings_was` are ActiveModel::Dirty's,
    // synthesized for every column — the schema keys are what must be
    // absent.)
    assert!(
        !lc.methods.iter().any(|m| m.name.as_str().starts_with("settings_beta")),
        "no half-expansion",
    );
    // The whole declaration is unclaimed, so the column writer stays
    // the schema synthesizer's plain one.
    let src = account_source(&app);
    assert!(
        !src.contains("SchematizedJson"),
        "nothing routes through the JSON seam:\n{src}"
    );
}

#[test]
fn two_hop_call_sites_rewrite_to_the_flat_accessor() {
    let mut app = app_with(
        "  has_json :settings, restrict_room_creation_to_administrators: false\n",
        vec![(
            "app/controllers/rooms_controller.rb",
            r#"class RoomsController < ApplicationController
  def create
    head :forbidden if Current.account.settings.restrict_room_creation_to_administrators?
  end

  def update
    Current.account.settings.restrict_room_creation_to_administrators = true
  end
end
"#,
        )],
    );
    roundhouse::session::analyze_and_lower(&mut app);
    let controller = app
        .controllers
        .iter()
        .find(|c| c.name.0.as_str() == "RoomsController")
        .expect("controller");
    let body = format!("{:?}", controller.actions().collect::<Vec<_>>());
    assert!(
        body.contains("settings_restrict_room_creation_to_administrators?"),
        "the predicate hop collapses to the flat accessor:\n{body}"
    );
    assert!(
        body.contains("settings_restrict_room_creation_to_administrators="),
        "the write hop collapses too:\n{body}"
    );
    // The accessor object Rails returns between the hops is gone.
    assert!(
        !body.contains("\"settings\""),
        "no bare `settings` hop survives:\n{body}"
    );
}

#[test]
fn the_whole_column_reads_as_a_hash_and_assigns_through_the_schema() {
    // Rails' accessor reverse-merges the declared defaults into the
    // stored object, so `record[:settings]` is one typed entry per
    // schema key, in declaration order — the flat readers, gathered.
    let src = account_source(&campfire_like_app());
    let index_read = src
        .split("def [](name)")
        .nth(1)
        .and_then(|rest| rest.split("\n  end\n").next())
        .expect("the synthesized [] reader");
    assert!(
        index_read.contains(
            r#"{ "restrict_room_creation_to_administrators" => SchematizedJson.read_boolean(@settings, "restrict_room_creation_to_administrators", false), "max_invites" => SchematizedJson.read_integer(@settings, "max_invites", 10), "greeting" => SchematizedJson.read_string(@settings, "greeting", "Hello!") }"#
        ),
        "record[:settings] is the decoded object with defaults merged:\n{index_read}"
    );
    // Rails' `settings=` (`update!(settings: { … })`, `new(settings:
    // { … })`, `self[:settings] = { … }`) casts each supplied key
    // through the schema and merges — the seam, with the schema as
    // data. The same writers carry hydration's serialized text, which
    // is why the value stays untyped and the seam tells them apart.
    let schema = r#"{ "restrict_room_creation_to_administrators" => "boolean", "max_invites" => "integer", "greeting" => "string" }"#;
    assert!(
        src.contains(&format!(
            "self.settings = SchematizedJson.assign(@settings, attrs[:settings], {schema}) if attrs.key?(:settings)"
        )),
        "update!(settings: …) assigns through the seam:\n{src}"
    );
    assert!(
        src.contains(&format!("@settings = SchematizedJson.assign(@settings, value, {schema})")),
        "self[:settings] = … assigns through the seam:\n{src}"
    );
    assert!(
        src.contains(&format!(
            "self.settings = SchematizedJson.assign(@settings, attrs[:settings], {schema})\n"
        )),
        "new(settings: …) assigns through the seam:\n{src}"
    );
    // Nothing is left to report: the whole-Hash spelling is modeled.
    let mut app = app_with(
        "  has_json :settings, restrict_room_creation_to_administrators: false\n",
        vec![(
            "app/controllers/accounts_controller.rb",
            r#"class AccountsController < ApplicationController
  def update
    Current.account.update!(settings: { "restrict_room_creation_to_administrators" => "true" })
  end
end
"#,
        )],
    );
    let diags = roundhouse::session::analyze_and_lower(&mut app);
    assert!(
        !diags.iter().any(|d| d.message.contains("has_json") || d.message.contains("whole-Hash")),
        "no has_json diagnostic for a modeled assignment, got: {:?}",
        diags.iter().map(|d| d.message.clone()).collect::<Vec<_>>()
    );
}
