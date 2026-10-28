//! A `rails_dependent` gem whose source is compiled is not refused by name:
//! its constants are the compiled classes.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests, with the gem written
//! as a PATH gem of `Gemfile.lock`. A test the compiler does not pass yet
//! is ignored with the group of behavior it waits on (`fork spec: ...`).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::project::BuildTarget;

const LOCK: &str = "PATH\n  remote: vendor/gems/token_client-59.3.0\n  specs:\n    token_client (59.3.0)\n\nGEM\n  remote: https://rubygems.org/\n  specs:\n\nDEPENDENCIES\n  token_client!\n";

const GEMS: &str = "rails_dependent:\n  - {gem: token_client, why: \"requiring it loads 199 Rails framework files\"}\n";

const RBI: &str = "# typed: true\nmodule TokenClient\n  class SessionStore\n    def initialize(session, expiry_leeway = nil, grace_period: nil); end\n    def login(scope, attrs); end\n  end\nend\n";

const LIB: &str = "vendor/gems/token_client-59.3.0/lib";

const ENTRY: &str = "module TokenClient\n  def self.client_identifiers = @client_identifiers ||= []\nend\n\nrequire \"token_client/session_store\"\n";

const SESSION_STORE: &str = "module TokenClient\n  class SessionStore\n    attr_reader :logins\n\n    def initialize(session, expiry_leeway = nil, grace_period: nil)\n      @session = session\n      @expiry_leeway = expiry_leeway\n      @grace_period = grace_period\n      @logins = []\n    end\n\n    def login(scope, attrs)\n      @session[scope] = attrs\n      @logins << scope\n      self\n    end\n  end\nend\n";

const HELPER: &str = "module AuthenticationHelper\n  def self.login_without_db_writes(session)\n    store = TokenClient::SessionStore.new(session)\n    store.login(:account, access_token: \"t\")\n    TokenClient.client_identifiers << :app\n    [session[:account], TokenClient.client_identifiers]\n  end\nend\n";

fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write(
            "roundhouse.yml",
            "extensions:\n  - ext\ntest_paths:\n  - test\n",
        )
        .write("ext/gems.yml", GEMS)
        .write("Gemfile", "source \"https://rubygems.org\"\ngem \"token_client\", \"59.3.0\"\n")
        .write("Gemfile.lock", LOCK)
        .write("config/application.rb", "Bundler.require(*Rails.groups)\n")
        .write("sorbet/rbi/gems/token_client@59.3.0.rbi", RBI)
        .write(&format!("{LIB}/token_client.rb"), ENTRY)
        .write(&format!("{LIB}/token_client/session_store.rb"), SESSION_STORE)
        .write("app/models/authentication_helper.rb", HELPER)
}

#[test]
fn a_rails_dependent_gem_compiled_from_source_is_not_refused() {
    let (dir, errors) = app().emit(BuildTarget::Ruby);
    assert!(!errors.iter().any(|e| e.contains("gem_requires_rails")), "{errors:#?}");
    let helper = std::fs::read_to_string(dir.join("app/models/authentication_helper.rb")).expect("helper");
    assert!(helper.contains("TokenClient::SessionStore.new(session)"), "{helper}");
    assert!(!helper.contains("gem_requires_rails"), "{helper}");
}

#[test]
#[ignore = "fork spec: gem closures"]
fn a_rails_dependent_gem_compiled_from_source_runs() {
    let run = app().run_ruby("p AuthenticationHelper.login_without_db_writes({})\n");
    assert!(!run.errors.iter().any(|e| e.contains("gem_requires_rails")), "{:#?}", run.errors);
    assert!(run.success, "{}\n{}", run.stdout, run.stderr);
    assert_eq!(run.stdout, "[{access_token: \"t\"}, [:app]]\n", "{}", run.stderr);
}
