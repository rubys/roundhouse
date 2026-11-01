//! typed_store accessor synthesis (`lower::typed_store::
//! push_typed_store_methods`) — the shared model lowering synthesizes
//! per-attribute reader/predicate/writer methods routing through the
//! `TypedStore` runtime (YAML seam), for every target. Shape tests at
//! the IR level plus a ruby-emit render check.

use roundhouse::dialect::MethodReceiver;
use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::lower::lower_model_to_library_class;

fn lobsters_like_app() -> roundhouse::App {
    let files: Vec<(&str, &str)> = vec![
        (
            "db/schema.rb",
            "ActiveRecord::Schema.define(version: 1) do\n  create_table :users do |t|\n    t.text :settings\n  end\nend\n",
        ),
        (
            "app/models/user.rb",
            r#"class User < ApplicationRecord
  typed_store :settings do |s|
    s.string :prefers_color_scheme, :default => "system"
    s.string :totp_secret
    s.boolean :email_notifications, :default => false
  end

  def totp_secret
    "custom"
  end
end
"#,
        ),
    ];
    let tree = files
        .into_iter()
        .map(|(p, c)| (std::path::PathBuf::from(p), c.as_bytes().to_vec()))
        .collect();
    ingest_app_from_tree(tree).expect("ingest tree")
}

#[test]
fn typed_store_attrs_synthesize_shared_accessors() {
    let app = lobsters_like_app();
    let user = app.models.iter().find(|m| m.name.0.as_str() == "User").expect("User model");
    let lc = lower_model_to_library_class(user, &app.schema);

    let find = |name: &str| {
        lc.methods
            .iter()
            .find(|m| m.name.as_str() == name && m.receiver == MethodReceiver::Instance)
    };

    // String attr with default: reader + writer, no predicate.
    let reader = find("prefers_color_scheme").expect("reader synthesized");
    let body = format!("{:?}", reader.body);
    assert!(
        body.contains("TypedStore") && body.contains("read"),
        "reader must route through TypedStore.read: {body}",
    );
    assert!(
        body.contains("system"),
        "reader must carry the declared default: {body}",
    );
    assert!(find("prefers_color_scheme=").is_some(), "writer synthesized");
    // The typed_store gem generates a `?` predicate for EVERY attr;
    // non-booleans get the typed nil-check (users/show's
    // keybase_signatures? drove the widening).
    let pred = find("prefers_color_scheme?").expect("predicate synthesized for non-bool attr");
    let pbody = format!("{:?}", pred.body);
    assert!(
        pbody.contains("nil?"),
        "non-bool predicate is the nil-check form:\n{pbody}"
    );

    // Boolean attr: reader + predicate + writer.
    assert!(find("email_notifications").is_some());
    assert!(
        find("email_notifications?").is_some(),
        "boolean attr must get the `?` predicate",
    );
    let writer = find("email_notifications=").expect("writer synthesized");
    let wbody = format!("{:?}", writer.body);
    assert!(
        wbody.contains("write"),
        "writer must route through TypedStore.write: {wbody}",
    );
    assert!(writer.mutates_self, "writer mutates the store column");

    // Custom method in the model body WINS — the synthesized reader
    // must yield (push_user_methods runs after synthesis and drops
    // collisions, so a synthesized duplicate would shadow the user's).
    let totp = find("totp_secret").expect("user method present");
    let tbody = format!("{:?}", totp.body);
    assert!(
        tbody.contains("custom") && !tbody.contains("TypedStore"),
        "user-defined totp_secret must win over synthesis: {tbody}",
    );
    // Its writer (no user def) still synthesizes.
    assert!(find("totp_secret=").is_some(), "writer for user-shadowed attr still synthesized");
}

#[test]
fn typed_store_accessors_render_on_the_ruby_tree() {
    let app = lobsters_like_app();
    let files = roundhouse::emit::ruby::emit_lowered_models(&app);
    let user_src = files
        .iter()
        .find(|f| f.path.to_string_lossy().ends_with("models/user.rb"))
        .map(|f| f.content.clone())
        .expect("user.rb emitted");
    assert!(
        user_src.contains(r#"TypedStore.read(@settings, "prefers_color_scheme", "system")"#),
        "reader body must render the overlay call: {user_src}",
    );
    assert!(
        user_src
            .contains(r#"@settings = TypedStore.write(@settings, "prefers_color_scheme", value)"#),
        "writer body must render the assign-back form: {user_src}",
    );
    // A BOOLEAN attr's writer is the coercion boundary: the settings
    // form assigns `"0"`/`"1"` strings, and the reader beside it is
    // declared `bool`. Storing the String verbatim made that signature
    // a lie — Rails casts here, and so do we.
    assert!(
        user_src.contains(
            r#"TypedStore.write(@settings, "email_notifications", value.to_s != "0" && value.to_s != "" && value.to_s != "false")"#
        ),
        "boolean writer must cast rather than store the form value verbatim: {user_src}",
    );
}

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

/// The store column's own reader answers the gem's Hash of the declared
/// attributes, at their defaults until written, and `[]=` writes back
/// into the column the record saves. a large app's `Shop#plan_name` reads
/// `settings[:plan_name]` and its tests write
/// `shop.settings[:gift_cards_activated] = true`; the column reader
/// answered the serialized String: TypeError (no implicit conversion of
/// Symbol into Integer).
#[test]
fn the_store_column_reads_and_writes_as_a_hash() {
    emit_and_run::empty_app()
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table :shops do |t|\n    t.string :name\n    t.text :settings\n  end\nend\n",
        )
        .write(
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n",
        )
        .write(
            "app/models/shop.rb",
            r#"class Shop < ApplicationRecord
  DEFAULT_TIMEZONE = "Eastern Time (US & Canada)"

  typed_store :settings do |s|
    s.string :timezone, blank: false, default: DEFAULT_TIMEZONE
    s.string :plan_name
    s.boolean :gift_cards_activated, default: false
  end

  def plan_name
    settings[:plan_name] || "basic"
  end

  def zone
    settings[:timezone]
  end
end
"#,
        )
        .run_ruby(
            r#"
shop = Shop.new(name: "snow")
raise "default plan" unless shop.plan_name == "basic"
raise "declared default #{shop.zone.inspect}" unless shop.zone == "Eastern Time (US & Canada)"
raise "bool default" unless shop.settings[:gift_cards_activated] == false
raise "key? declared" unless shop.settings.key?(:plan_name) && shop.settings.key?("timezone")
shop.settings[:gift_cards_activated] = "1"
shop.settings["plan_name"] = "plus"
raise "write through #{shop.plan_name.inspect}" unless shop.plan_name == "plus"
raise "cast bool" unless shop.gift_cards_activated == true
raise "indifferent" unless shop.settings.fetch("gift_cards_activated") == true
shop.save!
again = Shop.find(shop.id)
raise "persisted #{again.settings.to_h.inspect}" unless again.plan_name == "plus" && again.gift_cards_activated
raise "to_h" unless again.settings.to_h["timezone"] == "Eastern Time (US & Canada)"
puts "PASS"
"#,
        )
        .assert_passes();
}

/// A store with no boolean attribute has no cast arm: its cast method is
/// the value itself, not a `case` with only an `else` (a syntax error).
#[test]
fn a_hash_read_store_without_booleans_loads() {
    emit_and_run::empty_app()
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table :shops do |t|\n    t.string :name\n    t.text :settings\n  end\nend\n",
        )
        .write(
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n",
        )
        .write(
            "app/models/shop.rb",
            r#"class Shop < ApplicationRecord
  typed_store :settings do |s|
    s.string :plan_name
  end

  def plan_name
    settings[:plan_name] || "basic"
  end
end
"#,
        )
        .run_ruby(
            r#"
shop = Shop.new(name: "snow")
raise "default plan" unless shop.plan_name == "basic"
shop.settings[:plan_name] = "plus"
raise "write through #{shop.plan_name.inspect}" unless shop.plan_name == "plus"
puts "PASS"
"#,
        )
        .assert_passes();
}
