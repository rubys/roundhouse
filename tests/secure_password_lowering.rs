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

fn user_src(model_src: &str) -> String {
    user_src_beside(model_src, &[])
}

/// `user_src`, with more files in the app.
fn user_src_beside(model_src: &str, others: &[(&str, &str)]) -> String {
    let mut app = app_with(model_src);
    if !others.is_empty() {
        let mut files: Vec<(&str, &str)> = vec![
            ("db/schema.rb", "ActiveRecord::Schema.define(version: 1) do\n  create_table :users do |t|\n    t.string :password_digest\n  end\nend\n"),
            ("app/models/user.rb", model_src),
        ];
        files.extend_from_slice(others);
        let tree = files
            .into_iter()
            .map(|(p, c)| (std::path::PathBuf::from(p), c.as_bytes().to_vec()))
            .collect();
        app = ingest_app_from_tree(tree).expect("ingest tree");
    }
    roundhouse::session::analyze_and_lower(&mut app);
    roundhouse::emit::ruby::emit_lowered_models(&app)
        .into_iter()
        .find(|f| f.path.to_string_lossy().ends_with("models/user.rb"))
        .map(|f| f.content)
        .expect("user.rb emitted")
}

/// The body of the model's own `password=`, as emitted.
fn writer_body(src: &str) -> &str {
    src.split("def password=(")
        .nth(1)
        .and_then(|rest| rest.split_once('\n'))
        .and_then(|(_, body)| body.split("\n  end").next())
        .expect("the model's own writer")
}

#[test]
fn a_password_writer_that_calls_super_reaches_the_macro_writer() {
    // The model's own writer wins, so the macro's is synthesized under
    // a name of its own and each `super` calls that.
    for (call, want) in [
        ("super", "_secure_password_writer(value)"),
        ("super(value.strip)", "_secure_password_writer(value.strip)"),
        ("super(value) if value", "_secure_password_writer(value) if value"),
    ] {
        let src = user_src(&format!(
            "class User < ApplicationRecord\n  has_secure_password\n\n  def password=(value)\n    @supplied = true\n    {call}\n  end\nend\n"
        ));
        let writer = writer_body(&src);
        assert!(writer.contains("@supplied = true"), "{call}: {writer}");
        assert!(writer.contains(want), "{call}: {writer}");
        assert!(!writer.contains("super"), "{call}: {writer}");
        assert!(
            src.contains("def _secure_password_writer(unencrypted_password)")
                && src.contains("@password_digest = BCrypt::Password.create(unencrypted_password).to_s"),
            "{call}: the macro's writer under its own name: {src}",
        );
    }
}

#[test]
fn a_super_the_rewrite_cannot_place_is_left_alone() {
    // Each of these keeps its `super`, unchanged from before, rather
    // than risk skipping a writer or forwarding the wrong value.
    let record_with_helper = "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\n\n  def _secure_password_writer(v)\n    v\n  end\nend\n";
    let record_with_include = "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\n  include PasswordHooks\nend\n";
    let schema_with_column = "ActiveRecord::Schema.define(version: 1) do\n  create_table :users do |t|\n    t.string :password_digest\n    t.string :_secure_password_writer\n  end\nend\n";
    let schema_with_other_column = "ActiveRecord::Schema.define(version: 1) do\n  create_table :users do |t|\n    t.string :password_digest\n  end\n  create_table :tokens do |t|\n    t.string :_secure_password_writer\n  end\nend\n";
    for (body, others) in [
        // In a block, the writer's parameter name can be the block's own
        // (`|value|`, or `|n; value|`, whose block-local ingest drops).
        ("def password=(value)\n    [true].each { |value| super }\n  end", vec![]),
        ("def password=(value)\n    [1].each { |n; value| super(value) }\n  end", vec![]),
        // `create_block` inlines this block later; the pass runs first.
        ("def password=(value)\n    Token.create { |t; value| super(value) }\n  end", vec![]),
        // The helper's name is taken: by the model's own `def` or macro,
        // or by an ancestor.
        ("def password=(value)\n    super\n  end\n\n  def _secure_password_writer(v)\n    v\n  end", vec![]),
        ("attr_reader :_secure_password_writer\n\n  def password=(value)\n    super\n  end", vec![]),
        ("has_one :_secure_password_writer, class_name: \"Token\"\n\n  def password=(value)\n    super\n  end", vec![]),
        // The helper takes one argument; this writer's bare `super`
        // would pass two.
        ("def password=(value, **options)\n    super\n  end", vec![]),
        (
            "def password=(value)\n    super\n  end",
            vec![("app/models/application_record.rb", record_with_helper)],
        ),
        // A column of that name, on this model's table or another's.
        ("def password=(value)\n    super\n  end", vec![("db/schema.rb", schema_with_column)]),
        ("def password=(value)\n    super\n  end", vec![("db/schema.rb", schema_with_other_column)]),
        // A mixed-in module could define `password=`, and `super` would
        // reach it first: from the body, or from an initializer.
        ("include PasswordHooks\n\n  def password=(value)\n    super\n  end", vec![]),
        (
            "def password=(value)\n    super\n  end",
            vec![("config/initializers/password_hooks.rb", "User.include PasswordHooks\n")],
        ),
        (
            "def password=(value)\n    super\n  end",
            vec![("app/models/application_record.rb", record_with_include)],
        ),
    ] {
        let src = user_src_beside(
            &format!("class User < ApplicationRecord\n  has_secure_password\n  {body}\nend\n"),
            &others,
        );
        assert!(writer_body(&src).contains("super"), "{body}:\n{src}");
        assert!(!src.contains("def _secure_password_writer(unencrypted_password)"), "{body}:\n{src}");
    }
}

#[test]
fn a_rewritten_writer_mutates_self() {
    // Its only write is the helper call; the strict targets need
    // `&mut self` for it.
    let mut app = app_with(
        "class User < ApplicationRecord\n  has_secure_password\n\n  def password=(value)\n    super\n  end\nend\n",
    );
    roundhouse::session::analyze_and_lower(&mut app);
    let user = app.models.iter().find(|m| m.name.0.as_str() == "User").expect("User model");
    let lc = lower_model_to_library_class(user, &app.schema);
    let writer = lc
        .methods
        .iter()
        .find(|m| m.name.as_str() == "password=" && m.receiver == MethodReceiver::Instance)
        .expect("the model's own writer");
    assert!(writer.mutates_self, "{:?}", writer.body);
}

#[test]
fn a_writer_without_super_gets_no_helper() {
    // Only a rewritten `super` brings the helper: a writer that calls a
    // method of that name itself (here, one its ancestor defines) keeps
    // calling that method.
    let src = user_src_beside(
        "class User < ApplicationRecord\n  has_secure_password\n\n  def password=(value)\n    _secure_password_writer(value)\n  end\nend\n",
        &[(
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\n\n  def _secure_password_writer(v)\n    v\n  end\nend\n",
        )],
    );
    assert!(!src.contains("def _secure_password_writer(unencrypted_password)"), "{src}");
}

#[test]
fn a_rewritten_writer_adds_no_diagnostic() {
    // The pass runs after the analyzer, and `diagnose` walks model
    // bodies: the call it writes and the helper it adds are typed, so
    // neither reports an unresolved read or a failed dispatch.
    let mut app = app_with(
        "class User < ApplicationRecord\n  has_secure_password\n\n  def password=(value)\n    @supplied = true\n    super\n  end\nend\n",
    );
    roundhouse::session::analyze_and_lower(&mut app);
    let user = app.models.iter().find(|m| m.name.0.as_str() == "User").expect("User model");
    assert!(
        user.methods().any(|m| m.name.as_str() == "_secure_password_writer"),
        "the rewrite ran",
    );
    let unresolved: Vec<String> = roundhouse::analyze::diagnose(&app)
        .into_iter()
        .filter(|d| {
            matches!(
                d.kind,
                roundhouse::analyze::DiagnosticKind::UnresolvedType { .. }
                    | roundhouse::analyze::DiagnosticKind::SendDispatchFailed { .. }
            )
        })
        .map(|d| d.message)
        .collect();
    assert!(unresolved.is_empty(), "{unresolved:#?}");
}
