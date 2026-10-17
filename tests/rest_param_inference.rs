use roundhouse::analyze::Analyzer;
use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::ty::Ty;

#[test]
fn an_uncalled_rest_parameter_is_an_array_with_unknown_elements() {
    let source = r#"class RestProbe
  def self.length(*values)
    values.length
  end

  def self.values(*values)
    values
  end
end
"#;
    let mut app = ingest_app_from_tree(
        [(
            std::path::PathBuf::from("app/models/rest_probe.rb"),
            source.as_bytes().to_vec(),
        )]
        .into_iter()
        .collect(),
    )
    .expect("ingest rest probe");
    Analyzer::new(&app).analyze(&mut app);
    let probe = app
        .library_classes
        .iter()
        .find(|class| class.name.0.as_str() == "RestProbe")
        .unwrap();
    for (name, expected) in [
        ("length", Ty::Int),
        (
            "values",
            Ty::Array {
                elem: std::sync::Arc::new(Ty::Untyped),
            },
        ),
    ] {
        let method = probe
            .methods
            .iter()
            .find(|method| method.name.as_str() == name)
            .unwrap();
        assert_eq!(method.body.ty.as_ref(), Some(&expected), "{name}");
    }
}
