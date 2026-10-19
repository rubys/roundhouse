# Regenerate expected.rb with the gems the contract is checked against:
#   ruby tests/support/dry_validation/gen.rb tests/support/dry_validation tests/support/dry_validation/cases.txt > tests/support/dry_validation/expected.rb
# Runs each case through the real gems and prints the Ruby assertions the
# lowered contracts must meet.
gem "dry-validation", "1.11.1"; gem "dry-schema", "1.14.1"; gem "dry-types", "1.8.3"
require "dry-validation"; require "bigdecimal"; require "date"; require "time"
CASES = File.read(ARGV[1]).lines.map(&:strip).reject { |l| l.empty? || l.start_with?("#") }
Dir.chdir(ARGV[0])
(Dir["lib/**/*.rb"].sort.sort_by { |f| f.include?("types") ? 1 : 0 } + Dir["app/contracts/*.rb"].sort).each { |f| load File.expand_path(f) }

def lit(v)
  case v
  when Hash then "{" + v.map { |k, x| "#{lit(k)} => #{lit(x)}" }.join(", ") + "}"
  when Array then "[" + v.map { |x| lit(x) }.join(", ") + "]"
  when BigDecimal then "BigDecimal(#{v.to_s("F").inspect})"
  when DateTime then "DateTime.new(#{v.year}, #{v.month}, #{v.day}, #{v.hour}, #{v.minute}, #{v.second})"
  when Date then "Date.new(#{v.year}, #{v.month}, #{v.day})"
  when Time then "Time.new(#{v.year}, #{v.month}, #{v.day}, #{v.hour}, #{v.min}, #{v.sec})"
  when Range then "(#{lit(v.first)}..#{lit(v.last)})"
  # An app object stands for its class (`dv_norm` maps the answer alike).
  when Shop::User, Shop::Guest then v.class.name
  else v.inspect
  end
end

CASES.each_with_index do |line, i|
  callee, input = line.split(" | ", 2)
  begin
    r = eval(callee).call(eval(input))
    puts "dv_check(#{i}, #{callee}, #{input}, #{r.success?}, #{lit(r.to_h)}, #{lit(r.errors.to_h)})"
  rescue => e
    puts "dv_raises(#{i}, #{callee}, #{input}, #{e.class})"
  end
end
