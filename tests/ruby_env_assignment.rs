use std::process::Command;

fn run_assignment(source: &str, initial: &str) -> String {
    let parsed = ruby_prism::parse(source.as_bytes());
    let expr = roundhouse::ingest::ingest_expr(
        &parsed
            .node()
            .as_program_node()
            .unwrap()
            .statements()
            .as_node(),
        "env.rb",
    )
    .expect("ingest");
    let emitted = roundhouse::emit::ruby::emit_expr(&expr);
    let script = format!(
        "ENV.clear; keys = ['A', 'B']; calls = 0; {initial}; result = {emitted}; p [result, ENV['A'], ENV['B'], keys, calls]"
    );
    let output = Command::new("ruby")
        .args(["-e", &script])
        .output()
        .expect("ruby");
    assert!(
        output.status.success(),
        "{}\n{script}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("UTF-8")
}

#[test]
fn a_computed_env_key_is_evaluated_once_when_assigning() {
    assert_eq!(
        run_assignment("ENV[keys.shift] ||= 'value'", ""),
        "[\"value\", \"value\", nil, [\"B\"], 0]\n",
    );
    assert_eq!(
        run_assignment("ENV[keys.shift] &&= 'next'", "ENV['A'] = 'old'"),
        "[\"next\", \"next\", nil, [\"B\"], 0]\n",
    );
}

#[test]
fn a_literal_env_key_preserves_short_circuiting() {
    assert_eq!(
        run_assignment("ENV['A'] ||= (calls += 1).to_s", "ENV['A'] = 'kept'"),
        "[\"kept\", \"kept\", nil, [\"A\", \"B\"], 0]\n",
    );
    assert_eq!(
        run_assignment("ENV['A'] ||= (calls += 1).to_s", ""),
        "[\"1\", \"1\", nil, [\"A\", \"B\"], 1]\n",
    );
}
