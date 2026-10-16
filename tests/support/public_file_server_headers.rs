//! One `config.public_file_server.headers` contract shared by the
//! interpreted (CRuby overlay) and native (spinel) lanes: the headers the
//! static file server puts on every `public/` and `/assets/` response,
//! environment by environment.
//!
//! The overlay keeps the fixture's generated settings and gives
//! production campfire's own shape: a per-path lambda, then a plain Hash
//! that replaces it (`"public, max-age=#{30.days.to_i}"`). Development
//! keeps the generated caching toggle (`if Rails.root.join(
//! "tmp/caching-dev.txt").exist?`), read once without the toggle file and
//! once with it, so the condition evaluates at run time both ways.
//! `staging` has no file of its own and gets Rails' default, no extra
//! header.

/// In order: (environment, whether `tmp/caching-dev.txt` exists by
/// then, the Hash Rails would hand ActionDispatch::Static).
pub const EXPECTED: [(&str, bool, &str); 5] = [
    ("production", false, r#"{ "Cache-Control" => "public, max-age=2592000" }"#),
    ("development", false, "{}"),
    ("development", true, r#"{ "cache-control" => "public, max-age=172800" }"#),
    ("test", true, r#"{ "cache-control" => "public, max-age=3600" }"#),
    ("staging", true, "{}"),
];

pub fn overlay() -> super::emit_and_run::Overlay {
    super::emit_and_run::real_blog()
        .edit(
            "config/environments/production.rb",
            "  config.public_file_server.headers = { \"cache-control\" => \"public, max-age=#{1.year.to_i}\" }\n",
            "  config.public_file_server.headers = {\n    \"cache-control\" => lambda do |path, _|\n      path.start_with?(\"/assets/\") ? \"public, immutable\" : \"public\"\n    end\n  }\n  config.public_file_server.headers = {\n    \"Cache-Control\" => \"public, max-age=#{30.days.to_i}\"\n  }\n",
        )
}

/// The script both lanes run: every environment against the app's lifted
/// `public_file_server_headers`.
pub fn script() -> String {
    let mut out = String::from("failures = []\nFile.delete(\"tmp/caching-dev.txt\") if File.exist?(\"tmp/caching-dev.txt\")\n");
    let mut toggled = false;
    for (env, toggle, expected) in EXPECTED {
        if toggle && !toggled {
            out.push_str("Dir.mkdir(\"tmp\") unless Dir.exist?(\"tmp\")\nFile.write(\"tmp/caching-dev.txt\", \"\")\n");
            toggled = true;
        }
        out.push_str(&format!(
            "Rails.env_name = {env:?}\n\
             got = Rails.application.public_file_server_headers\n\
             failures << \"{env} (toggle {toggle}): #{{got.inspect}}\" unless got == {expected}\n"
        ));
    }
    out.push_str("raise failures.join(\"\\n\") unless failures.empty?\nputs \"public file server headers contract passed\"\n");
    out
}
