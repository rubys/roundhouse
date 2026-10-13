pub const SOURCE: &str = r#"class ArrayExcludingProbe
  def self.results
    [
      [3, 1, 2, 1].excluding,
      [3, 1, 2, 1].excluding(2),
      [3, 1, 2, 1].without(3, 1),
      [3, 1, 2, 1].excluding([2, 3]),
      [3, 1, 2, 1].excluding(9),
      [3, 1, 2, 1],
    ]
  end
end
"#;

pub const ASSERTIONS: &str = r#"expected = [[3, 1, 2, 1], [3, 1, 1], [2], [1, 1], [3, 1, 2, 1], [3, 1, 2, 1]]
raise "Array#excluding/#without parity: #{ArrayExcludingProbe.results.inspect}" unless ArrayExcludingProbe.results == expected
puts "Array excluding emitted contract passed"
"#;
