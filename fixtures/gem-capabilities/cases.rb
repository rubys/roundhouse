# No gem registry, reflection discovery, or expected values computed from emissions.
module Cases
  HISTORICAL = {
    "alba" => ["4.0.0", ["alba"], "PASS inherited/nested serialization"],
    "faraday" => ["2.14.4", ["faraday"], "PASS status/header/body and stub consumption (no network adapter)"],
    "nokogiri" => ["1.19.4", ["nokogiri"], "PASS XML predicate/entity/count"],
    "oj" => ["3.17.7", ["oj"], "PASS strict JSON false/null/array roundtrip"],
    "pagy" => ["43.6.3", ["pagy"], "PASS short last page and overflow empty array"],
    "bcrypt" => ["3.1.22", ["bcrypt"], "PASS correct/wrong password (test cost 4)"],
    "dry-validation" => ["1.11.1", ["dry/validation"], "PASS valid age and rejected age with exact error"],
    "rubyzip" => ["3.7.0", ["zip"], "PASS in-memory ZIP name/payload/EOF roundtrip"],
    "json" => ["3.0.2", ["json"], "PASS JSON false/null/array roundtrip"],
    "active_model_serializers" => ["0.10.16", ["active_support/all", "active_model_serializers"], "PASS AMS inherited/nested/false serialization"],
    "oj_serializers" => ["3.0.2", ["rails", "active_support/all", "oj_serializers"], "PASS Oj serializer inherited/nested/false serialization"],
    "panko_serializer" => ["0.8.5", ["active_record", "active_support/all", "panko_serializer"], "PASS Panko inherited/nested/false serialization"],
    "inertia_rails" => ["3.22.0", ["rails", "action_controller", "inertia_rails"], "PASS Inertia full/partial false/null props+headers; DELETE/POST redirect boundary (no HTTP/app/DB)"],
    "typelizer" => ["0.13.1", ["alba", "typelizer"], "PASS Typelizer Alba explicit number/optional/nullable metadata (no model/DB inference)"]
  }.freeze

  def self.all(root)
    historical = File.join(root, "fixtures/gem-capabilities/historical")
    rows = HISTORICAL.to_h do |name, (version, requires, marker)|
      [name, {"gem" => name, "version" => version, "requires" => requires,
              "source" => "#{historical}/sources/#{name}.rb", "classes" => "#{historical}/sources/#{name}",
              "contract" => "#{historical}/contracts/#{name}.rb", "lock" => "#{historical}/locks/#{name}.lock",
              "expected_stdout" => marker + "\n", "family" => "historical"}]
    end
    {"declarations-alba" => ["alba", "alba", "PASS Alba declarations inheritance false/null presence"],
     "declarations-dry" => ["dry-validation", "dry", "PASS dry declarations inheritance coercion 17/18/19 false/null"]}.each do |id, (gem, path, marker)|
      dir = File.join(root, "fixtures/gem-capabilities/declarations", path)
      rows[id] = rows.fetch(gem).merge("source" => "#{dir}/source.rb", "classes" => "#{dir}/classes",
                                     "contract" => "#{dir}/contract.rb", "expected_stdout" => marker + "\n",
                                     "family" => "declarative-class-bodies")
    end
    rows.each { |id, row| row["consumer"] = File.join(root, "fixtures/gem-capabilities/consumers", "#{id}.rb") }
    rows
  end

  CORE = %w[alba dry-validation bcrypt declarations-alba declarations-dry].freeze
end
