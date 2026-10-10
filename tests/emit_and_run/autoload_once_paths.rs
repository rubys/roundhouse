use super::emit_and_run;

const GATEWAY: &str = r#"module Gateway
  def self.ping
    "pong"
  end
end
"#;

#[test]
fn a_constant_from_an_autoload_once_root_runs_in_emitted_ruby() {
    emit_and_run::real_blog()
        .edit(
            "config/application.rb",
            "    config.autoload_lib(ignore: %w[assets tasks])\n",
            "    config.autoload_lib(ignore: %w[assets tasks autoload_once])\n    config.autoload_once_paths << \"#{root}/lib/autoload_once\"\n",
        )
        .write("lib/autoload_once/gateway.rb", GATEWAY)
        .edit(
            "app/models/article.rb",
            "  validates :title, presence: true\n",
            "  validates :title, presence: true\n\n  def ping\n    Gateway.ping\n  end\n",
        )
        .run_ruby(r##"raise "Article#ping answered #{Article.new.ping.inspect}" unless Article.new.ping == "pong""##)
        .assert_passes();
}
