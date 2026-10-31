# Driver for the PostgreSQL probe tests (tests/support/pg_probe.rs): runs
# the probe app's `PROBE.lines` on native Rails and on the emitted program,
# each against its own fresh copy of a template database holding the app's
# db/structure.sql on the ROUNDHOUSE_PG_TEST_URL server, and prints both
# answers as JSON.
#
#   ruby tests/support/pg_probe_driver.rb APP_ROOT OUT_ROOT DATABASE PROBE LOAD...
#
# The server is shared with other tests running at once, so every database
# is named after DATABASE and this driver's pid (`<DATABASE>_<pid>_t`,
# `_n`, `_e`), and dropped at the end.
#
# native: the Rails release tests/support/rails_oracle.rb activates, the app
# files LOAD (relative to APP_ROOT, in order) over `establish_connection`.
# A probe that needs more than Active Record ships `native_prelude.rb`
# beside its files, required after Active Record. emitted: the emitted
# tree's main.rb, its database the one DATABASE_URL names.
require "pg"
require "json"
require "open3"
require "rbconfig"
require_relative "rails_oracle"

app_root, out_root = File.expand_path(ARGV.fetch(0)), File.expand_path(ARGV.fetch(1))
admin_options = PG::Connection.conninfo_parse(ENV.fetch("ROUNDHOUSE_PG_TEST_URL"))
  .each_with_object({}) { |o, h| h[o[:keyword].to_sym] = o[:val] if o[:val] }
# PostgreSQL names are at most 63 bytes: 44 + "_" + pid (<= 7) + "_t".
DATABASE = "#{ARGV.fetch(2)[0, 44]}_#{Process.pid}"
TEMPLATE = "#{DATABASE}_t"
SIDES = { "native" => "#{DATABASE}_n", "emitted" => "#{DATABASE}_e" }.freeze
PROBE = ARGV.fetch(3)
DDL = File.read(File.join(app_root, "db/structure.sql")).freeze
LOADS = ARGV.drop(4).freeze
PRELUDE = File.join(app_root, "native_prelude.rb").then { |file| file if File.exist?(file) }
GEMS = (["activesupport", "activemodel", "activerecord"] + (PRELUDE ? ["activejob"] : [])).freeze

def admin(options)
  conn = PG.connect(**options)
  yield conn
ensure
  conn&.close
end

def create_template(admin_options)
  admin(admin_options) { |conn| conn.exec("CREATE DATABASE #{conn.quote_ident(TEMPLATE)}") }
  admin(admin_options.merge(dbname: TEMPLATE)) { |conn| conn.exec(DDL) }
end

def clone(admin_options, name)
  admin(admin_options) do |conn|
    conn.exec("CREATE DATABASE #{conn.quote_ident(name)} TEMPLATE #{conn.quote_ident(TEMPLATE)}")
  end
end

def drop(admin_options)
  admin(admin_options) do |conn|
    [*SIDES.values, TEMPLATE].each { |name| conn.exec("DROP DATABASE IF EXISTS #{conn.quote_ident(name)} WITH (FORCE)") }
  end
end

# The URL of one side's database on the same server.
def url_for(options, database)
  userinfo = options[:user].to_s
  userinfo += ":#{options[:password]}" if options[:password]
  host = options[:host] || "localhost"
  port = options[:port] ? ":#{options[:port]}" : ""
  "postgres://#{userinfo.empty? ? "" : "#{userinfo}@"}#{host}#{port}/#{database}"
end

clean = %w[RAILS_ENV RACK_ENV DATABASE_URL PGOPTIONS].to_h { |k| [k, nil] }
endpoint = { host: admin_options[:host], port: admin_options[:port], username: admin_options[:user],
             password: admin_options[:password], database: SIDES["native"] }
native_script = <<~RUBY
  require #{File.join(__dir__, "rails_oracle").inspect}
  RailsOracle.activate(#{GEMS.map(&:inspect).join(", ")}, pg: ">= 1.1")
  require "active_record"
  #{PRELUDE ? "require \"active_job\"\n  require #{PRELUDE.inspect}" : ""}
  # Rails puts the app's lib/ on $LOAD_PATH (railties engine.rb, paths["lib"]),
  # so an initializer's `require "x/y"` reads lib/x/y.rb.
  $LOAD_PATH.unshift(#{File.join(app_root, "lib").inspect})
  ActiveRecord::Base.establish_connection(#{{ adapter: "postgresql" }.merge(endpoint.compact).inspect})
  #{LOADS.map { |m| "require #{File.join(app_root, m).inspect}" }.join("\n")}
  puts #{PROBE}.lines
RUBY
emitted_script = <<~RUBY
  require File.expand_path("main", Dir.pwd)
  Main.configure_default_adapter!
  puts #{PROBE}.lines
RUBY

answers = {}
begin
  create_template(admin_options)
  clone(admin_options, SIDES["native"])
  out, err, status = Open3.capture3(clean, RbConfig.ruby, "-e", native_script, chdir: app_root)
  answers["native"] = { "lines" => out.lines.map(&:chomp), "ok" => status.success?, "stderr" => err }
  clone(admin_options, SIDES["emitted"])
  out, err, status = Open3.capture3(clean.merge("DATABASE_URL" => url_for(admin_options, SIDES["emitted"]), "RAILS_ENV" => "test"),
    RbConfig.ruby, "-e", emitted_script, chdir: out_root)
  answers["emitted"] = { "lines" => out.lines.map(&:chomp), "ok" => status.success?, "stderr" => err }
ensure
  drop(admin_options)
end
puts JSON.generate(answers)
