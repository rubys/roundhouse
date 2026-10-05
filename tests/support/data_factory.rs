pub const DECLARATIONS: &str = r#"module FactoryExamples
  class First
    Result = Data.define(:name, :score, :enabled)
    Alias = Result
    ChainedAlias = Alias

    def self.build
      Result.new(name: "first", score: 0.8, enabled: false)
    end

    def self.aliased
      ChainedAlias.new(name: nil, score: 0.0, enabled: true)
    end

    def self.qualified
      FactoryExamples::First::Result.new("qualified", 1.0, false)
    end
  end

  class Second
    Result = Data.define(:name)

    def self.build
      Result.new(name: "second")
    end
  end

  class Empty
    Result = Data.define

    def self.build
      Result.new
    end
  end
end
"#;
