use roundhouse::analyze::Analyzer;
use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::ty::Ty;

#[test]
fn time_addition_keeps_the_time_result_without_widening_subtraction() {
    let source = r#"class AdditionClock
  def self.integer_seconds
    Time.now + 5
  end
  def self.fractional_seconds
    Time.now + 0.5
  end
  def self.duration_local
    delay = 5.seconds
    Time.now + delay
  end
  def self.difference
    Time.now - Time.now
  end
end

class CustomClock
  def +(value)
    value + 1
  end
  def self.result
    CustomClock.new + 5
  end
end
"#;
    let mut app = ingest_app_from_tree(
        [(
            std::path::PathBuf::from("app/models/addition_clock.rb"),
            source.as_bytes().to_vec(),
        )]
        .into_iter()
        .collect(),
    )
    .unwrap();
    Analyzer::new(&app).analyze(&mut app);
    let clock = app
        .library_classes
        .iter()
        .find(|class| class.name.0.as_str() == "AdditionClock")
        .unwrap();
    for name in ["integer_seconds", "fractional_seconds", "duration_local"] {
        let method = clock
            .methods
            .iter()
            .find(|method| method.name.as_str() == name)
            .unwrap();
        assert_eq!(method.body.ty, Some(Ty::Time), "{name}");
    }
    let difference = clock
        .methods
        .iter()
        .find(|method| method.name.as_str() == "difference")
        .unwrap();
    assert_eq!(difference.body.ty, Some(Ty::untyped()));
    let custom = app
        .library_classes
        .iter()
        .find(|class| class.name.0.as_str() == "CustomClock")
        .unwrap();
    let method = custom
        .methods
        .iter()
        .find(|method| method.name.as_str() == "result")
        .unwrap();
    assert_eq!(method.body.ty, Some(Ty::Int));
}
