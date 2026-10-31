use super::time_change_usec_contract as contract;

#[test]
fn a_time_changes_integer_microseconds_without_losing_its_offset() {
    let run = contract::overlay().run_ruby(contract::SCRIPT);
    run.assert_passes();
    assert_eq!(run.stdout, contract::EXPECTED, "stderr:\n{}", run.stderr);
}

#[test]
fn a_time_usec_change_keeps_the_system_zone_across_dst() {
    let run = contract::overlay().run_ruby_env(contract::SCRIPT, &[("TZ", "America/New_York")]);
    run.assert_passes();
    assert_eq!(run.stdout, contract::EXPECTED, "stderr:\n{}", run.stderr);
}

#[test]
fn a_time_usec_change_refuses_a_changed_system_timezone() {
    let run = contract::overlay().run_ruby_env(
        r#"
local = Time.at(1730615400).getlocal
ENV["TZ"] = "Asia/Tokyo"
begin
  ClockProbe.with_usec(at: local, value: 42)
  raise "changed system timezone accepted"
rescue NotImplementedError => error
  raise error.message unless error.message.include?("custom or changed system timezone")
end
puts "changed system timezone refused"
"#,
        &[("TZ", "America/New_York")],
    );
    run.assert_passes();
    assert_eq!(run.stdout, "changed system timezone refused\n");
}

#[test]
fn a_time_usec_change_is_explicitly_unsupported_on_strict_targets() {
    let (_emitted, errors) = contract::overlay().emit(roundhouse::project::BuildTarget::Go);
    assert!(
        errors.iter().any(|error| error.contains("ActiveSupport.change_time_usec is only available on the ruby family and spinel")),
        "{errors:#?}"
    );
}

#[test]
fn other_time_change_options_keep_their_original_call() {
    let (emitted, errors) = super::emit_and_run::real_blog()
        .write(
            "app/models/unsupported_clock.rb",
            r#"class UnsupportedClock
  def self.nanoseconds(at: Time.current)
    at.change(nsec: 42)
  end

  def self.fractional(at: Time.current)
    at.change(usec: 0.5)
  end

  def self.multiple(at: Time.current)
    at.change(usec: 42, sec: 1)
  end
end
"#,
        )
        .emit(roundhouse::project::BuildTarget::Ruby);
    assert!(errors.is_empty(), "{errors:#?}");
    let model = std::fs::read_to_string(emitted.join("app/models/unsupported_clock.rb")).unwrap();
    assert!(!model.contains("change_time_usec"), "{model}");
    assert_eq!(model.matches(".change(").count(), 3, "{model}");
}
