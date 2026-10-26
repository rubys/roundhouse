use super::emit_and_run;

fn app() -> emit_and_run::Overlay {
    let gem = "vendor/bundle/ruby/3.4.0/gems/greetings-1.0";
    emit_and_run::real_blog()
        .edit("Gemfile.lock", "    debug (1.11.1)\n", "    debug (1.11.1)\n    greetings (1.0)\n")
        .edit("Gemfile.lock", "DEPENDENCIES\n", "DEPENDENCIES\n  greetings\n")
        .write(".bundle/config", "---\nBUNDLE_PATH: \"vendor/bundle\"\n")
        .write("roundhouse.yml", "extensions:\n  - config/roundhouse\n")
        .write("config/roundhouse/gems.yml", "source_gems:\n  - {gem: \"greetings\"}\n")
        .write(&format!("{gem}/lib/greetings.rb"), "module Greetings\n  autoload :Formatter, \"greetings/formatter\"\nend\n")
        .write(
            &format!("{gem}/lib/greetings/formatter.rb"),
            "module Greetings\n  class Formatter\n    def self.shout(text)\n      text.upcase + \"!\"\n    end\n  end\nend\n",
        )
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n",
            "class Article < ApplicationRecord\n  def shouted_title\n    Greetings::Formatter.shout(title)\n  end\n\n",
        )
}

#[test]
fn a_class_the_gem_autoloads_runs_from_the_gem_source() {
    app()
        .run_ruby(
            r#"
got = Article.new(title: "hello", body: "a body that is long enough").shouted_title
raise "got #{got.inspect}" unless got == "HELLO!"
puts "ok"
"#,
        )
        .assert_passes();
}
