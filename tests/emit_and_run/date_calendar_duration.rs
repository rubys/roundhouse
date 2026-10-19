use super::emit_and_run;

#[test]
fn calendar_duration_arithmetic_runs_with_rails_dates() {
    let expected = std::process::Command::new("ruby")
        .arg("-e")
        .arg(format!("gem \"activesupport\", \"8.1.4\"\nrequire \"active_support/all\"\n{}\np CalendarDurationProbe.values", include_str!("date_calendar_duration_model.rb")))
        .output().expect("Rails calendar oracle");
    assert!(
        expected.status.success(),
        "{}",
        String::from_utf8_lossy(&expected.stderr)
    );
    let expected = String::from_utf8(expected.stdout).unwrap();
    emit_and_run::empty_app()
        .write("app/services/calendar_duration_probe.rb", include_str!("date_calendar_duration_model.rb"))
        .run_ruby(&format!("expected = {}\nactual = CalendarDurationProbe.values\nraise actual.inspect unless actual == expected\nputs \"Calendar duration arithmetic matches Rails\"\n", expected.trim()))
        .assert_passes();
}
