use roundhouse::analyze::Analyzer;
use roundhouse::diagnostic::Severity;
use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::ty::Ty;

fn app(body: &str) -> roundhouse::App {
    let source = format!("class CalendarProbe\n  def self.value\n    {body}\n  end\nend\n");
    ingest_app_from_tree(
        [(
            std::path::PathBuf::from("app/services/calendar_probe.rb"),
            source.into_bytes(),
        )]
        .into_iter()
        .collect(),
    )
    .unwrap()
}
#[test]
fn explicit_integer_calendar_builders_return_dates() {
    for unit in [
        "day", "days", "week", "weeks", "month", "months", "year", "years",
    ] {
        for op in ["+", "-"] {
            let mut app = app(&format!("Date.new(2024, 3, 31) {op} 2.{unit}"));
            Analyzer::new(&app).analyze(&mut app);
            let class = app
                .library_classes
                .iter()
                .find(|c| c.name.0.as_str() == "CalendarProbe")
                .unwrap();
            assert_eq!(class.methods[0].body.ty, Some(Ty::Date), "{op} {unit}");
            let diagnostics = roundhouse::session::analyze_and_lower(&mut app);
            assert!(!diagnostics.iter().any(|d| d.severity == Severity::Error));
            let ruby = roundhouse::emit::ruby::emit_library(&app)
                .into_iter()
                .map(|file| file.content)
                .collect::<Vec<_>>()
                .join("\n");
            let plural = if unit.ends_with('s') {
                unit.to_string()
            } else {
                format!("{unit}s")
            };
            let direction = if op == "+" { "since" } else { "ago" };
            assert!(
                ruby.contains(&format!(
                    "ActiveSupport.date_{plural}_{direction}(Date.new(2024, 3, 31), 2)"
                )),
                "{ruby}"
            );
        }
    }
}
