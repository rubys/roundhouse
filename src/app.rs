use serde::{Deserialize, Serialize};

use crate::dialect::{Controller, Fixture, Model, RouteTable, TestModule, View};
use crate::expr::Expr;
use crate::schema::Schema;

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
        }
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}
