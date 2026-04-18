use serde::{Deserialize, Serialize};

use crate::dialect::{Controller, Model, RouteTable, TestModule, View};
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
        }
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}
