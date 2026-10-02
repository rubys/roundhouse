pub const RUBY: &str = r#"
class Batch
  def rows
    yield [1, 2]
    nil
  end

  def consume
    total = 0
    rows { |items| items.each { |item| total = total + item } }
    total
  end
end
"#;

pub const RBS: &str = r#"
class Batch
  def rows: () { (Array[Integer]) -> void } -> nil
  def consume: () -> Integer
end
"#;
