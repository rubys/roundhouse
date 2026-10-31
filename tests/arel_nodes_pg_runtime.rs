//! PostgreSQL: arel nodes, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;
#[path = "support/pg_probe.rs"]
mod pg_probe;

const STRUCTURE: &str = r#"CREATE TABLE products (
    id bigserial NOT NULL,
    title character varying NOT NULL,
    price integer NOT NULL,
    kind character varying,
    CONSTRAINT products_pkey PRIMARY KEY (id)
);

CREATE TABLE purchases (
    id bigserial NOT NULL,
    product_id bigint NOT NULL,
    qty integer NOT NULL,
    status character varying NOT NULL,
    CONSTRAINT purchases_pkey PRIMARY KEY (id)
);
"#;

const DATABASE_YML: &str = r#"default: &default
  adapter: postgresql
  encoding: unicode

test:
  <<: *default
  database: rh_spec_arel_nodes
"#;

const APPLICATION_RECORD: &str = r#"class ApplicationRecord < ActiveRecord::Base
  self.abstract_class = true
end
"#;

const PRODUCT: &str = r#"class Product < ApplicationRecord
end
"#;

const PURCHASE: &str = r#"class Purchase < ApplicationRecord
end
"#;

const PROBE: &str = r##"class ArelProbe
  def self.titles(relation)
    relation.map(&:title).join(",")
  end

  # The rows of a statement the nodes wrote, read through the model.
  def self.rows_of(sql)
    Product.from("(#{sql}) products").select(:id, :title).order(:id)
  end

  def self.lines
    out = []
    p = Product.arel_table
    o = Purchase.arel_table
    free = Arel::Table.new(name: :products)
    Product.create!(title: "Alpha", price: 5, kind: "a")
    beta = Product.create!(title: "beta", price: 15, kind: "b")
    Product.create!(title: "Gamma", price: 25, kind: "a")
    delta = Product.create!(title: "delta", price: 35, kind: nil)
    Purchase.create!(product_id: beta.id, qty: 2, status: "open")
    Purchase.create!(product_id: beta.id, qty: 3, status: "done")
    Purchase.create!(product_id: delta.id, qty: 7, status: "open")

    out << "predicates: #{[p[:price].eq(5), p[:price].not_eq(5), p[:price].gt(5), p[:price].gteq(5), p[:price].lt(5), p[:price].lteq(5)].map(&:to_sql).join(" | ")}"
    out << "strings: #{[p[:title].eq("it's"), p[:title].matches("a%"), p[:title].does_not_match("a%"), p[:title].eq(nil), p[:title].not_eq(nil)].map(&:to_sql).join(" | ")}"
    out << "lists: #{[p[:price].in([1, 2]), p[:price].not_in([1, 2]), p[:price].between(1..3), p[:title].eq_any(%w[a b]), p[:price].in(1..3)].map(&:to_sql).join(" | ")}"
    out << "functions: #{[p[:title].lower.eq("a"), p[:price].sum, p[:price].count, p[:price].maximum.as("top"), p[:title].lower].map(&:to_sql).join(" | ")}"
    out << "free table: #{[free[:price].gt(1), free[:title].lower.eq("x"), free.alias("p2")[:id].eq(1)].map(&:to_sql).join(" | ")}"
    out << "combinators: #{[p[:price].gt(1).and(p[:price].lt(9)), p[:price].gt(1).or(p[:kind].eq("a")), p[:price].gt(1).not, Arel::Nodes::Grouping.new(Arel.sql("a = b")), Arel::Nodes::And.new([p[:id].eq(1), p[:id].eq(2), p[:id].eq(3)])].map(&:to_sql).join(" | ")}"
    out << "literals: #{[Arel.sql("a = 1"), Arel.sql("price > ?", 5), Arel.sql("kind = :kind AND price < :max", kind: "a", max: 9), Arel::Nodes.build_quoted("x"), Arel::Nodes::Quoted.new(4), Arel::Nodes::True.new, Arel::Nodes::False.new].map { |n| n.respond_to?(:to_sql) ? n.to_sql : n.to_s }.join(" | ")}"
    out << "named functions: #{[Arel::Nodes::NamedFunction.new("COALESCE", [p[:kind], Arel.sql("'none'")]), Arel::Nodes::NamedFunction.new("CAST", [Arel::Nodes::As.new(Arel::Nodes.build_quoted("5"), Arel.sql("integer"))]), Arel::Nodes::NamedFunction.new("ROW_NUMBER", []), Arel::Nodes::NamedFunction.new("GREATEST", [p[:price], 10]), Arel::Nodes::NamedFunction.new("COUNT", [p[:id]]).as("n")].map(&:to_sql).join(" | ")}"
    out << "infix: #{[Arel::Nodes::InfixOperation.new("||", p[:title], Arel::Nodes.build_quoted("!")), p[:price] + 1, p[:price] * 2, Arel::Nodes::Addition.new(p[:price], 3)].map(&:to_sql).join(" | ")}"
    out << "window: #{Arel::Nodes::NamedFunction.new("ROW_NUMBER", []).over(Arel::Nodes::Window.new.partition(p[:kind]).order(p[:price].desc)).to_sql}"
    out << "ordering: #{[p[:price].asc, p[:price].desc, p[:price].desc.nulls_last, p[:price].asc.nulls_first].map(&:to_sql).join(" | ")}"

    kind_rank = Arel::Nodes::Case.new(p[:kind]).when("a").then(1).when("b").then(2).else(0)
    searched = Arel::Nodes::Case.new.when(p[:price].gt(20)).then("high").when(p[:price].gt(10)).then("mid").else("low")
    out << "case: #{[kind_rank, searched, kind_rank.as("rank"), kind_rank.sum].map(&:to_sql).join(" | ")}"

    join = p.join(o).on(p[:id].eq(o[:product_id]))
    outer = p.join(o, Arel::Nodes::OuterJoin).on(p[:id].eq(o[:product_id]))
    out << "joins: #{[join.to_sql, outer.to_sql, p.project(p[:id], o[:qty]).join(o).on(p[:id].eq(o[:product_id])).where(o[:status].eq("open")).order(o[:qty].desc).take(2).to_sql].join(" | ")}"
    out << "manager: #{p.project(p[:id]).where(p[:price].gt(10)).where(p[:kind].eq("a")).group(p[:id]).having(p[:id].gt(1)).order(p[:id].desc).take(3).skip(1).to_sql}"
    sub = o.project(o[:product_id]).where(o[:status].eq("open"))
    out << "subquery: #{[p[:id].in(sub), p[:id].not_in(sub), sub.exists, Arel::Nodes::Union.new(sub, sub), Arel::Nodes::UnionAll.new(sub, sub)].map(&:to_sql).join(" | ")}"
    out << "distinct: #{p.project(p[:kind]).distinct.to_sql} | #{p.project(Arel.star).to_sql} | #{p.project(p[Arel.star]).to_sql}"

    out << "where node: #{titles(Product.where(p[:price].gt(10)).order(:id))} / #{titles(Product.where(p[:title].lower.eq("alpha")))} / #{titles(Product.where(p[:price].gt(10).and(p[:price].lt(30)).or(p[:kind].eq("a"))).order(:id))}"
    out << "where not node: #{titles(Product.where.not(p[:price].gt(10)).order(:id))} / #{titles(Product.where(p[:price].gt(10).not).order(:id))}"
    out << "where in subquery: #{titles(Product.where(p[:id].in(sub)).order(:id))} / #{titles(Product.where(p[:id].not_in(sub)).order(:id))}"
    out << "where exists: #{titles(Product.where(Purchase.where("purchases.product_id = products.id").arel.exists).order(:id))} / #{titles(Product.where.not(Purchase.where("purchases.product_id = products.id").arel.exists).order(:id))}"
    out << "where chained: #{titles(Product.where(p[:price].gt(1)).where(p[:kind].eq("a")).where("price > 10"))}"
    out << "order node: #{titles(Product.order(p[:price].desc))} / #{titles(Product.order(kind_rank.asc, p[:id].desc))} / #{titles(Product.order(p[:kind].asc.nulls_last, p[:id]))}"
    out << "reorder node: #{titles(Product.order(:id).reorder(p[:title].lower.desc))}"
    out << "select node: #{Product.select(p[:id], p[:title].lower.as("t"), kind_rank.as("rank")).to_sql} / #{Product.select(p[:id], p[:price]).order(:id).map { |r| [r.id, r.price] }.inspect}"
    out << "pluck node: #{Product.order(:id).pluck(p[:title].lower).inspect} / #{Product.order(:id).pluck(searched).inspect}"
    out << "group node: #{Product.group(p[:kind]).order(p[:kind].asc.nulls_last).pluck(p[:kind], p[:price].sum).inspect}"
    out << "having node: #{Product.group(p[:kind]).having(p[:price].sum.gt(20)).order(p[:kind]).pluck(p[:kind]).inspect}"
    out << "joins node: #{titles(Product.joins(join.join_sources).where(o[:status].eq("open")).order(:id))} / #{titles(Product.joins(outer.join_sources).where(o[:id].eq(nil)).order(:id))}"
    out << "joins string node: #{titles(Product.joins(Arel::Nodes::InnerJoin.new(o, Arel::Nodes::On.new(p[:id].eq(o[:product_id])))).where(o[:qty].gt(2)).order(:id))}"
    # A VALUES list of bound attributes in a join, then a bound where: the
    # join's binds number first ($1, $2) and the where's follow ($3).
    pair_rows = [[1, "alpha"], [2, "beta"]].map { |id, title| [ActiveRecord::Relation::QueryAttribute.new("id", beta.id, Product.type_for_attribute("id")), ActiveRecord::Relation::QueryAttribute.new("title", title, Product.type_for_attribute("title"))] }
    pairs = Arel::Nodes::As.new(Arel::Nodes::Grouping.new(Arel::Nodes::ValuesList.new(pair_rows)), Arel.sql("pairs(id, title)"))
    pairs_join = Arel::Nodes::InnerJoin.new(pairs, Arel::Nodes::On.new(p[:id].eq(Arel::Nodes::NamedFunction.new("CAST", [Arel::Nodes::As.new(Arel.sql("pairs.id"), Arel.sql("bigint"))]))))
    out << "joins bound values: #{titles(Product.where(kind: "b").joins(pairs_join).order(:id))} / #{titles(Product.joins(pairs_join).where(price: 15).order(:id))}"
    # `model.select(*columns)` on a class-valued reader): the class-side
    # query methods Rails delegates to `all`.
    klass = Object.const_get("Product")
    out << "class query: #{titles(klass.select(:id, :title).order(:id))} / #{titles(klass.order(:id).limit(2).offset(1))} / #{klass.group(:kind).order(:kind).pluck(:kind).inspect} / #{klass.distinct.order(:kind).pluck(:kind).inspect}"
    out << "from node: #{Product.from(Arel::Nodes::TableAlias.new(Arel::Nodes::Grouping.new(Arel.sql(Product.where(p[:price].gt(10)).to_sql)), "products")).order(:id).map(&:title).inspect}"
    ranked = Product.order(:id).pluck(Arel::Nodes::NamedFunction.new("CONCAT", [p[:title], Arel.sql("'-'"), p[:price]]))
    out << "concat: #{ranked.inspect}"

    Product.where(p[:id].eq(beta.id)).update_all(price: Arel::Nodes::Case.new(p[:id]).when(beta.id).then(99).else(p[:price]))
    out << "update_all node: #{Product.order(:id).pluck(:price).inspect}"
    Product.where(p[:id].eq(delta.id)).update_all(price: p[:price] + 1, kind: Arel::Nodes::NamedFunction.new("UPPER", [Arel::Nodes.build_quoted("z")]))
    out << "update_all expr: #{Product.order(:id).pluck(:price, :kind).inspect}"

    relation = Product.where(p[:price].gt(10)).where(p[:kind].eq("a")).order(:id)
    out << "relation arel: #{relation.arel.class.name} #{rows_of(relation.arel.to_sql).map(&:title).inspect}"
    out << "relation arel extend: #{rows_of(Product.where(p[:kind].eq("a")).arel.where(p[:price].gt(10)).to_sql).map(&:title).inspect}"
    out << "relation arel join_sources: #{Product.where(p[:kind].eq("a")).arel.join_sources.inspect}"
    out << "relation arel exists: #{titles(Product.where(Purchase.where(o[:status].eq("open")).where("purchases.product_id = products.id").arel.exists).order(:id))}"
    out << "relation arel union: #{rows_of(Arel::Nodes::UnionAll.new(Product.where(p[:kind].eq("a")).select(:id, :title).arel, Product.where(p[:kind].eq("b")).select(:id, :title).arel).to_sql).map(&:title).sort.inspect}"
    out << "arel_table: #{Product.arel_table.name} #{Product.arel_table[:title].name} #{Product.arel_table[:title].relation.name} #{Arel::Table.new(name: :things).name}"
    out << "classes: #{[Arel::Nodes::Node, Arel::Nodes::Case, Arel::Nodes::NamedFunction, Arel::Attribute, Arel::Nodes::SqlLiteral, Arel::SelectManager].map(&:name).join(",")} #{Arel::Nodes::NamedFunction.ancestors.take(4).map(&:name).join(">")} #{Arel.sql("x").is_a?(String)} #{Arel.sql("x").is_a?(Arel::Nodes::SqlLiteral)} #{Arel.arel_node?(p[:id])} #{Arel.arel_node?("x")}"
    out
  end
end
"##;

/// What native Rails requires, in order.
const LOADS: [&str; 4] = [
    "app/models/application_record",
    "app/models/product",
    "app/models/purchase",
    "app/models/arel_probe",
];

const FILES: [(&str, &str); 8] = [
    ("roundhouse.yml", ""),
    ("config/database.yml", DATABASE_YML),
    (
        "config/routes.rb",
        "Rails.application.routes.draw do\nend\n",
    ),
    ("db/structure.sql", STRUCTURE),
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/models/product.rb", PRODUCT),
    ("app/models/purchase.rb", PURCHASE),
    ("app/models/arel_probe.rb", PROBE),
];

fn overlay() -> emit_and_run::Overlay {
    FILES
        .iter()
        .fold(emit_and_run::empty_app(), |app, (path, text)| {
            app.write(path, text)
        })
}

/// `check`'s errors other than the PostgreSQL adapter notice every
/// PostgreSQL probe app carries.
fn real_errors(errors: &[String]) -> Vec<&String> {
    errors
        .iter()
        .filter(|e| !e.contains("database_adapter not supported"))
        .collect()
}

/// Every Arel node class the probe names resolves: no "constant not
/// supported" for the node hierarchy.
#[test]
#[ignore = "fork spec: postgresql"]
fn arel_node_constants_resolve() {
    let (_, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    let constants: Vec<_> = errors.iter().filter(|e| e.contains("constant not supported")).collect();
    assert!(constants.is_empty(), "{constants:#?}");
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
}

/// The model files natively and emitted, same PostgreSQL.
#[test]
#[ignore = "fork spec: postgresql native control"]
fn arel_nodes_match_native_rails_on_postgresql() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    let Some((native, emitted_lines)) = pg_probe::native_and_emitted(
        "arel-nodes",
        &FILES,
        &emitted,
        "rh_spec_arel_nodes",
        "ArelProbe",
        &LOADS,
    ) else { return };
    assert!(native.len() > 30, "native answered {} lines", native.len());
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    assert_eq!(emitted_lines, native, "emitted");
}
