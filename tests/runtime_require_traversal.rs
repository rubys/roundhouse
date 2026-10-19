#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::project::BuildTarget;

fn assert_loads(source: &str, script: &str) {
    for target in [BuildTarget::Ruby, BuildTarget::Spinel] {
        let (emitted, errors) = emit_and_run::empty_app()
            .write("app/services/registry.rb", source)
            .emit(target);
        assert!(errors.is_empty(), "{target:?}: {errors:?}");
        let output = std::process::Command::new("ruby")
            .args(["--enable-frozen-string-literal", "-r"])
            .arg(emitted.join("app/models/registry.rb"))
            .args(["-e", script])
            .output()
            .expect("run emitted class");
        assert!(
            output.status.success(),
            "{target:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(String::from_utf8_lossy(&output.stdout), "OK\n");
    }
}

#[test]
fn a_memoized_concurrent_map_loads_its_runtime() {
    assert_loads(
        r#"class Registry
  def self.map
    @map ||= Concurrent::Map.new
  end
end
"#,
        r#"map = Registry.map
map[:key] = 17
raise unless map.equal?(Registry.map) && Registry.map[:key] == 17
raise unless map.compute_if_absent(:other) { 29 } == 29
puts "OK"
"#,
    );
}

#[test]
fn an_assignment_receiver_loads_its_runtime() {
    assert_loads(
        r#"class Registry
  def self.value
    Concurrent::Map.new[:key] = 17
  end
end
"#,
        "raise unless Registry.value == 17\nputs \"OK\"\n",
    );
}

#[test]
fn memoization_loads_other_runtime_namespaces() {
    assert_loads(r#"class Registry
  def self.address
    @address ||= IPAddr.new("127.0.0.1")
  end
end
"#, "raise unless Registry.address.ipv4? && Registry.address.equal?(Registry.address)\nputs \"OK\"\n");
}
