//! Round-trip test: IR → JSON → IR preserves semantics.
//!
//! The forcing function for IR completeness. If constructing an App by hand
//! and round-tripping through JSON loses information, the shape is wrong.
//! Extend this test as the IR grows — every new node kind gets exercised here.

use indexmap::IndexMap;
use roundhouse::{
    Action, App, ClassId, Column, ColumnType, Controller, Effect, EffectSet, Expr, ExprNode,
    HttpMethod, Literal, Model, RenderTarget, Route, RouteTable, Row, Schema, Symbol, Table,
    TableRef, Ty,
};
use roundhouse::span::Span;

fn sp() -> Span {
    Span::synthetic()
}

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
                },
                Column {
                    name: Symbol::from("title"),
                    col_type: ColumnType::String { limit: None },
                    nullable: false,
                    default: None,
                    primary_key: false,
                },
            ],
            indexes: vec![],
            foreign_keys: vec![],
        },
    );
    let schema = Schema { tables };

    let mut attrs = IndexMap::new();
    attrs.insert(Symbol::from("id"), Ty::Int);
    attrs.insert(Symbol::from("title"), Ty::Str);

    let post_model = Model {
        name: ClassId(Symbol::from("Post")),
        parent: None,
        table: TableRef(Symbol::from("posts")),
        attributes: Row { fields: attrs, rest: None },
        body: vec![],
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
        name: Symbol::from("index"),
        params: Row::closed(),
        body: action_body,
        renders: RenderTarget::Inferred,
        effects: EffectSet::singleton(Effect::DbRead {
            table: TableRef(Symbol::from("posts")),
        }),
    };

    let posts_controller = Controller {
        name: ClassId(Symbol::from("PostsController")),
        parent: Some(ClassId(Symbol::from("ApplicationController"))),
        body: vec![roundhouse::ControllerBodyItem::Action {
            action: index_action,
            leading_comments: vec![],
            leading_blank_line: false,
        }],
    };

    let routes = RouteTable {
        entries: vec![roundhouse::RouteSpec::Explicit {
            method: HttpMethod::Get,
            path: "/posts".into(),
            controller: ClassId(Symbol::from("PostsController")),
            action: Symbol::from("index"),
            as_name: Some(Symbol::from("posts")),
            constraints: IndexMap::new(),
        }],
    };

    let app = App {
        schema_version: App::SCHEMA_VERSION,
        schema,
        models: vec![post_model],
        controllers: vec![posts_controller],
        routes,
        views: vec![],
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
