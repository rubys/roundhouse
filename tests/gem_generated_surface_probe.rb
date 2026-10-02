# Manual oracle for attribution's naming rules, not emitted gem support.
# The ignored Rust test supplies a class name and the literal declaration
# on stdin, then compares the gem's actual method delta with the harvester.
# Run: cargo test --lib native_generator_rules_match_pinned_gems -- --ignored
# Install these exact native gems first; normal CI does not require them.
gem 'activerecord', '8.1.4'
gem 'activesupport', '8.1.4'
gem 'sqlite3', '2.9.6'
gem 'aasm', '6.0.0'
gem 'state_machines', '0.202.0'
gem 'state_machines-activemodel', '0.200.0'
gem 'state_machines-activerecord', '0.200.0'
require 'json'
require 'active_record'
require 'aasm'
require 'state_machines-activerecord'

ActiveRecord::Base.establish_connection(adapter: 'sqlite3', database: ':memory:')
ActiveRecord::Schema.verbose = false
ActiveRecord::Schema.define do
  create_table(:articles) do |t|
    %i[state status review alarm_state].each { |name| t.string name }
  end
end
class ApplicationRecord < ActiveRecord::Base
  self.table_name = 'articles'
  include AASM
end

Object.class_eval(STDIN.read, 'native-declaration.rb')
declaring = Object.const_get(ARGV.fetch(0))
instance = declaring.instance_methods - ApplicationRecord.instance_methods
singleton = declaring.methods - ApplicationRecord.methods
# Shared integration helpers are independent of declared names, unlike
# the surface under test. This is not the generator's naming algorithm.
fixed = %w[fire_events fire_events! initialize_state_machines state_machines]
puts JSON.generate(((instance + singleton).map(&:to_s) - fixed).uniq.sort)
