//! has_secure_password synthesis (`lower::secure_password::
//! push_secure_password_methods`) — the shared model lowering
//! synthesizes authenticate + plaintext accessors in the bcrypt gem's
//! own contract shape (`BCrypt::Password.create/new`), for every
//! target.

use roundhouse::dialect::MethodReceiver;
use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::lower::lower_model_to_library_class;

fn app_with(model_src: &str) -> roundhouse::App {
    let files: Vec<(&str, &str)> = vec![
        (
            "db/schema.rb",
            "ActiveRecord::Schema.define(version: 1) do\n  create_table :users do |t|\n    t.string :password_digest\n  end\nend\n",
        ),
        ("app/models/user.rb", model_src),
    ];
    let tree = files
        .into_iter()
        .map(|(p, c)| (std::path::PathBuf::from(p), c.as_bytes().to_vec()))
        .collect();
    ingest_app_from_tree(tree).expect("ingest tree")
}

#[test]
fn secure_password_synthesizes_authenticate_and_accessors() {
    let app = app_with("class User < ApplicationRecord\n  has_secure_password\nend\n");
    let user = app.models.iter().find(|m| m.name.0.as_str() == "User").expect("User model");
    let lc = lower_model_to_library_class(user, &app.schema);
    let find = |name: &str| {
        lc.methods
            .iter()
            .find(|m| m.name.as_str() == name && m.receiver == MethodReceiver::Instance)
    };

    let auth = find("authenticate").expect("authenticate synthesized");
    let abody = format!("{:?}", auth.body);
    assert!(
        abody.contains("BCrypt") && abody.contains("password_digest"),
        "authenticate must compare through BCrypt::Password over the digest: {abody}",
    );

    assert!(find("password").is_some(), "plaintext reader synthesized");
    let writer = find("password=").expect("plaintext writer synthesized");
    let wbody = format!("{:?}", writer.body);
    assert!(
        wbody.contains("create") && wbody.contains("password_digest"),
        "writer must store the bcrypt digest: {wbody}",
    );
    assert!(writer.mutates_self);
    assert!(find("password_confirmation").is_some());
    assert!(find("password_confirmation=").is_some());
}

#[test]
fn custom_authenticate_wins_over_synthesis() {
    let app = app_with(
        "class User < ApplicationRecord\n  has_secure_password\n\n  def authenticate(pw)\n    \"custom\"\n  end\nend\n",
    );
    let user = app.models.iter().find(|m| m.name.0.as_str() == "User").expect("User model");
    let lc = lower_model_to_library_class(user, &app.schema);
    let auths: Vec<_> = lc
        .methods
        .iter()
        .filter(|m| m.name.as_str() == "authenticate" && m.receiver == MethodReceiver::Instance)
        .collect();
    assert_eq!(auths.len(), 1, "exactly one authenticate (no duplicate def)");
    let body = format!("{:?}", auths[0].body);
    assert!(
        body.contains("custom") && !body.contains("BCrypt"),
        "user-defined authenticate must win: {body}",
    );
}

#[test]
fn secure_password_renders_on_the_ruby_tree() {
    let app = app_with("class User < ApplicationRecord\n  has_secure_password\nend\n");
    let files = roundhouse::emit::ruby::emit_lowered_models(&app);
    let user_src = files
        .iter()
        .find(|f| f.path.to_string_lossy().ends_with("models/user.rb"))
        .map(|f| f.content.clone())
        .expect("user.rb emitted");
    assert!(
        user_src.contains("BCrypt::Password.new(@password_digest) == unencrypted_password"),
        "authenticate must render the gem-contract compare: {user_src}",
    );
    assert!(
        user_src.contains("@password_digest = BCrypt::Password.create(unencrypted_password).to_s"),
        "writer must render the digest store: {user_src}",
    );
}
