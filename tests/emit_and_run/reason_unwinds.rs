#[test]
fn rescued_returns_and_forwarded_lambdas_keep_the_live_receiver() {
    crate::emit_and_run::real_blog()
        .write("lib/return_probes.rb", r#"module ReturnProbes
  def self.ensure_rebound
    JSON.parse("42").tap do
      p = Kernel.proc { return :escaped }
      begin
        p = -> { 42 }
      ensure
        p.call
      end
    end
  end

  def self.rescue_rebound
    JSON.parse("42").tap do
      p = Kernel.proc { return :escaped }
      begin
        p = -> { 42 }
        Kernel.raise("x")
      rescue RuntimeError
        p.call
      end
    end
  end

  def self.rescued_operand
    JSON.parse('"live"').tap do
      begin
        return Integer("bad")
      rescue ArgumentError
        nil
      end
    end
  end

  def self.forwarded_lambda
    JSON.parse('"live"').tap(&->(x) { return :local })
  end

  def self.rescued_prefix
    JSON.parse('"live"').tap do
      begin
        Integer("bad")
        return :escaped
      rescue ArgumentError
        nil
      end
    end
  end

  def self.rescued_ensure
    JSON.parse('"live"').tap do
      begin
        begin
          return :escaped
        ensure
          Integer("bad")
        end
      rescue ArgumentError
        nil
      end
    end
  end

  def self.strict_lambda
    JSON.parse("42").tap(&-> { "unused" })
  end

  def self.array_value
    ["ok"].dig(JSON.parse("0")) || "fallback"
  end

  def self.hash_value
    {"k" => "ok"}.dig(JSON.parse('"k"')) || "fallback"
  end
end
"#)
        .run_ruby(r#"
raise "ensure rebound" unless ReturnProbes.ensure_rebound == 42
raise "rescue rebound" unless ReturnProbes.rescue_rebound == 42
raise "rescued operand" unless ReturnProbes.rescued_operand == "live"
raise "lambda scope" unless ReturnProbes.forwarded_lambda == "live"
raise "rescued prefix" unless ReturnProbes.rescued_prefix == "live"
raise "rescued ensure" unless ReturnProbes.rescued_ensure == "live"
begin
  ReturnProbes.strict_lambda
  raise "lambda arity was lost"
rescue ArgumentError
end
raise "array dig" unless ReturnProbes.array_value == "ok"
raise "hash dig" unless ReturnProbes.hash_value == "ok"
"#)
        .assert_passes();
}
