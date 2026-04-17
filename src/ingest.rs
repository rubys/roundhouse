//! Prism → Roundhouse IR.
//!
//! Reads Ruby source (a single file or a Rails app directory) and produces an
//! [`App`]. This is the reverse of [`crate::emit::ruby`]; together they form
//! the round-trip forcing function.
//!
//! Scope for the initial landing is the tiny-blog fixture: a single model, a
//! single controller with one action, a trivial routes file, and a schema.
//! The ingester deliberately panics on unrecognized constructs — a failed
//! ingest is a signal that the IR (or the recognizer) needs to grow.

use std::path::Path;

use indexmap::IndexMap;
use ruby_prism::{Node, parse};

use crate::dialect::{
    Action, Controller, HttpMethod, Model, RenderTarget, RouteSpec, RouteTable, View,
};
use crate::erb;
use crate::effect::EffectSet;
use crate::expr::{BoolOpKind, BoolOpSurface, Expr, ExprNode, InterpPart, Literal};
use crate::schema::{Column, ColumnType, Schema, Table};
use crate::span::Span;
use crate::ty::{Row, Ty};
use crate::{App, ClassId, Symbol, TableRef};

// Errors ----------------------------------------------------------------

#[derive(Debug)]
pub enum IngestError {
    Io(std::io::Error),
    Parse { file: String, message: String },
    Unsupported { file: String, message: String },
}

impl std::fmt::Display for IngestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "io error: {e}"),
            Self::Parse { file, message } => write!(f, "parse error in {file}: {message}"),
            Self::Unsupported { file, message } => {
                write!(f, "unsupported construct in {file}: {message}")
            }
        }
    }
}

impl std::error::Error for IngestError {}

impl From<std::io::Error> for IngestError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

type IngestResult<T> = Result<T, IngestError>;

// Entry points -----------------------------------------------------------

/// Ingest an entire Rails app directory.
pub fn ingest_app(dir: &Path) -> IngestResult<App> {
    let mut app = App::new();

    let schema_path = dir.join("db/schema.rb");
    if schema_path.exists() {
        let source = std::fs::read(&schema_path)?;
        app.schema = ingest_schema(&source, &schema_path.display().to_string())?;
    }

    let models_dir = dir.join("app/models");
    if models_dir.is_dir() {
        for entry in read_rb_files(&models_dir)? {
            let source = std::fs::read(&entry)?;
            if let Some(model) = ingest_model(&source, &entry.display().to_string(), &app.schema)?
            {
                app.models.push(model);
            }
        }
    }

    let controllers_dir = dir.join("app/controllers");
    if controllers_dir.is_dir() {
        for entry in read_rb_files(&controllers_dir)? {
            let source = std::fs::read(&entry)?;
            if let Some(controller) = ingest_controller(&source, &entry.display().to_string())? {
                app.controllers.push(controller);
            }
        }
    }

    let routes_path = dir.join("config/routes.rb");
    if routes_path.exists() {
        let source = std::fs::read(&routes_path)?;
        app.routes = ingest_routes(&source, &routes_path.display().to_string())?;
    }

    let views_dir = dir.join("app/views");
    if views_dir.is_dir() {
        let erb_files = read_erb_files(&views_dir)?;
        for erb_path in erb_files {
            let source = std::fs::read_to_string(&erb_path)?;
            let rel = erb_path
                .strip_prefix(&views_dir)
                .map_err(|_| IngestError::Unsupported {
                    file: erb_path.display().to_string(),
                    message: "view path outside views dir".into(),
                })?;
            let view = ingest_view(&source, rel, &erb_path.display().to_string())?;
            app.views.push(view);
        }
    }

    Ok(app)
}

fn read_erb_files(dir: &Path) -> IngestResult<Vec<std::path::PathBuf>> {
    let mut out = Vec::new();
    walk_erb(dir, &mut out)?;
    out.sort();
    Ok(out)
}

fn walk_erb(dir: &Path, out: &mut Vec<std::path::PathBuf>) -> IngestResult<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            walk_erb(&path, out)?;
        } else if path.extension().and_then(|e| e.to_str()) == Some("erb") {
            out.push(path);
        }
    }
    Ok(())
}

/// Ingest a single `.erb` template. The path-extension shape
/// `posts/index.html.erb` yields name=`posts/index`, format=`html`.
pub fn ingest_view(source: &str, rel_path: &Path, file: &str) -> IngestResult<View> {
    let path_str = rel_path.to_string_lossy();
    let no_erb = path_str.strip_suffix(".erb").unwrap_or(&path_str);
    let (name, format) = match no_erb.rsplit_once('.') {
        Some((stem, fmt)) => (stem.to_string(), fmt.to_string()),
        None => (no_erb.to_string(), "html".to_string()),
    };

    // Compile ERB to Ruby, then ingest the compiled Ruby through our
    // existing pipeline. The resulting View body is a `Seq` of `_buf`
    // operations the emitter pattern-matches back to template form.
    let compiled = erb::compile_erb(source);
    let body = ingest_ruby_program(&compiled, file)?;

    Ok(View {
        name: Symbol::from(name),
        format: Symbol::from(format),
        locals: Row::closed(),
        body,
    })
}

/// Parse a Ruby source program (possibly multiple top-level statements)
/// and return the resulting `Expr`. Used by the ERB ingester; generalized
/// so future multi-statement sources can share it.
fn ingest_ruby_program(source: &str, file: &str) -> IngestResult<Expr> {
    let result = parse(source.as_bytes());
    let root = result.node();
    let program = root.as_program_node().ok_or_else(|| IngestError::Parse {
        file: file.into(),
        message: "compiled Ruby is not a program".into(),
    })?;
    let stmts = program.statements();
    ingest_expr(&stmts.as_node(), file)
}

fn read_rb_files(dir: &Path) -> IngestResult<Vec<std::path::PathBuf>> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("rb") {
            out.push(path);
        }
    }
    out.sort();
    Ok(out)
}

// Models -----------------------------------------------------------------

/// Parse a single model file. The first class definition is treated as the
/// model; any schema-derived attributes are filled in from `schema`.
pub fn ingest_model(
    source: &[u8],
    file: &str,
    schema: &Schema,
) -> IngestResult<Option<Model>> {
    let result = parse(source);
    let root = result.node();
    let Some(class) = find_first_class(&root) else {
        return Ok(None);
    };

    let name_path = class_name_path(&class).ok_or_else(|| IngestError::Unsupported {
        file: file.into(),
        message: "model class name must be a simple constant or path".into(),
    })?;
    let class_name = Symbol::from(name_path.join("::"));
    let owner = ClassId(class_name.clone());
    let table_name = pluralize_snake(class_name.as_str());

    let attributes = if let Some(table) = schema.tables.get(&Symbol::from(table_name.as_str())) {
        row_from_table(table)
    } else {
        Row::closed()
    };

    let mut associations = Vec::new();
    let mut validations = Vec::new();
    let mut scopes = Vec::new();
    let mut callbacks = Vec::new();
    let mut methods = Vec::new();
    if let Some(body) = class.body() {
        for stmt in flatten_statements(body) {
            if let Some(call) = stmt.as_call_node() {
                if call.receiver().is_some() {
                    continue;
                }
                let method = constant_id_str(&call.name()).to_string();
                if let Some(assoc) = parse_association(&call, &owner, &method) {
                    associations.push(assoc);
                } else if method == "validates" {
                    validations.extend(parse_validates(&call));
                } else if method == "scope" {
                    if let Some(scope) = parse_scope(&call, file)? {
                        scopes.push(scope);
                    }
                } else if let Some(cb) = parse_callback(&call, &method) {
                    callbacks.push(cb);
                }
            } else if let Some(def) = stmt.as_def_node() {
                methods.push(ingest_method(&def, file)?);
            }
        }
    }

    Ok(Some(Model {
        name: owner,
        table: TableRef(Symbol::from(table_name)),
        attributes,
        associations,
        validations,
        scopes,
        callbacks,
        methods,
    }))
}

fn ingest_method(
    def: &ruby_prism::DefNode<'_>,
    file: &str,
) -> IngestResult<crate::dialect::MethodDef> {
    use crate::dialect::{MethodDef, MethodReceiver};

    let name = Symbol::from(constant_id_str(&def.name()));
    // `def self.foo` / `def Post.foo` have explicit receivers; plain `def foo`
    // is an instance method.
    let receiver = if def.receiver().is_some() {
        MethodReceiver::Class
    } else {
        MethodReceiver::Instance
    };

    // Parameter-list parsing beyond "no params" lands when a fixture uses
    // optional/keyword/rest params. Record an empty list for now; Prism's
    // ParametersNode has the detail we need when we need it.
    let params: Vec<Symbol> = Vec::new();

    let body = match def.body() {
        Some(b) => ingest_expr(&b, file)?,
        None => Expr::new(Span::synthetic(), ExprNode::Seq { exprs: vec![] }),
    };

    Ok(MethodDef {
        name,
        receiver,
        params,
        body,
        signature: None,
        effects: EffectSet::pure(),
    })
}

fn parse_callback(
    call: &ruby_prism::CallNode<'_>,
    method: &str,
) -> Option<crate::dialect::Callback> {
    use crate::dialect::{Callback, CallbackHook};

    let hook = match method {
        "before_validation" => CallbackHook::BeforeValidation,
        "after_validation" => CallbackHook::AfterValidation,
        "before_save" => CallbackHook::BeforeSave,
        "after_save" => CallbackHook::AfterSave,
        "before_create" => CallbackHook::BeforeCreate,
        "after_create" => CallbackHook::AfterCreate,
        "before_update" => CallbackHook::BeforeUpdate,
        "after_update" => CallbackHook::AfterUpdate,
        "before_destroy" => CallbackHook::BeforeDestroy,
        "after_destroy" => CallbackHook::AfterDestroy,
        "after_commit" => CallbackHook::AfterCommit,
        "after_rollback" => CallbackHook::AfterRollback,
        _ => return None,
    };

    let args = call.arguments()?;
    let all_args = args.arguments();
    let mut iter = all_args.iter();
    let first = iter.next()?;
    let target = Symbol::from(symbol_value(&first)?.as_str());

    // `if:` / `unless:` conditions land when a fixture demands them.
    let condition = None;

    Some(Callback { hook, target, condition })
}

fn parse_scope(
    call: &ruby_prism::CallNode<'_>,
    file: &str,
) -> IngestResult<Option<crate::dialect::Scope>> {
    use crate::dialect::Scope;

    let Some(args) = call.arguments() else { return Ok(None) };
    let all_args = args.arguments();
    let mut iter = all_args.iter();

    let Some(name_node) = iter.next() else { return Ok(None) };
    let Some(name_str) = symbol_value(&name_node) else { return Ok(None) };
    let name = Symbol::from(name_str.as_str());

    let Some(body_node) = iter.next() else { return Ok(None) };
    let Some(lambda) = body_node.as_lambda_node() else {
        return Err(IngestError::Unsupported {
            file: file.into(),
            message: format!("scope :{name} body must be a lambda (`-> {{ ... }}`)"),
        });
    };

    // Parameter parsing lands when a fixture needs parameterized scopes.
    let params: Vec<Symbol> = vec![];

    let body = match lambda.body() {
        Some(b) => ingest_expr(&b, file)?,
        None => Expr::new(Span::synthetic(), ExprNode::Seq { exprs: vec![] }),
    };

    Ok(Some(Scope { name, params, body }))
}

fn parse_validates(call: &ruby_prism::CallNode<'_>) -> Vec<crate::dialect::Validation> {
    use crate::dialect::{Validation, ValidationRule};
    let Some(args) = call.arguments() else { return vec![] };
    let all_args = args.arguments();

    let mut attrs: Vec<Symbol> = Vec::new();
    let mut rules: Vec<ValidationRule> = Vec::new();

    for arg in all_args.iter() {
        if let Some(sym) = symbol_value(&arg) {
            attrs.push(Symbol::from(sym.as_str()));
        } else if let Some(kh) = arg.as_keyword_hash_node() {
            for el in kh.elements().iter() {
                let Some(assoc) = el.as_assoc_node() else { continue };
                let Some(key) = symbol_value(&assoc.key()) else { continue };
                let value = assoc.value();
                if let Some(rule) = validation_rule_from_kv(&key, &value) {
                    rules.push(rule);
                }
            }
        }
    }

    let mut out = Vec::new();
    for attr in attrs {
        out.push(Validation { attribute: attr, rules: rules.clone() });
    }
    out
}

fn validation_rule_from_kv(
    key: &str,
    value: &ruby_prism::Node<'_>,
) -> Option<crate::dialect::ValidationRule> {
    use crate::dialect::ValidationRule;
    match key {
        "presence" => bool_value(value).filter(|b| *b).map(|_| ValidationRule::Presence),
        "absence" => bool_value(value).filter(|b| *b).map(|_| ValidationRule::Absence),
        _ => None,
    }
}

fn parse_association(
    call: &ruby_prism::CallNode<'_>,
    owner: &ClassId,
    method: &str,
) -> Option<crate::dialect::Association> {
    use crate::dialect::{Association, Dependent};

    let args = call.arguments()?;
    let all_args = args.arguments();
    let mut iter = all_args.iter();
    let first = iter.next()?;
    let name_str = symbol_value(&first)?;
    let name = Symbol::from(name_str.as_str());

    let mut class_name: Option<String> = None;
    let mut foreign_key: Option<String> = None;
    let mut through: Option<String> = None;
    let mut dependent: Option<Dependent> = None;
    let mut optional: Option<bool> = None;
    let mut join_table: Option<String> = None;

    for arg in iter {
        let Some(kh) = arg.as_keyword_hash_node() else { continue };
        for el in kh.elements().iter() {
            let Some(assoc) = el.as_assoc_node() else { continue };
            let Some(key) = symbol_value(&assoc.key()) else { continue };
            let value = assoc.value();
            match key.as_str() {
                "class_name" => class_name = string_value(&value),
                "foreign_key" => {
                    foreign_key = string_value(&value).or_else(|| symbol_value(&value))
                }
                "through" => through = symbol_value(&value),
                "dependent" => {
                    dependent = symbol_value(&value).and_then(|s| dependent_from_sym(&s))
                }
                "optional" => optional = bool_value(&value),
                "join_table" => join_table = string_value(&value),
                _ => {}
            }
        }
    }

    let owner_snake = snake_case(owner.0.as_str());

    match method {
        "has_many" => Some(Association::HasMany {
            name: name.clone(),
            target: class_name
                .map(|s| ClassId(Symbol::from(s.as_str())))
                .unwrap_or_else(|| ClassId(Symbol::from(singularize_camelize(name_str.as_str())))),
            foreign_key: foreign_key
                .map(|s| Symbol::from(s.as_str()))
                .unwrap_or_else(|| Symbol::from(format!("{owner_snake}_id"))),
            through: through.map(|s| Symbol::from(s.as_str())),
            dependent: dependent.unwrap_or_default(),
        }),
        "has_one" => Some(Association::HasOne {
            name: name.clone(),
            target: class_name
                .map(|s| ClassId(Symbol::from(s.as_str())))
                .unwrap_or_else(|| ClassId(Symbol::from(camelize(name_str.as_str())))),
            foreign_key: foreign_key
                .map(|s| Symbol::from(s.as_str()))
                .unwrap_or_else(|| Symbol::from(format!("{owner_snake}_id"))),
            dependent: dependent.unwrap_or_default(),
        }),
        "belongs_to" => Some(Association::BelongsTo {
            name: name.clone(),
            target: class_name
                .map(|s| ClassId(Symbol::from(s.as_str())))
                .unwrap_or_else(|| ClassId(Symbol::from(camelize(name_str.as_str())))),
            foreign_key: foreign_key
                .map(|s| Symbol::from(s.as_str()))
                .unwrap_or_else(|| Symbol::from(format!("{name_str}_id"))),
            optional: optional.unwrap_or(false),
        }),
        "has_and_belongs_to_many" => Some(Association::HasAndBelongsToMany {
            name: name.clone(),
            target: class_name
                .map(|s| ClassId(Symbol::from(s.as_str())))
                .unwrap_or_else(|| ClassId(Symbol::from(singularize_camelize(name_str.as_str())))),
            join_table: join_table
                .map(|s| Symbol::from(s.as_str()))
                .unwrap_or_else(|| Symbol::from(default_habtm_table(owner, name_str.as_str()))),
        }),
        _ => None,
    }
}

fn dependent_from_sym(s: &str) -> Option<crate::dialect::Dependent> {
    use crate::dialect::Dependent;
    Some(match s {
        "destroy" => Dependent::Destroy,
        "destroy_async" => Dependent::DestroyAsync,
        "delete" => Dependent::Delete,
        "delete_all" => Dependent::DeleteAll,
        "nullify" => Dependent::Nullify,
        "restrict_with_exception" | "restrict_with_error" => Dependent::Restrict,
        _ => return None,
    })
}

fn default_habtm_table(owner: &ClassId, target_plural_sym: &str) -> String {
    crate::naming::habtm_join_table(owner.0.as_str(), target_plural_sym)
}

fn row_from_table(table: &Table) -> Row {
    let mut fields = IndexMap::new();
    for col in &table.columns {
        fields.insert(col.name.clone(), ty_of_column(&col.col_type));
    }
    Row { fields, rest: None }
}

fn ty_of_column(t: &ColumnType) -> Ty {
    match t {
        ColumnType::Integer | ColumnType::BigInt => Ty::Int,
        ColumnType::Float | ColumnType::Decimal { .. } => Ty::Float,
        ColumnType::String { .. } | ColumnType::Text => Ty::Str,
        ColumnType::Boolean => Ty::Bool,
        ColumnType::Date | ColumnType::DateTime | ColumnType::Time => {
            Ty::Class { id: ClassId(Symbol::from("Time")), args: vec![] }
        }
        ColumnType::Binary => Ty::Str,
        ColumnType::Json => Ty::Hash { key: Box::new(Ty::Str), value: Box::new(Ty::Str) },
        ColumnType::Reference { .. } => Ty::Int,
    }
}

// Controllers -----------------------------------------------------------

pub fn ingest_controller(source: &[u8], file: &str) -> IngestResult<Option<Controller>> {
    let result = parse(source);
    let root = result.node();
    let Some(class) = find_first_class(&root) else {
        return Ok(None);
    };

    let name_path = class_name_path(&class).ok_or_else(|| IngestError::Unsupported {
        file: file.into(),
        message: "controller class name must be a simple constant or path".into(),
    })?;

    let parent = class.superclass().and_then(|n| {
        constant_path_of(&n).map(|p| ClassId(Symbol::from(p.join("::"))))
    });

    let mut actions = Vec::new();
    let mut filters = Vec::new();
    if let Some(body) = class.body() {
        for stmt in flatten_statements(body) {
            if let Some(def) = stmt.as_def_node() {
                let action_name = constant_id_str(&def.name()).to_string();
                let body_expr = match def.body() {
                    Some(b) => ingest_expr(&b, file)?,
                    None => Expr::new(Span::synthetic(), ExprNode::Seq { exprs: vec![] }),
                };
                actions.push(Action {
                    name: Symbol::from(action_name),
                    params: Row::closed(),
                    body: body_expr,
                    renders: RenderTarget::Inferred,
                    effects: EffectSet::pure(),
                });
            } else if let Some(call) = stmt.as_call_node() {
                if call.receiver().is_none() {
                    let method = constant_id_str(&call.name()).to_string();
                    if let Some(filter) = parse_filter(&call, &method) {
                        filters.push(filter);
                    }
                }
            }
        }
    }

    Ok(Some(Controller {
        name: ClassId(Symbol::from(name_path.join("::"))),
        parent,
        filters,
        actions,
    }))
}

fn parse_filter(
    call: &ruby_prism::CallNode<'_>,
    method: &str,
) -> Option<crate::dialect::Filter> {
    use crate::dialect::{Filter, FilterKind};

    let kind = match method {
        "before_action" => FilterKind::Before,
        "around_action" => FilterKind::Around,
        "after_action" => FilterKind::After,
        "skip_before_action" => FilterKind::Skip,
        _ => return None,
    };

    let args = call.arguments()?;
    let all_args = args.arguments();
    let mut iter = all_args.iter();
    let first = iter.next()?;
    let target = Symbol::from(symbol_value(&first)?.as_str());

    let mut only: Vec<Symbol> = Vec::new();
    let mut except: Vec<Symbol> = Vec::new();

    for arg in iter {
        let Some(kh) = arg.as_keyword_hash_node() else { continue };
        for el in kh.elements().iter() {
            let Some(assoc) = el.as_assoc_node() else { continue };
            let Some(key) = symbol_value(&assoc.key()) else { continue };
            let value = assoc.value();
            match key.as_str() {
                "only" => only = symbol_list_value(&value),
                "except" => except = symbol_list_value(&value),
                _ => {}
            }
        }
    }

    Some(Filter { kind, target, only, except })
}

fn symbol_list_value(node: &ruby_prism::Node<'_>) -> Vec<Symbol> {
    if let Some(arr) = node.as_array_node() {
        return arr
            .elements()
            .iter()
            .filter_map(|n| symbol_value(&n))
            .map(|s| Symbol::from(s.as_str()))
            .collect();
    }
    if let Some(s) = symbol_value(node) {
        return vec![Symbol::from(s.as_str())];
    }
    vec![]
}

// Routes ----------------------------------------------------------------

pub fn ingest_routes(source: &[u8], file: &str) -> IngestResult<RouteTable> {
    let result = parse(source);
    let root = result.node();

    // Find the outer `Rails.application.routes.draw do ... end` call.
    let Some(draw_call) = find_call_named(&root, "draw") else {
        return Ok(RouteTable::default());
    };
    let Some(block_node) = draw_call.block() else {
        return Ok(RouteTable::default());
    };
    let Some(block) = block_node.as_block_node() else {
        return Ok(RouteTable::default());
    };

    let entries = match block.body() {
        Some(body) => ingest_route_body(body, file)?,
        None => Vec::new(),
    };

    Ok(RouteTable { entries })
}

/// Walk the statements inside a `routes.draw do ... end` block (or a
/// nested `resources :x do ... end` block) and collect their `RouteSpec`
/// entries. Recognized forms: verb shortcuts, `root "c#a"`, and
/// `resources :name`.
fn ingest_route_body(body: Node<'_>, file: &str) -> IngestResult<Vec<RouteSpec>> {
    let mut entries = Vec::new();
    for stmt in flatten_statements(body) {
        let Some(call) = stmt.as_call_node() else { continue };
        if call.receiver().is_some() {
            // `Rails.application.routes.draw` gets re-found as a nested
            // call when we walk a weird input; skip anything with an
            // explicit receiver here.
            continue;
        }
        let method = constant_id_str(&call.name()).to_string();
        if let Some(spec) = ingest_route_call(&call, &method, file)? {
            entries.push(spec);
        }
    }
    Ok(entries)
}

fn ingest_route_call(
    call: &ruby_prism::CallNode<'_>,
    method: &str,
    file: &str,
) -> IngestResult<Option<RouteSpec>> {
    // Verb shortcuts (`get "/p", to: "c#a"`).
    if let Some(http) = http_method_from(method) {
        return ingest_explicit_route(call, http, file).map(Some);
    }
    match method {
        "root" => ingest_root_route(call).map(Some),
        "resources" => ingest_resources_route(call, file).map(Some),
        // Unknown DSL — `resource` (singular), `namespace`, `scope`,
        // `concern`, `mount`, etc. land here. Fail loud so the fixture
        // that introduces them forces a recognizer.
        _ => Err(IngestError::Unsupported {
            file: file.into(),
            message: format!("unsupported routes DSL: `{method}`"),
        }),
    }
}

fn http_method_from(name: &str) -> Option<HttpMethod> {
    Some(match name {
        "get" => HttpMethod::Get,
        "post" => HttpMethod::Post,
        "put" => HttpMethod::Put,
        "patch" => HttpMethod::Patch,
        "delete" => HttpMethod::Delete,
        "head" => HttpMethod::Head,
        "options" => HttpMethod::Options,
        "match" => HttpMethod::Any,
        _ => return None,
    })
}

fn ingest_explicit_route(
    call: &ruby_prism::CallNode<'_>,
    method: HttpMethod,
    file: &str,
) -> IngestResult<RouteSpec> {
    let Some(args_node) = call.arguments() else {
        return Err(IngestError::Unsupported {
            file: file.into(),
            message: "verb route without arguments".into(),
        });
    };
    let mut path: Option<String> = None;
    let mut to: Option<String> = None;
    let mut as_name: Option<Symbol> = None;
    let mut constraints: IndexMap<Symbol, String> = IndexMap::new();

    for arg in args_node.arguments().iter() {
        if let Some(s) = string_value(&arg) {
            if path.is_none() {
                path = Some(s);
            }
        } else if let Some(kh) = arg.as_keyword_hash_node() {
            for el in kh.elements().iter() {
                let Some(assoc) = el.as_assoc_node() else { continue };
                let Some(key_sym) = symbol_value(&assoc.key()) else { continue };
                let value = &assoc.value();
                match key_sym.as_str() {
                    "to" => to = string_value(value),
                    "as" => as_name = symbol_value(value).map(Symbol::from),
                    other => {
                        if let Some(v) = string_value(value) {
                            constraints.insert(Symbol::from(other), v);
                        }
                    }
                }
            }
        }
    }

    let (controller, action) = match to.as_deref().and_then(|s| s.split_once('#')) {
        Some((c, a)) => (c.to_string(), a.to_string()),
        None => {
            return Err(IngestError::Unsupported {
                file: file.into(),
                message: "route missing `to: \"controller#action\"`".into(),
            });
        }
    };

    Ok(RouteSpec::Explicit {
        method,
        path: path.unwrap_or_default(),
        controller: ClassId(Symbol::from(controller_class_name(&controller))),
        action: Symbol::from(action),
        as_name,
        constraints,
    })
}

fn ingest_root_route(call: &ruby_prism::CallNode<'_>) -> IngestResult<RouteSpec> {
    // `root "c#a"` — exactly one string arg. Keyword forms
    // (`root to: "c#a"`) aren't in any fixture yet; add when needed.
    let target = call
        .arguments()
        .and_then(|a| a.arguments().iter().next().and_then(|n| string_value(&n)))
        .unwrap_or_default();
    Ok(RouteSpec::Root { target })
}

fn ingest_resources_route(
    call: &ruby_prism::CallNode<'_>,
    file: &str,
) -> IngestResult<RouteSpec> {
    let Some(args_node) = call.arguments() else {
        return Err(IngestError::Unsupported {
            file: file.into(),
            message: "resources call without a name".into(),
        });
    };
    let all_args = args_node.arguments();
    let mut iter = all_args.iter();
    let first = iter.next().ok_or_else(|| IngestError::Unsupported {
        file: file.into(),
        message: "resources call without a name".into(),
    })?;
    let name_str = symbol_value(&first).ok_or_else(|| IngestError::Unsupported {
        file: file.into(),
        message: "resources name must be a symbol".into(),
    })?;
    let name = Symbol::from(name_str.as_str());

    let mut only: Vec<Symbol> = Vec::new();
    let mut except: Vec<Symbol> = Vec::new();
    for arg in iter {
        let Some(kh) = arg.as_keyword_hash_node() else { continue };
        for el in kh.elements().iter() {
            let Some(assoc) = el.as_assoc_node() else { continue };
            let Some(key) = symbol_value(&assoc.key()) else { continue };
            let value = assoc.value();
            match key.as_str() {
                "only" => only = symbol_list_value(&value),
                "except" => except = symbol_list_value(&value),
                // `as:`, `path:`, `controller:`, `shallow:` land when
                // a fixture demands them.
                _ => {}
            }
        }
    }

    let nested = match call.block() {
        Some(block_node) => match block_node.as_block_node() {
            Some(block) => match block.body() {
                Some(body) => ingest_route_body(body, file)?,
                None => Vec::new(),
            },
            None => Vec::new(),
        },
        None => Vec::new(),
    };

    Ok(RouteSpec::Resources { name, only, except, nested })
}

fn controller_class_name(short: &str) -> String {
    let mut s = camelize(short);
    s.push_str("Controller");
    s
}

// Schema ----------------------------------------------------------------

pub fn ingest_schema(source: &[u8], _file: &str) -> IngestResult<Schema> {
    let result = parse(source);
    let root = result.node();

    let mut schema = Schema::default();
    walk_calls(&root, &mut |call| {
        if constant_id_str(&call.name()) != "create_table" {
            return;
        }
        let Some(args) = call.arguments() else { return };
        let first = args.arguments().iter().next();
        let Some(table_name) = first.as_ref().and_then(string_value) else { return };

        // Rails convention: every table has an implicit bigint primary-key `id`
        // unless `id: false` is passed to `create_table`. We honor that here by
        // synthesizing the column; the Ruby emitter's `primary_key` skip keeps
        // schema.rb round-trip-equal to the source.
        let mut has_id = true;
        let create_table_args = args.arguments();
        for arg in create_table_args.iter().skip(1) {
            let Some(kh) = arg.as_keyword_hash_node() else { continue };
            for el in kh.elements().iter() {
                let Some(assoc) = el.as_assoc_node() else { continue };
                let Some(key) = symbol_value(&assoc.key()) else { continue };
                if key.as_str() == "id" {
                    if let Some(false) = bool_value(&assoc.value()) {
                        has_id = false;
                    }
                }
            }
        }

        let mut columns = Vec::new();
        if has_id {
            columns.push(Column {
                name: Symbol::from("id"),
                col_type: ColumnType::BigInt,
                nullable: false,
                default: None,
                primary_key: true,
            });
        }
        if let Some(block_node) = call.block() {
            if let Some(block) = block_node.as_block_node() {
                if let Some(body) = block.body() {
                    for stmt in flatten_statements(body) {
                        if let Some(call) = stmt.as_call_node() {
                            if let Some(col) = column_from_call(&call) {
                                columns.push(col);
                            }
                        }
                    }
                }
            }
        }

        schema.tables.insert(
            Symbol::from(table_name.clone()),
            Table {
                name: Symbol::from(table_name),
                columns,
                indexes: vec![],
                foreign_keys: vec![],
            },
        );
    });

    Ok(schema)
}

fn column_from_call(call: &ruby_prism::CallNode<'_>) -> Option<Column> {
    // Expected: t.string "title", null: false
    // Receiver is a LocalVariableReadNode named "t".
    let recv = call.receiver()?;
    recv.as_local_variable_read_node()?;

    let col_type_name = constant_id_str(&call.name()).to_string();
    let args_node = call.arguments()?;
    let first = args_node.arguments().iter().next()?;
    let col_name = string_value(&first)?;

    let mut nullable = true;
    let mut default: Option<String> = None;
    let mut limit: Option<u32> = None;

    for arg in args_node.arguments().iter().skip(1) {
        if let Some(kh) = arg.as_keyword_hash_node() {
            for el in kh.elements().iter() {
                let Some(assoc) = el.as_assoc_node() else { continue };
                let Some(key) = symbol_value(&assoc.key()) else { continue };
                let value = &assoc.value();
                match key.as_str() {
                    "null" => nullable = bool_value(value).unwrap_or(true),
                    "default" => default = string_value(value),
                    "limit" => {
                        if let Some(n) = integer_value(value) {
                            if n >= 0 {
                                limit = Some(n as u32);
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    let col_type = match col_type_name.as_str() {
        "integer" => ColumnType::Integer,
        "bigint" => ColumnType::BigInt,
        "float" => ColumnType::Float,
        "decimal" => ColumnType::Decimal { precision: None, scale: None },
        "string" => ColumnType::String { limit },
        "text" => ColumnType::Text,
        "boolean" => ColumnType::Boolean,
        "date" => ColumnType::Date,
        "datetime" => ColumnType::DateTime,
        "time" => ColumnType::Time,
        "binary" => ColumnType::Binary,
        "json" => ColumnType::Json,
        "references" => ColumnType::Reference { table: TableRef(Symbol::from(col_name.as_str())) },
        _ => return None,
    };

    Some(Column {
        name: Symbol::from(col_name),
        col_type,
        nullable,
        default,
        primary_key: false,
    })
}

// Expression ingest -----------------------------------------------------

pub fn ingest_expr(node: &Node<'_>, file: &str) -> IngestResult<Expr> {
    let span = Span::synthetic(); // Real spans land when miette is wired in.
    let expr_node = match node {
        n if n.as_constant_read_node().is_some() => {
            let c = n.as_constant_read_node().unwrap();
            ExprNode::Const {
                path: vec![Symbol::from(constant_id_str(&c.name()))],
            }
        }
        n if n.as_constant_path_node().is_some() => {
            let p = n.as_constant_path_node().unwrap();
            ExprNode::Const { path: constant_path_segments(&p) }
        }
        n if n.as_call_node().is_some() => {
            let c = n.as_call_node().unwrap();
            let method = constant_id_str(&c.name()).to_string();
            let args: Vec<Expr> = if let Some(a) = c.arguments() {
                a.arguments()
                    .iter()
                    .map(|arg| ingest_expr(&arg, file))
                    .collect::<IngestResult<_>>()?
            } else {
                vec![]
            };
            let recv = match c.receiver() {
                Some(r) => Some(ingest_expr(&r, file)?),
                None => None,
            };
            let parenthesized = c.opening_loc().is_some();
            let block = match c.block() {
                Some(block_node) => ingest_call_block(&block_node, file)?,
                None => None,
            };
            ExprNode::Send {
                recv,
                method: Symbol::from(method),
                args,
                block,
                parenthesized,
            }
        }
        n if n.as_integer_node().is_some() => {
            let i = n.as_integer_node().unwrap();
            let v: i32 = i.value().try_into().unwrap_or(0);
            ExprNode::Lit { value: Literal::Int { value: v as i64 } }
        }
        n if n.as_string_node().is_some() => {
            let s = n.as_string_node().unwrap();
            let bytes = s.unescaped();
            ExprNode::Lit {
                value: Literal::Str { value: String::from_utf8_lossy(bytes).into_owned() },
            }
        }
        n if n.as_interpolated_string_node().is_some() => {
            let is = n.as_interpolated_string_node().unwrap();
            let mut parts: Vec<InterpPart> = Vec::new();
            for part in is.parts().iter() {
                if let Some(sn) = part.as_string_node() {
                    let bytes = sn.unescaped();
                    parts.push(InterpPart::Text {
                        value: String::from_utf8_lossy(bytes).into_owned(),
                    });
                } else if let Some(es) = part.as_embedded_statements_node() {
                    let stmts = es.statements().ok_or_else(|| IngestError::Unsupported {
                        file: file.into(),
                        message: "empty `#{}` in interpolated string".into(),
                    })?;
                    let inner = ingest_expr(&stmts.as_node(), file)?;
                    parts.push(InterpPart::Expr { expr: inner });
                } else {
                    return Err(IngestError::Unsupported {
                        file: file.into(),
                        message: format!(
                            "unsupported interpolated-string part: {part:?}"
                        ),
                    });
                }
            }
            ExprNode::StringInterp { parts }
        }
        n if n.as_symbol_node().is_some() => {
            ExprNode::Lit { value: Literal::Sym { value: symbol_value(n).unwrap_or_default().into() } }
        }
        n if n.as_true_node().is_some() => ExprNode::Lit { value: Literal::Bool { value: true } },
        n if n.as_false_node().is_some() => ExprNode::Lit { value: Literal::Bool { value: false } },
        n if n.as_nil_node().is_some() => ExprNode::Lit { value: Literal::Nil },
        n if n.as_statements_node().is_some() => {
            let stmts = n.as_statements_node().unwrap();
            let exprs: Vec<Expr> = stmts
                .body()
                .iter()
                .map(|s| ingest_expr(&s, file))
                .collect::<IngestResult<_>>()?;
            if exprs.len() == 1 {
                return Ok(exprs.into_iter().next().unwrap());
            }
            ExprNode::Seq { exprs }
        }
        n if n.as_local_variable_read_node().is_some() => {
            let v = n.as_local_variable_read_node().unwrap();
            ExprNode::Var {
                id: crate::ident::VarId(0),
                name: Symbol::from(constant_id_str(&v.name())),
            }
        }
        n if n.as_instance_variable_read_node().is_some() => {
            let v = n.as_instance_variable_read_node().unwrap();
            let raw = constant_id_str(&v.name());
            let name = raw.strip_prefix('@').unwrap_or(raw);
            ExprNode::Ivar { name: Symbol::from(name) }
        }
        n if n.as_if_node().is_some() => {
            let if_node = n.as_if_node().unwrap();
            let cond = ingest_expr(&if_node.predicate(), file)?;
            let then_branch = match if_node.statements() {
                Some(s) => ingest_expr(&s.as_node(), file)?,
                None => Expr::new(Span::synthetic(), ExprNode::Seq { exprs: vec![] }),
            };
            let else_branch = match if_node.subsequent() {
                Some(sub) => {
                    if let Some(else_node) = sub.as_else_node() {
                        match else_node.statements() {
                            Some(s) => ingest_expr(&s.as_node(), file)?,
                            None => Expr::new(
                                Span::synthetic(),
                                ExprNode::Seq { exprs: vec![] },
                            ),
                        }
                    } else {
                        // elsif — recurse as nested if.
                        ingest_expr(&sub, file)?
                    }
                }
                None => Expr::new(Span::synthetic(), ExprNode::Lit { value: Literal::Nil }),
            };
            ExprNode::If { cond, then_branch, else_branch }
        }
        n if n.as_yield_node().is_some() => {
            let y = n.as_yield_node().unwrap();
            let args: Vec<Expr> = if let Some(a) = y.arguments() {
                a.arguments()
                    .iter()
                    .map(|arg| ingest_expr(&arg, file))
                    .collect::<IngestResult<_>>()?
            } else {
                vec![]
            };
            ExprNode::Yield { args }
        }
        n if n.as_or_node().is_some() => {
            let o = n.as_or_node().unwrap();
            let left = ingest_expr(&o.left(), file)?;
            let right = ingest_expr(&o.right(), file)?;
            let surface = bool_op_surface(o.operator_loc().as_slice());
            ExprNode::BoolOp { op: BoolOpKind::Or, surface, left, right }
        }
        n if n.as_and_node().is_some() => {
            let a = n.as_and_node().unwrap();
            let left = ingest_expr(&a.left(), file)?;
            let right = ingest_expr(&a.right(), file)?;
            let surface = bool_op_surface(a.operator_loc().as_slice());
            ExprNode::BoolOp { op: BoolOpKind::And, surface, left, right }
        }
        n if n.as_parentheses_node().is_some() => {
            // Parens are surface-only: unwrap to the inner expression.
            // Empty `()` shouldn't appear in well-formed Ruby, but fall back
            // to `nil` if it does rather than panicking.
            let p = n.as_parentheses_node().unwrap();
            return match p.body() {
                Some(inner) => ingest_expr(&inner, file),
                None => Ok(Expr::new(span, ExprNode::Lit { value: Literal::Nil })),
            };
        }
        n if n.as_array_node().is_some() => {
            let arr = n.as_array_node().unwrap();
            let style = array_style_from(&arr);
            let elements: Vec<Expr> = arr
                .elements()
                .iter()
                .map(|el| ingest_expr(&el, file))
                .collect::<IngestResult<_>>()?;
            ExprNode::Array { elements, style }
        }
        n if n.as_hash_node().is_some() => {
            let hn = n.as_hash_node().unwrap();
            ExprNode::Hash {
                entries: hash_entries_from(&hn.elements(), file)?,
                braced: true,
            }
        }
        n if n.as_keyword_hash_node().is_some() => {
            // Bare keyword args `foo(a: 1)` arrive here when the arg list
            // is passed through generic expression ingest. No braces in source.
            let kh = n.as_keyword_hash_node().unwrap();
            ExprNode::Hash {
                entries: hash_entries_from(&kh.elements(), file)?,
                braced: false,
            }
        }
        n if n.as_instance_variable_write_node().is_some() => {
            let w = n.as_instance_variable_write_node().unwrap();
            let raw = constant_id_str(&w.name());
            let name = raw.strip_prefix('@').unwrap_or(raw);
            let value = ingest_expr(&w.value(), file)?;
            ExprNode::Assign {
                target: crate::expr::LValue::Ivar { name: Symbol::from(name) },
                value,
            }
        }
        n if n.as_local_variable_write_node().is_some() => {
            let w = n.as_local_variable_write_node().unwrap();
            let name = Symbol::from(constant_id_str(&w.name()));
            let value = ingest_expr(&w.value(), file)?;
            ExprNode::Assign {
                target: crate::expr::LValue::Var { id: crate::ident::VarId(0), name },
                value,
            }
        }
        other => {
            return Err(IngestError::Unsupported {
                file: file.into(),
                message: format!("unsupported expression node: {other:?}"),
            });
        }
    };
    Ok(Expr::new(span, expr_node))
}

/// Ingest a `CallNode`'s block — the `do |...| ... end` or `{ |...| ... }`
/// attached to a method call. Represented as a `Lambda` expression.
/// Returns `None` for block-argument nodes (`&block`) which aren't closures.
fn ingest_call_block(node: &Node<'_>, file: &str) -> IngestResult<Option<Expr>> {
    let Some(b) = node.as_block_node() else {
        // `&block` — pass-through block argument, not a closure.
        return Ok(None);
    };
    let params = block_param_names(&b);
    let body = match b.body() {
        Some(body) => ingest_expr(&body, file)?,
        None => Expr::new(Span::synthetic(), ExprNode::Seq { exprs: vec![] }),
    };
    Ok(Some(Expr::new(
        Span::synthetic(),
        ExprNode::Lambda { params, block_param: None, body },
    )))
}

fn block_param_names(b: &ruby_prism::BlockNode<'_>) -> Vec<Symbol> {
    let Some(params_node) = b.parameters() else { return vec![] };
    let Some(bpn) = params_node.as_block_parameters_node() else {
        return vec![];
    };
    let Some(pn) = bpn.parameters() else { return vec![] };
    pn.requireds()
        .iter()
        .filter_map(|req| req.as_required_parameter_node())
        .map(|rp| Symbol::from(constant_id_str(&rp.name())))
        .collect()
}

/// Map the operator bytes of an `OrNode` / `AndNode` to the surface form.
/// Prism's `operator_loc` always points at the actual source bytes, so
/// `&&`/`||` map to `Symbol` and `and`/`or` to `Word`.
fn bool_op_surface(op_bytes: &[u8]) -> BoolOpSurface {
    match op_bytes {
        b"and" | b"or" => BoolOpSurface::Word,
        _ => BoolOpSurface::Symbol,
    }
}

/// Detect the surface form of an array literal from its opening token:
/// `%i[` → PercentI, `%w[` → PercentW, else Brackets.
fn array_style_from(arr: &ruby_prism::ArrayNode<'_>) -> crate::expr::ArrayStyle {
    use crate::expr::ArrayStyle;
    let Some(loc) = arr.opening_loc() else { return ArrayStyle::Brackets };
    let bytes = loc.as_slice();
    if bytes.starts_with(b"%i") || bytes.starts_with(b"%I") {
        ArrayStyle::PercentI
    } else if bytes.starts_with(b"%w") || bytes.starts_with(b"%W") {
        ArrayStyle::PercentW
    } else {
        ArrayStyle::Brackets
    }
}

fn hash_entries_from(
    elements: &ruby_prism::NodeList<'_>,
    file: &str,
) -> IngestResult<Vec<(Expr, Expr)>> {
    let mut out = Vec::new();
    for el in elements.iter() {
        let Some(assoc) = el.as_assoc_node() else {
            // Splats and other non-assoc elements: lift when a fixture demands.
            return Err(IngestError::Unsupported {
                file: file.into(),
                message: "non-assoc hash element (splat?) not yet supported".into(),
            });
        };
        let k = ingest_expr(&assoc.key(), file)?;
        let v = ingest_expr(&assoc.value(), file)?;
        out.push((k, v));
    }
    Ok(out)
}

// Prism helpers ---------------------------------------------------------

fn find_first_class<'pr>(node: &Node<'pr>) -> Option<ruby_prism::ClassNode<'pr>> {
    if let Some(c) = node.as_class_node() {
        return Some(c);
    }
    if let Some(p) = node.as_program_node() {
        return find_first_class(&p.statements().as_node());
    }
    if let Some(s) = node.as_statements_node() {
        for stmt in s.body().iter() {
            if let Some(found) = find_first_class(&stmt) {
                return Some(found);
            }
        }
    }
    None
}

fn class_name_path(class: &ruby_prism::ClassNode<'_>) -> Option<Vec<String>> {
    let cp = class.constant_path();
    constant_path_segments_strs(&cp)
}

fn constant_path_of(node: &Node<'_>) -> Option<Vec<String>> {
    constant_path_segments_strs(node)
}

fn constant_path_segments_strs(node: &Node<'_>) -> Option<Vec<String>> {
    if let Some(c) = node.as_constant_read_node() {
        return Some(vec![constant_id_str(&c.name()).to_string()]);
    }
    if let Some(p) = node.as_constant_path_node() {
        let mut out = p
            .parent()
            .and_then(|n| constant_path_segments_strs(&n))
            .unwrap_or_default();
        if let Some(id) = p.name() {
            out.push(constant_id_str(&id).to_string());
        }
        return Some(out);
    }
    None
}

fn constant_path_segments(p: &ruby_prism::ConstantPathNode<'_>) -> Vec<Symbol> {
    constant_path_segments_strs(&p.as_node())
        .unwrap_or_default()
        .into_iter()
        .map(Symbol::from)
        .collect()
}

fn flatten_statements<'pr>(node: Node<'pr>) -> Vec<Node<'pr>> {
    if let Some(s) = node.as_statements_node() {
        s.body().iter().collect()
    } else {
        vec![node]
    }
}

fn find_call_named<'pr>(node: &Node<'pr>, name: &str) -> Option<ruby_prism::CallNode<'pr>> {
    if let Some(c) = node.as_call_node() {
        if constant_id_str(&c.name()) == name {
            return Some(c);
        }
        if let Some(recv) = c.receiver() {
            if let Some(found) = find_call_named(&recv, name) {
                return Some(found);
            }
        }
        if let Some(args) = c.arguments() {
            for arg in args.arguments().iter() {
                if let Some(f) = find_call_named(&arg, name) {
                    return Some(f);
                }
            }
        }
        if let Some(block_node) = c.block() {
            if let Some(f) = find_call_named(&block_node, name) {
                return Some(f);
            }
        }
        return None;
    }
    if let Some(p) = node.as_program_node() {
        return find_call_named(&p.statements().as_node(), name);
    }
    if let Some(s) = node.as_statements_node() {
        for stmt in s.body().iter() {
            if let Some(f) = find_call_named(&stmt, name) {
                return Some(f);
            }
        }
    }
    if let Some(b) = node.as_block_node() {
        if let Some(body) = b.body() {
            return find_call_named(&body, name);
        }
    }
    None
}

fn walk_calls<'pr, F: FnMut(&ruby_prism::CallNode<'pr>)>(node: &Node<'pr>, f: &mut F) {
    if let Some(c) = node.as_call_node() {
        f(&c);
        if let Some(recv) = c.receiver() {
            walk_calls(&recv, f);
        }
        if let Some(args) = c.arguments() {
            for arg in args.arguments().iter() {
                walk_calls(&arg, f);
            }
        }
        if let Some(block_node) = c.block() {
            walk_calls(&block_node, f);
        }
        return;
    }
    if let Some(p) = node.as_program_node() {
        walk_calls(&p.statements().as_node(), f);
        return;
    }
    if let Some(s) = node.as_statements_node() {
        for stmt in s.body().iter() {
            walk_calls(&stmt, f);
        }
        return;
    }
    if let Some(b) = node.as_block_node() {
        if let Some(body) = b.body() {
            walk_calls(&body, f);
        }
    }
}

fn constant_id_str<'a>(id: &ruby_prism::ConstantId<'a>) -> &'a str {
    std::str::from_utf8(id.as_slice()).expect("prism constant id is UTF-8")
}

fn string_value(node: &Node<'_>) -> Option<String> {
    let s = node.as_string_node()?;
    Some(String::from_utf8_lossy(s.unescaped()).into_owned())
}

fn symbol_value(node: &Node<'_>) -> Option<String> {
    let s = node.as_symbol_node()?;
    let loc = s.value_loc()?;
    Some(String::from_utf8_lossy(loc.as_slice()).into_owned())
}

fn bool_value(node: &Node<'_>) -> Option<bool> {
    if node.as_true_node().is_some() {
        Some(true)
    } else if node.as_false_node().is_some() {
        Some(false)
    } else {
        None
    }
}

fn integer_value(node: &Node<'_>) -> Option<i64> {
    let i = node.as_integer_node()?;
    let v: i32 = i.value().try_into().ok()?;
    Some(v as i64)
}

// Naming conventions ----------------------------------------------------

use crate::naming::{camelize, pluralize_snake, singularize_camelize, snake_case};
