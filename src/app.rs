use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::dialect::{Controller, Fixture, Model, RouteTable, TestModule, View};
use crate::expr::Expr;
use crate::ident::{ClassId, Symbol};
use crate::schema::Schema;
use crate::ty::Ty;

/// The top-level IR: a Rails application as data. This is the serializable
/// deliverable — the thing ingesters produce and emitters consume.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct App {
    pub schema_version: u32,
    pub schema: Schema,
    pub models: Vec<Model>,
    pub controllers: Vec<Controller>,
    pub routes: RouteTable,
    pub views: Vec<View>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub test_modules: Vec<TestModule>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fixtures: Vec<Fixture>,
    /// Body of `db/seeds.rb` as a typed expression (usually a
    /// `Seq` of AR-create calls with an early-return guard). The
    /// TS emitter wraps it in `async function run()` and the
    /// generated `main.ts` invokes it at startup when the DB is
    /// fresh. None when the app has no seeds file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seeds: Option<Expr>,
    /// Pins from `config/importmap.rb`, expanded (each
    /// `pin_all_from` has been resolved into explicit per-file
    /// pins via `app/javascript/**` walking). Consumed by the
    /// `<%= javascript_importmap_tags %>` view-helper lowering.
    /// None when the app has no importmap.rb.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub importmap: Option<Importmap>,
    /// Logical stylesheet names discovered in `app/assets/stylesheets/`
    /// + `app/assets/builds/` (file stems without `.css`). When the
    /// ERB uses `stylesheet_link_tag :app, ...`, Rails with Propshaft
    /// + tailwindcss-rails expands to one `<link>` per stylesheet in
    /// these dirs; our emitter mirrors the expansion so the rendered
    /// head matches structurally.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stylesheets: Vec<String>,
    /// User-authored RBS sidecars discovered under `sig/**/*.rbs` in
    /// the Rails app root. Keyed by fully-qualified class/module name
    /// (nested namespaces joined with `::`), inner map is method name
    /// → signature (`Ty::Fn`). The analyzer consults these when
    /// building `ClassInfo` so user methods the Rails conventions
    /// can't fully type (helpers, concerns, POROs) still flow types.
    /// Empty when the app ships no `sig/` directory.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub rbs_signatures: HashMap<ClassId, HashMap<Symbol, Ty>>,
}

/// A Rails-style importmap: one `<name>` → `<path>` entry per
/// pin, in declaration order (Rails preserves order for
/// modulepreload link emission).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct Importmap {
    pub pins: Vec<ImportmapPin>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ImportmapPin {
    /// Module specifier the page imports (`"application"`,
    /// `"@hotwired/turbo-rails"`, `"controllers/hello_controller"`).
    pub name: String,
    /// Served asset path (`/assets/application.js`,
    /// `/assets/turbo.min.js`, …). Canonical (no fingerprint);
    /// real deployments sprinkle digests in here.
    pub path: String,
}

impl App {
    pub const SCHEMA_VERSION: u32 = 1;

    pub fn new() -> Self {
        Self {
            schema_version: Self::SCHEMA_VERSION,
            schema: Schema::default(),
            models: Vec::new(),
            controllers: Vec::new(),
            routes: RouteTable::default(),
            views: Vec::new(),
            test_modules: Vec::new(),
            fixtures: Vec::new(),
            seeds: None,
            importmap: None,
            stylesheets: Vec::new(),
            rbs_signatures: HashMap::new(),
        }
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}
