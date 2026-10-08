//! An app engine's `isolate_namespace` prefixes its models' tables.
//!
//! A `billing` engine calls
//! `isolate_namespace Billing`, and Rails defines
//! `Billing.table_name_prefix` as `billing_`.
//! `Billing::Rate` declares no `table_name`; it read `rates`, a table
//! no schema has, so the emitted SQL named the wrong table and every column
//! read on a rate was `no known method`. A module's own
//! `table_name_prefix` still wins, as it does in Rails.

use std::collections::HashMap;
use std::path::PathBuf;

const STRUCTURE: &str = "CREATE TABLE fx_eng_rates (\n    id bigint NOT NULL,\n    code_to character varying(8) NOT NULL,\n    CONSTRAINT fx_eng_rates_pkey PRIMARY KEY(id)\n);\n\n\
CREATE TABLE custom_quotes (\n    id bigint NOT NULL,\n    amount integer NOT NULL,\n    CONSTRAINT custom_quotes_pkey PRIMARY KEY(id)\n);\n";

const ENGINE: &str = "module FxEng\n  class Engine < ::Rails::Engine\n    isolate_namespace FxEng\n  end\nend\n";
const QUOTES_ENGINE: &str = "module Quotes\n  class Engine < ::Rails::Engine\n    isolate_namespace Quotes\n  end\nend\n";
const QUOTES_PREFIX: &str = "module Quotes\n  def self.table_name_prefix\n    \"custom_\"\n  end\nend\n";

const READER: &str = r#"class Reader
  #: (FxEng::Rate) -> String
  def self.code(rate) = rate.code_to

  #: (Quotes::Quote) -> Integer
  def self.amount(quote) = quote.amount
end
"#;

#[test]
fn an_isolated_namespace_prefixes_its_model_tables() {
    let tree: HashMap<PathBuf, Vec<u8>> = [
        ("db/structure.sql", STRUCTURE),
        ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
        ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n"),
        ("lib/fx_eng/engine.rb", ENGINE),
        ("lib/quotes/engine.rb", QUOTES_ENGINE),
        ("app/models/quotes.rb", QUOTES_PREFIX),
        ("app/models/fx_eng/rate.rb", "module FxEng\n  class Rate < ApplicationRecord\n  end\nend\n"),
        ("app/models/quotes/quote.rb", "module Quotes\n  class Quote < ApplicationRecord\n  end\nend\n"),
        ("app/models/reader.rb", READER),
    ]
    .into_iter()
    .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
    .collect();
    let mut app = roundhouse::ingest::ingest_app_from_tree(tree).expect("ingest tree");
    let table = |name: &str| app.models.iter().find(|m| m.name.0.as_str() == name).map(|m| m.table.0.to_string());
    assert_eq!(table("FxEng::Rate").as_deref(), Some("fx_eng_rates"));
    assert_eq!(table("Quotes::Quote").as_deref(), Some("custom_quotes"));
    roundhouse::session::analyze_and_lower(&mut app);
    let errors: Vec<String> = roundhouse::analyze::diagnose(&app).into_iter().map(|d| d.to_string()).collect();
    assert!(!errors.iter().any(|e| e.contains("code_to") || e.contains("amount")), "{errors:?}");
}
