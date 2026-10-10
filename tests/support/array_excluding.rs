pub const SOURCE: &str = r#"class ArrayExcludingProbe
  def self.results
    items = [3, 1, 2, 1]
    [
      items.excluding,
      items.excluding(2),
      items.without(3, 1),
      items.excluding([2, 3]),
      items.excluding(9),
      items,
    ]
  end
end
"#;

pub const ASSERTIONS: &str = r#"expected = [[3, 1, 2, 1], [3, 1, 1], [2], [1, 1], [3, 1, 2, 1], [3, 1, 2, 1]]
raise "Array#excluding/#without parity: #{ArrayExcludingProbe.results.inspect}" unless ArrayExcludingProbe.results == expected
puts "Array excluding emitted contract passed"
"#;
