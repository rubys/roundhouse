#[path = "support/emit_and_run.rs"]
mod emit_and_run;

#[test]
fn duplicate_enum_values_read_the_first_declared_label() {
    emit_and_run::real_blog()
        .edit("db/schema.rb", "create_table \"articles\", force: :cascade do |t|",
            "create_table \"articles\", force: :cascade do |t|\n    t.integer \"stage\"\n    t.string \"tone\"")
        .edit("app/models/article.rb", "class Article < ApplicationRecord\n",
            "class Article < ApplicationRecord\n  enum :stage, { early: 1, late: 1, other: 3 }\n  enum :tone, { soft: \"s\", gentle: \"s\", loud: \"l\" }, prefix: true\n")
        .run_ruby(r#"
article = Article.create!(title: "Aliases", body: "A body long enough to validate.", stage: :late, tone: :gentle)
reloaded = Article.find(article.id)
raise "integer alias" unless reloaded.stage == "early"
raise "string alias" unless reloaded.tone == "soft"
raise "index aliases" unless reloaded[:stage] == "early" && reloaded[:tone] == "soft"
raise "attribute aliases" unless reloaded.attributes["stage"] == "early" && reloaded.attributes["tone"] == "soft"
raise "integer predicate" unless reloaded.early? && reloaded.late? && !reloaded.other?
raise "string predicate" unless reloaded.tone_soft? && reloaded.tone_gentle? && !reloaded.tone_loud?
raise "nil enum" unless Article.new.stage.nil? && Article.new.tone.nil?
reloaded.update!(stage: :other, tone: :loud)
raise "distinct values" unless reloaded.reload.stage == "other" && reloaded.tone == "loud"
"#).assert_passes();
}

#[test]
fn alias_predicates_compare_with_the_canonical_reader_label() {
    let source = b"class Article < ApplicationRecord\n  enum :stage, { early: 1, late: 1, other: 3 }\nend\n";
    let model = roundhouse::ingest::ingest_model(source, "app/models/article.rb", &Default::default(), &Default::default()).unwrap().unwrap();
    for (method, expected) in [("early?", "early"), ("late?", "early"), ("other?", "other")] {
        let body = &model.methods().find(|m| m.name.as_str() == method).unwrap().body;
        let roundhouse::ExprNode::Send { args, .. } = &*body.node else { panic!("{body:?}") };
        assert!(matches!(&*args[0].node, roundhouse::ExprNode::Lit { value: roundhouse::Literal::Str { value } } if value == expected));
    }
}
