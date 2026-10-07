//! Aliases become visible where the source declares them in a stateful body.
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

fn source(is_module: bool, keyword: bool, accessor: bool) -> String {
    let declaration = if is_module { "module" } else { "class" };
    let original = if accessor { "attr_reader :original" } else { "def original; @original; end" };
    let alias = if keyword { "alias renamed original" } else { "alias_method :renamed, :original" };
    format!("{declaration} AliasProbe\n  def initialize; @original = 19; end\n  {original}\n  \
        @before = AliasProbe.instance_methods.include?(:renamed)\n  \
        {alias}\n  @after = AliasProbe.instance_methods.include?(:renamed)\n  \
        def self.observations\n    [@before, @after]\n  end\nend\n")
}

#[test]
fn class_and_module_aliases_follow_their_declarations() {
    for is_module in [false, true] {
        for keyword in [false, true] {
            for accessor in [false, true] {
                let mut contract = String::from(
                    "raise \"alias visible before its declaration\" \
                        unless AliasProbe.observations == [false, true]\n",
                );
                contract.push_str(if is_module {
                    "class AliasConsumer; include AliasProbe; end\n\
                        raise \"alias body changed\" unless AliasConsumer.new.renamed == 19\n"
                } else {
                    "raise \"alias body changed\" unless AliasProbe.new.renamed == 19\n"
                });
                contract.push_str("puts \"alias source order passed\"\n");
                emit_and_run::empty_app()
                    .write("db/schema.rb", "ActiveRecord::Schema.define do\n  \
                        create_table :items do |t|\n    t.string :name\n  end\nend\n")
                    .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
                    .write("app/controllers/application_controller.rb",
                        "class ApplicationController < ActionController::Base\nend\n")
                    .write("app/services/alias_probe.rb", &source(is_module, keyword, accessor))
                    .run_ruby(&contract)
                    .assert_passes();
            }
        }
    }
}

#[test]
fn alias_method_spans_point_to_the_alias_declaration() {
    for is_module in [false, true] {
        for keyword in [false, true] {
            for accessor in [false, true] {
                let source = source(is_module, keyword, accessor);
                let classes = roundhouse::ingest::ingest_library_classes(source.as_bytes(), "probe.rb")
                    .unwrap();
                let probe = classes.iter().find(|class| class.name.0.as_str() == "AliasProbe").unwrap();
                let alias = probe.methods.iter().find(|method| method.name.as_str() == "renamed").unwrap();
                let declaration = if keyword { "alias renamed original" } else { "alias_method :renamed, :original" };
                assert_eq!(&source[alias.name_span.start as usize..alias.name_span.end as usize], declaration);
            }
        }
    }
}
