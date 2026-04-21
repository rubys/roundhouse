//! Emitter smoke test: the tiny-blog IR produces the expected Ruby files.

use std::path::PathBuf;

use indexmap::IndexMap;
use roundhouse::emit::ruby;
use roundhouse::span::Span;
use roundhouse::{
    Action, App, ClassId, Column, ColumnType, Controller, Effect, EffectSet, Expr, ExprNode,
    HttpMethod, Model, RenderTarget, Route, RouteTable, Row, Schema, Symbol, Table, TableRef,
    Ty,
};

fn sp() -> Span { Span::synthetic() }

fn tiny_blog() -> App {
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

    let recv = Expr::new(sp(), ExprNode::Const { path: vec![Symbol::from("Post")] });
    let body = Expr::new(
        sp(),
        ExprNode::Send {
            recv: Some(recv),
            method: Symbol::from("all"),
            args: vec![],
            block: None,
            parenthesized: false,
        },
    );

    let controller = Controller {
        name: ClassId(Symbol::from("PostsController")),
        parent: Some(ClassId(Symbol::from("ApplicationController"))),
        body: vec![roundhouse::ControllerBodyItem::Action {
            action: Action {
                name: Symbol::from("index"),
                params: Row::closed(),
                body,
                renders: RenderTarget::Inferred,
                effects: EffectSet::singleton(Effect::DbRead {
                    table: TableRef(Symbol::from("posts")),
                }),
            },
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

    App {
        schema_version: App::SCHEMA_VERSION,
        schema,
        models: vec![post_model],
        controllers: vec![controller],
        routes,
        views: vec![],
        test_modules: vec![],
        fixtures: vec![],
        seeds: None,
        importmap: None,
        stylesheets: vec![],
    }
}

fn find<'a>(files: &'a [roundhouse::emit::EmittedFile], p: &str) -> &'a str {
    files
        .iter()
        .find(|f| f.path == PathBuf::from(p))
        .unwrap_or_else(|| panic!("no file at {p}; got {:?}", files.iter().map(|f| &f.path).collect::<Vec<_>>()))
        .content
        .as_str()
}

#[test]
fn emits_expected_files() {
    let files = ruby::emit(&tiny_blog());
    let paths: Vec<_> = files.iter().map(|f| f.path.display().to_string()).collect();
    assert!(paths.contains(&"db/schema.rb".to_string()), "paths: {paths:?}");
    assert!(paths.contains(&"app/models/post.rb".to_string()), "paths: {paths:?}");
    assert!(
        paths.contains(&"app/controllers/posts_controller.rb".to_string()),
        "paths: {paths:?}"
    );
    assert!(paths.contains(&"config/routes.rb".to_string()), "paths: {paths:?}");
}

#[test]
fn model_is_empty_class() {
    let files = ruby::emit(&tiny_blog());
    let content = find(&files, "app/models/post.rb");
    assert_eq!(content, "class Post < ApplicationRecord\nend\n");
}

#[test]
fn controller_has_index_action() {
    let files = ruby::emit(&tiny_blog());
    let content = find(&files, "app/controllers/posts_controller.rb");
    let expected = "\
class PostsController < ApplicationController
  def index
    Post.all
  end
end
";
    assert_eq!(content, expected);
}

#[test]
fn routes_file_is_idiomatic() {
    let files = ruby::emit(&tiny_blog());
    let content = find(&files, "config/routes.rb");
    let expected = "\
Rails.application.routes.draw do
  get \"/posts\", to: \"posts#index\", as: :posts
end
";
    assert_eq!(content, expected);
}

#[test]
fn emits_root_and_nested_resources() {
    use roundhouse::{RouteSpec, RouteTable};

    let routes = RouteTable {
        entries: vec![
            RouteSpec::Root { target: "articles#index".into() },
            RouteSpec::Resources {
                name: Symbol::from("articles"),
                only: vec![],
                except: vec![],
                nested: vec![RouteSpec::Resources {
                    name: Symbol::from("comments"),
                    only: vec![Symbol::from("create"), Symbol::from("destroy")],
                    except: vec![],
                    nested: vec![],
                }],
            },
        ],
    };
    let mut app = tiny_blog();
    app.routes = routes;

    let files = ruby::emit(&app);
    let content = find(&files, "config/routes.rb");
    let expected = "\
Rails.application.routes.draw do
  root \"articles#index\"

  resources :articles do
    resources :comments, only: [:create, :destroy]
  end
end
";
    assert_eq!(content, expected);
}

#[test]
fn schema_has_title_column() {
    let files = ruby::emit(&tiny_blog());
    let content = find(&files, "db/schema.rb");
    assert!(content.contains("create_table \"posts\""), "got:\n{content}");
    assert!(content.contains("t.string \"title\""), "got:\n{content}");
    assert!(content.contains("null: false"), "got:\n{content}");
}
