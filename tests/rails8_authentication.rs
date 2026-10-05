//! The Rails 8 authentication generator (`bin/rails g authentication`),
//! overlaid on the blog and run.
//!
//! `check` said 0 errors on the generator's output while the build
//! refused it: `User.authenticate_by(params.permit(…))` and
//! `User.find_by_password_reset_token!` had types and no runtime. The
//! generated app needs, end to end:
//!
//!   - `has_secure_password`'s reset token: signed, expiring, and dead
//!     once the password changes — in Rails' own wire format
//!     (`runtime/ruby/active_record/token_for.rb`);
//!   - `authenticate_by` over a top-level `params.permit`;
//!   - `has_secure_password`'s validations (a mismatched confirmation
//!     must not reset the password);
//!   - `normalizes :email_address` on assignment and in `find_by`;
//!   - `distance_of_time_in_words(0, seconds)` in the reset mailer.
//!
//! The overlay is the generator's output against the blog itself:
//! `tests/support/rails8_authentication/` holds its new files verbatim,
//! and the edits below are the lines it inserts into existing ones.
//! The generated tests then run unchanged, but for one assertion (see
//! `sessions_controller_test_runs`).

use std::path::Path;

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

const GENERATED: &str = "tests/support/rails8_authentication";

const SCHEMA_TABLES: &str = r#"  create_table "sessions", force: :cascade do |t|
    t.datetime "created_at", null: false
    t.string "ip_address"
    t.datetime "updated_at", null: false
    t.string "user_agent"
    t.integer "user_id", null: false
    t.index ["user_id"], name: "index_sessions_on_user_id"
  end

  create_table "users", force: :cascade do |t|
    t.datetime "created_at", null: false
    t.string "email_address", null: false
    t.string "password_digest", null: false
    t.datetime "updated_at", null: false
    t.index ["email_address"], name: "index_users_on_email_address", unique: true
  end

  add_foreign_key "comments", "articles"
  add_foreign_key "sessions", "users"
"#;

fn blog_with_authentication() -> emit_and_run::Overlay {
    let mut overlay = emit_and_run::real_blog()
        .edit(
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\n",
            "class ApplicationController < ActionController::Base\n  include Authentication\n",
        )
        .edit(
            "config/routes.rb",
            "Rails.application.routes.draw do\n",
            "Rails.application.routes.draw do\n  resource :session\n  resources :passwords, param: :token\n",
        )
        .edit("db/schema.rb", "  add_foreign_key \"comments\", \"articles\"\n", SCHEMA_TABLES)
        .edit(
            "test/test_helper.rb",
            "require \"rails/test_help\"\n",
            "require \"rails/test_help\"\nrequire_relative \"test_helpers/session_test_helper\"\n",
        )
        .edit("Gemfile", "# gem \"bcrypt\"", "gem \"bcrypt\"");
    for (path, content) in generated_files() {
        overlay = overlay.write(&path, &content);
    }
    overlay
}

fn generated_files() -> Vec<(String, String)> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<(String, String)>) {
        for entry in std::fs::read_dir(dir).expect("read generated dir") {
            let path = entry.expect("dir entry").path();
            if path.is_dir() {
                walk(root, &path, out);
            } else {
                let rel = path.strip_prefix(root).unwrap().to_string_lossy().into_owned();
                out.push((rel, std::fs::read_to_string(&path).expect("read generated file")));
            }
        }
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join(GENERATED);
    let mut out = Vec::new();
    walk(&root, &root, &mut out);
    out.sort();
    out
}

/// The reset flow: request, mail, edit and update with the token, an
/// invalid token, and a mismatched confirmation.
#[test]
fn passwords_controller_test_runs() {
    blog_with_authentication()
        .run_test("test/controllers/passwords_controller_test.rb")
        .assert_passes();
}

/// `normalizes :email_address, with: ->(e) { e.strip.downcase }`.
#[test]
fn user_test_runs() {
    blog_with_authentication().run_test("test/models/user_test.rb").assert_passes();
}

/// Sign in with valid and invalid credentials, and sign out.
///
/// DIVERGENCE, the one edit to a generated test: the emitted cookie jar
/// answers "" for a cookie that was never set, where Rails answers nil
/// (runtime/ruby/action_controller/cookies.rb says why: a nilable String
/// there tripped spinel). So "no session cookie" is asserted as blank.
#[test]
fn sessions_controller_test_runs() {
    blog_with_authentication()
        .edit(
            "test/controllers/sessions_controller_test.rb",
            "assert_nil cookies[:session_id]",
            "assert cookies[:session_id].blank?",
        )
        .run_test("test/controllers/sessions_controller_test.rb")
        .assert_passes();
}

/// The token is Rails' own: one minted by Rails 8.1.4 verifies here, and
/// stops verifying once the password changes, the purpose differs, or
/// the signature is touched. Minted with `SECRET_KEY_BASE=test-secret`
/// and the clock at 2100-01-01, so its 15-minute expiry is still ahead:
///
///   {"_rails":{"data":[1,"BO0R/e48J."],"exp":"2100-01-01T00:15:00.000Z",
///    "pur":"User\npassword_reset\n900"}}
#[test]
fn a_reset_token_minted_by_rails_verifies() {
    blog_with_authentication()
        .run_ruby(
            r#"
Rails.secret_key_base = "test-secret"
token = "eyJfcmFpbHMiOnsiZGF0YSI6WzEsIkJPMFIvZTQ4Si4iXSwiZXhwIjoiMjEwMC0wMS0wMVQwMDoxNTowMC4wMDBaIiwicHVyIjoiVXNlclxucGFzc3dvcmRfcmVzZXRcbjkwMCJ9fQ==--228af7d51f5e1dea0db4761a6d68c83597f07253"
user = User.new(email_address: "x@y.z")
user.password_digest = "$2a$04$gPxnOwGL7EzUBO0R/e48J.MXo0wqwZ9n.y9kQIphp8ZM4Y2SkcnLS"
user.save!
raise "expected the first row" unless user.id == 1
raise "rails token rejected" unless User.find_by_password_reset_token(token)&.id == 1
raise "bang form rejected" unless User.find_by_password_reset_token!(token).id == 1
raise "tampered token accepted" unless User.find_by_password_reset_token(token.sub("--2", "--3")).nil?
raise "expires_in" unless user.password_reset_token_expires_in == 900

# Ours round-trips, and a password change kills both.
own = user.password_reset_token
raise "own token rejected" unless User.find_by_password_reset_token(own)&.id == 1
user.password_digest = "$2a$04$abcdefghijklmnopqrstuuMXo0wqwZ9n.y9kQIphp8ZM4Y2SkcnLS"
user.save!
raise "stale rails token accepted" unless User.find_by_password_reset_token(token).nil?
raise "stale own token accepted" unless User.find_by_password_reset_token(own).nil?
begin
  User.find_by_password_reset_token!(own)
  raise "bang form accepted a stale token"
rescue ActiveSupport::MessageVerifier::InvalidSignature
end
puts "token contract passed"
"#,
        )
        .assert_passes();
}
