# Runs each case through the real gems and prints the Ruby assertions the
# lowered code must meet. Regenerate expected.rb with:
#   ruby tests/support/dry_monads/gen.rb tests/support/dry_monads tests/support/dry_monads/cases.txt > tests/support/dry_monads/expected.rb
gem "dry-monads", "1.9.0"; gem "dry-validation", "1.11.1"; gem "dry-schema", "1.14.1"
require "dry/monads"; require "dry-validation"
CASES = File.read(ARGV[1]).lines.map(&:strip).reject { |l| l.empty? || l.start_with?("#") }
Dir.chdir(ARGV[0])
(Dir["app/contracts/*.rb"].sort + Dir["app/services/*.rb"].sort.sort_by { |f| f.include?("base") ? 0 : 1 }).each { |f| load File.expand_path(f) }
CASES.each_with_index do |source, i|
  begin
    puts "dm_check(#{i}, (#{source}).inspect, #{eval(source).inspect.inspect})"
  rescue => e
    puts "dm_raises(#{i}, -> { #{source} }, #{e.class}, #{e.message.inspect})"
  end
end
