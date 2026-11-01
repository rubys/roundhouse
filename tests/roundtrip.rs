//! Round-trip test: IR → JSON → IR preserves semantics.
//!
//! The forcing function for IR completeness. If constructing an App by hand
//! and round-tripping through JSON loses information, the shape is wrong.
//! Extend this test as the IR grows — every new node kind gets exercised here.

use indexmap::IndexMap;
use roundhouse::{
    Action, App, ClassId, Column, ColumnType, Controller, Effect, EffectSet, Expr, ExprNode,
    HttpMethod, Literal, Model, RenderTarget, RouteTable, Row, Schema, Symbol, Table,
    TableRef, Ty,
};
use roundhouse::span::Span;

fn sp() -> Span {
    Span::synthetic()
}

/// Verifies that a representative blog app survives an IR serialization round-trip.
#[test]
fn tiny_blog_round_trips() {
    let mut tables = IndexMap::new();
    tables.insert(
        Symbol::from("posts"),
        Table {
            name: Symbol::from("posts"),
            columns: vec![
                Column {
                    name: Symbol::from("id"),
                    col_type: ColumnType::BigInt,
                    nullable: false,
                    default: None,
                    primary_key: true,
                    generated: None,
                    generated_text_compatible: None,
                    generated_int4_compatible: None,
                    array: false,
                    array_stored: false,
                },
                Column {
                    name: Symbol::from("title"),
                    col_type: ColumnType::String { limit: None },
                    nullable: false,
                    default: None,
                    primary_key: false,
                    generated: None,
                    generated_text_compatible: None,
                    generated_int4_compatible: None,
                    array: false,
                    array_stored: false,
                },
            ],
            indexes: vec![],
            foreign_keys: vec![],
            virtual_module: None,
            known_keyless: false,
            primary_key_columns: Vec::new(),
        },
    );
    let schema = Schema { tables };

    let mut attrs = IndexMap::new();
    attrs.insert(Symbol::from("id"), Ty::Int);
    attrs.insert(Symbol::from("title"), Ty::Str);

    let post_model = Model {
        name: ClassId(Symbol::from("Post")),
        parent: None,
        parent_span: Default::default(),
        table: TableRef(Symbol::from("posts")),
        primary_key: None,
        attributes: Row { fields: attrs.into(), rest: None },
        body: vec![],
        enums: Default::default(),
        enum_defaults: Default::default(),
        class_attr_defaults: Default::default(),
        lexical_json_shadow: false,
        sti_subclass_names: Vec::new(),
        select_aliases: Vec::new(),
        i18n: Default::default(),
        span: Span::synthetic(),
    };

    // Action body: `Post.all` — a Send to a class-level method.
    let recv = Expr::new(sp(), ExprNode::Const { path: vec![Symbol::from("Post")] });
    let action_body = Expr::new(
        sp(),
        ExprNode::Send {
            recv: Some(recv),
            method: Symbol::from("all"),
            args: vec![],
            block: None,
            parenthesized: false,
        },
    );

    let index_action = Action {
        name_span: roundhouse::span::Span::synthetic(),
        name: Symbol::from("index"),
        params: Row::closed(),
        opt_params: vec![],
        kw_params: vec![],
        kwrest_param: None,
        rest_param: None,
        anonymous_formal: None,
        block_param: None,
        body: action_body,
        renders: RenderTarget::Inferred,
        effects: EffectSet::singleton(Effect::DbRead {
            table: TableRef(Symbol::from("posts")),
        }),
    };

    let posts_controller = Controller {
        name: ClassId(Symbol::from("PostsController")),
        parent: Some(ClassId(Symbol::from("ApplicationController"))),
        parent_span: Default::default(),
        body: vec![roundhouse::ControllerBodyItem::Action {
            action: index_action,
            leading_comments: vec![],
            leading_blank_line: false,
        }],
        layout: Default::default(),
        sibling_classes: Vec::new(),
    };

    let routes = RouteTable {
        entries: vec![roundhouse::RouteSpec::Explicit {
            method: HttpMethod::Get,
            path: "/posts".into(),
            controller: ClassId(Symbol::from("PostsController")),
            action: Symbol::from("index"),
            as_name: Some(Symbol::from("posts")),
            constraints: IndexMap::new(),
            scope: roundhouse::ResourceScope::Nested,
            action_symbol: false,
        }],
        direct_helpers: vec![],
        redirects: vec![],
        diagnostics: vec![],
    };

    let app = App {
        schema_version: App::SCHEMA_VERSION,
        schema,
        // Passthrough-only and `serde(skip)` — it never round-trips, so
        // the roundtrip fixture has nothing to say about it.
        binary_assets: Vec::new(),
        i18n: Default::default(),
        i18n_table: Default::default(),
        i18n_runtime_sites: Default::default(),
        i18n_table_sites: Default::default(),
        inferred_method_params: Default::default(),
        models: vec![post_model],
        library_classes: vec![],
        native_library_keywords: false,
        file_programs: vec![],
        boot_gem_entries: vec![],
        current_attribute_classes: vec![],
        initializer_filters: Vec::new(),
        initializer_defs: Vec::new(),
        gem_package_settings: Vec::new(),
        auto_source_gems: Vec::new(),
        initializer_response_headers: Vec::new(),
        collection_cache_store: None,
        app_requires: Vec::new(),
        sql_functions: Vec::new(),
        database_pragmas: Vec::new(),
        databases: Vec::new(),
        model_databases: Default::default(),
        database_refusals: Vec::new(),
        ingest_drops: Vec::new(),
        library_class_files: Vec::new(),
        rbs_includes: Default::default(),
        controllers: vec![posts_controller],
        routes,
        views: vec![],
        test_modules: vec![],
        fixtures: vec![],
        seeds: None,
        importmap: None,
        inertia: None,
        vite_manifest: None,
        stylesheets: vec![],
        rbs_signatures: roundhouse::det::HashMap::default(),
        gem_lock: None,
        loaded_frameworks: None,
        assume_ssl_envs: Vec::new(),
        gem_boundary: Default::default(),
        extensions: Default::default(),
        content_helper_allowed_attributes: Vec::new(),
        content_helper_allowed_tags: Vec::new(),
        helper_method_index: roundhouse::det::HashMap::default(),
        view_visible_controller_methods: std::collections::BTreeSet::new(),
        global_id_locate_models: std::collections::BTreeSet::new(),
        global_id_locate_signed_models: std::collections::BTreeSet::new(),
        unreachable_views: std::collections::BTreeSet::new(),
        helper_diagnostics: None,
        helper_modules: Default::default(),
        job_plans: Vec::new(),
        queue_adapter: None,
        sidekiq_redis: Default::default(),
        sidekiq_worker: Default::default(),
        cable_redis: None,
        attachable_unsigned_models: Vec::new(),
        pending_attachment_on_load: Vec::new(),
        load_hook_class_macros: Vec::new(),
        load_hook_extended_macros: Vec::new(),
        spliced_class_methods: Default::default(),
        partial_local_types: roundhouse::det::HashMap::default(),
        view_ivar_types: roundhouse::det::HashMap::default(),
        html_safe_methods: std::collections::BTreeSet::new(),
        html_safe_method_owners: Default::default(),
        time_formats: std::collections::BTreeMap::new(),
        generated_helper_methods: std::collections::BTreeMap::new(),
        module_mixins: Vec::new(),
        rake_tasks_source: None,
        rake_task_gaps: Vec::new(),
        rails_application: None,
        concern_filters: roundhouse::det::HashMap::default(),
        graphql_types: Vec::new(),
        graphql_signatures: Vec::new(),
        concern_spliced_actions: roundhouse::det::HashMap::default(),
        concern_spliced_class_methods: roundhouse::det::HashMap::default(),
        concern_model_items: roundhouse::det::HashMap::default(),
        concern_guarded_items: roundhouse::det::HashMap::default(),
        concern_included_redefs: roundhouse::det::HashMap::default(),
        validates_with_guards: Vec::new(),
        render_edges: roundhouse::det::HashMap::default(),
        view_feeders: roundhouse::det::HashMap::default(),
        controller_resolutions: roundhouse::det::HashMap::default(),
        sources: vec![],
        wrap_parameters_by_default: false,
        ports_active_model_validator: false,
        each_validator_calls: Vec::new(),
        // Derived from `sources` and `serde(skip)`, like `binary_assets`.
        const_resolver: Default::default(),
        source_index_required: false,
        root: String::new(),
        app_roots: vec!["app".to_string()],
        spinel_int_overflow: None,
    };

    let json = serde_json::to_string_pretty(&app).expect("serialize");
    let back: App = serde_json::from_str(&json).expect("deserialize");

    assert_eq!(app, back, "IR must survive JSON round-trip");
}

#[test]
fn literals_round_trip() {
    let lits = vec![
        Literal::Nil,
        Literal::Bool { value: true },
        Literal::Int { value: 42 },
        Literal::Str { value: "hello".into() },
        Literal::Sym { value: Symbol::from("name") },
    ];
    for lit in lits {
        let json = serde_json::to_string(&lit).unwrap();
        let back: Literal = serde_json::from_str(&json).unwrap();
        assert_eq!(lit, back);
    }
}
