//! `JsonBuilder.encode_value` on an Array, compiled by Spinel: each
//! element is encoded as a scalar value is (a Hash element as a JSON
//! object), so the value is a JSON array and not the Array's `to_s`. The CRuby side is `jbuilder_array_value.rs`.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

#[test]
#[ignore = "requires the Spinel toolchain"]
fn an_array_value_is_a_json_array_under_spinel() {
    let overlay = emit_and_run::empty_app()
        .write(
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        )
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n");
    let run = overlay.run_spinel(
        r#"
tags = JsonBuilder.encode_value(["b", nil])
raise "tags: #{tags}" unless tags == "[\"b\",null]"
numbers = JsonBuilder.encode_value([1, 2])
raise "numbers: #{numbers}" unless numbers == "[1,2]"
mixed = JsonBuilder.encode_value([{ "name" => "b" }, [1, nil]])
raise "mixed: #{mixed}" unless mixed == "[{\"name\":\"b\"},[1,null]]"
puts "ok"
"#,
    );
    run.assert_passes();
}
