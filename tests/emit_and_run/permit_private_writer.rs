//! A permitted key whose writer isn't public. Rails assigns permitted
//! params through `assign_attributes`, which calls `public_send`, so
//! `Article.new(article_params)` with a private `shout=` raises
//! `ActiveModel::UnknownAttributeError` (`rails_refuses_a_private_writer_key`,
//! activerecord and actionpack 8.1.4). The lowered permit filter keeps
//! only the keys that have a writer and drops the rest, so a private
//! writer's key is dropped the same way and the writer isn't called,
//! from `create` or `update`. A public hand-written writer still is.

use super::emit_and_run;

const WRITERS: &str = r##"
  def headline=(value)
    self.title = "H: #{value}"
  end

  def whisper=(value)
    self.title = value.downcase
  end
  private :whisper=

  private def yell=(value)
    self.title = value.upcase + "!"
  end

  private

  def shout=(value)
    self.title = value.upcase
  end
end
"##;

const MODEL_TAIL: &str = "  validates :body, presence: true, length: { minimum: 10 }\nend\n";

fn app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .edit("app/models/article.rb", MODEL_TAIL, &format!("  validates :body, presence: true, length: {{ minimum: 10 }}\n{WRITERS}"))
        .edit(
            "app/controllers/articles_controller.rb",
            "params.expect(article: [ :title, :body ])",
            "params.expect(article: [ :title, :body, :headline, :whisper, :yell, :shout ])",
        )
}

#[test]
fn a_permitted_private_writer_is_not_called() {
    app()
        .run_ruby(
            r##"
require "stringio"
ActionController::Base.allow_forgery_protection = false
def request(verb, path, body)
  env = {
    "REQUEST_METHOD" => verb, "PATH_INFO" => path, "QUERY_STRING" => "",
    "CONTENT_TYPE" => "application/x-www-form-urlencoded",
    "CONTENT_LENGTH" => body.bytesize.to_s, "HTTP_ACCEPT" => "application/json"
  }
  status, _body = Main.dispatch_core(env, StringIO.new(body))
  status
end
BODY = "article%5Bbody%5D=A+body+long+enough"
%w[whisper yell shout].each do |key|
  status = request("POST", "/articles.json", "article%5Btitle%5D=Plain+#{key}&article%5B#{key}%5D=Loud&#{BODY}")
  raise "create #{key}: #{status.inspect}" unless status == 201
  article = Article.find_by(title: "Plain #{key}")
  raise "create called the private #{key}=: #{Article.last.title.inspect}" unless article
  status = request("PATCH", "/articles/#{article.id}.json", "article%5B#{key}%5D=Loud")
  raise "update #{key}: #{status.inspect}" unless status == 200
  raise "update called the private #{key}=: #{article.reload.title.inspect}" unless article.reload.title == "Plain #{key}"
end
status = request("POST", "/articles.json", "article%5Bheadline%5D=News&#{BODY}")
raise "headline: #{status.inspect}" unless status == 201 && Article.find_by(title: "H: News")
puts "private writers are not called"
"##,
        )
        .assert_passes();
}

/// The same model and permit list against activerecord and actionpack
/// 8.1.4: each private writer's key raises from `new` and from `update`,
/// and the public one assigns. Skipped when the gems aren't installed.
#[test]
fn rails_refuses_a_private_writer_key() {
    let probe = r#"gem "activerecord", "8.1.4"; gem "actionpack", "8.1.4"; require "active_record"; require "action_controller"; require "sqlite3""#;
    if !std::process::Command::new("ruby").args(["-e", probe]).status().is_ok_and(|s| s.success()) {
        eprintln!("skipping: ruby with activerecord/actionpack 8.1.4 and sqlite3 not available");
        return;
    }
    let script = format!(
        r##"gem "activerecord", "8.1.4"
gem "actionpack", "8.1.4"
require "active_record"
require "action_controller"
require "sqlite3"
ActiveRecord::Base.establish_connection(adapter: "sqlite3", database: ":memory:")
ActiveRecord::Schema.verbose = false
ActiveRecord::Schema.define do
  create_table(:articles) {{ |t| t.string :title; t.text :body }}
end
class Article < ActiveRecord::Base
{WRITERS}
def permitted(fields)
  ActionController::Parameters.new(article: fields).expect(article: [:title, :body, :headline, :whisper, :yell, :shout])
end
%w[whisper yell shout].each do |key|
  begin
    Article.new(permitted("title" => "Plain", key => "Loud"))
    raise "new accepted #{{key}}"
  rescue ActiveModel::UnknownAttributeError
  end
  article = Article.create!(title: "Plain", body: "A body long enough")
  begin
    article.update(permitted(key => "Loud"))
    raise "update accepted #{{key}}"
  rescue ActiveModel::UnknownAttributeError
  end
end
raise "headline" unless Article.new(permitted("headline" => "News")).title == "H: News"
puts "PASS""##
    );
    let output = std::process::Command::new("ruby").arg("-e").arg(&script).output().expect("ruby is on PATH");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success() && stdout.contains("PASS"),
        "activerecord/actionpack 8.1.4 disagree\n=== stdout ===\n{stdout}\n=== stderr ===\n{stderr}"
    );
}
