//! Spinel consumes these signatures after the shared declarations and the
//! response overlay have been reconciled, so parse the final emitted files.

#[test]
fn emitted_spinel_response_signatures_parse() {
    roundhouse::stack::run(|| {
        let fixture = std::path::Path::new("fixtures/tiny-api");
        let mut app = roundhouse::ingest::ingest_app(fixture).expect("ingest tiny-api");
        roundhouse::session::analyze_and_lower(&mut app);
        let files = roundhouse::project::target_files(
            &app,
            fixture,
            roundhouse::project::BuildTarget::Spinel,
        )
        .expect("emit Spinel");
        for name in [
            "runtime/action_controller/base.rbs",
            "runtime/action_controller_head.rbs",
            "runtime/action_controller.rbs",
        ] {
            let (_, text) = files.iter().find(|(path, _)| path == name).expect(name);
            ruby_rbs::node::parse(text).unwrap_or_else(|e| panic!("{name}: {e}\n{text}"));
        }
    });
}
