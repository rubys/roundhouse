//! `Dry::Validation::Contract` classes and `Dry::Schema.Params` constants
//! lowered into the plain Ruby they stand for.
//!
//! ```ruby
//! class UserContract < Dry::Validation::Contract
//!   params do
//!     required(:name).filled(:string)
//!     optional(:age).value(:integer, gt?: 0)
//!     optional(:address).hash do
//!       required(:city).filled(:string)
//!     end
//!   end
//! end
//! ```
//!
//! A contract's `call(input)` runs dry-schema's steps: the declared keys
//! are picked (a String key before a Symbol one under `params`/`json`,
//! only Symbols under `schema`), each value is coerced as the processor
//! coerces that type (`Params`, `JSON`, or not at all), and the rules run
//! in dry-schema's order: `filled?`, then the type, then the predicates,
//! the first failure giving the key its message. A nested `hash`/`array`
//! keeps only its declared keys. The result answers `success?`,
//! `failure?`, `to_h`, `values`, `[]` and `errors` (`to_h`, `empty?`,
//! `messages` and their `text`/`path`).
//!
//! The messages are read here, as dry-schema's YAML backend reads them:
//! its own `errors.yml` (under the contract's `top_namespace`) and
//! dry-validation's, then each `config.messages.load_paths` file, looked
//! up for `default_locale` along dry-schema's paths. A message the files
//! do not have raises `MissingMessageError` where it is needed, as the
//! gem does.
//!
//! Checked against dry-validation 1.11.1 / dry-schema 1.14.1. A contract
//! with `rule`s, options, macros or anything else not modeled here is left
//! as it was, ledgered, together with every contract it would inherit a
//! `call` from or hand one to; one without a schema gets nothing.

use std::collections::{HashMap, HashSet};

use crate::App;
use crate::dialect::{LibraryClass, LibraryClassOrigin};
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::{ClassId, Symbol};

use super::IngestError;
use super::dry_types::resolve;
use super::survey;

const DRY_SCHEMA_ERRORS: &str = include_str!("dry_validation/dry_schema_errors.yml");
const DRY_VALIDATION_ERRORS: &str = include_str!("dry_validation/dry_validation_errors.yml");

#[derive(Clone, Copy, PartialEq, Debug)]
enum Processor {
    Params,
    Json,
    Schema,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Macro {
    Value,
    Filled,
    Maybe,
}

/// A type spec: what the processor coerces to and the predicate checking
/// it; a predicate alone (`:number?`, `type?: Hash`) coerces nothing.
#[derive(Clone, Debug)]
enum Ty {
    Coerced(&'static str),
    Predicate(&'static str),
    TypeOf(Expr),
    /// A dry type: it coerces leniently, its primitive is checked, then
    /// its constraints; `.optional` lets nil through, `.default` fills a
    /// missing key.
    Dry(Box<super::dry_types::DryType>),
    Any,
}

#[derive(Clone, Debug)]
struct Rule {
    macro_: Macro,
    ty: Ty,
    /// dry-logic predicates after the type, in order; a unary one (`int?`
    /// in a predicate block) takes no argument.
    preds: Vec<(String, Option<Expr>)>,
    nested: Option<Nested>,
    /// A chained `.hash` checks `hash?` before the macro's `filled?`.
    type_first: bool,
}

#[derive(Clone, Debug)]
enum Nested {
    /// A Hash of these keys, coerced by this processor.
    Hash(Schema),
    /// Each member through this rule.
    Array(Box<Rule>),
}

#[derive(Clone, Debug)]
struct Key {
    name: String,
    required: bool,
    rule: Rule,
}

#[derive(Clone, Debug)]
struct Schema {
    processor: Processor,
    keys: Vec<Key>,
}

impl Schema {
    /// `other`'s keys after these, a later key replacing an earlier one.
    fn extend(&mut self, other: &Schema) {
        for key in &other.keys {
            match self.keys.iter_mut().find(|k| k.name == key.name) {
                Some(slot) => *slot = key.clone(),
                None => self.keys.push(key.clone()),
            }
        }
    }
}

/// A contract's own declarations.
#[derive(Clone, Debug, Default)]
struct Contract {
    schema: Option<Schema>,
    rules: Vec<RuleDecl>,
}

/// `rule(:a, addr: :city) do ... end`: the key paths, the block's body.
#[derive(Clone, Debug)]
struct RuleDecl {
    paths: Vec<Vec<String>>,
    body: Expr,
}

/// A `failure` in a rule: where (None for `base`), and what.
struct Failure<'a> {
    path: Option<Vec<String>>,
    message: FailureMessage<'a>,
}

enum FailureMessage<'a> {
    Text(&'a str),
    /// A message id, looked up as dry-validation looks it up, with its
    /// interpolation tokens.
    Id(&'a str, Vec<(String, Expr)>),
}

/// `config.messages` as a class body sets it.
#[derive(Clone, Default, Debug)]
struct Config {
    top_namespace: Option<ConfigValue>,
    default_locale: Option<String>,
    load_paths: Vec<String>,
    refused: Option<String>,
}

#[derive(Clone, Debug)]
enum ConfigValue {
    Literal(String),
    Constant(String),
}

/// What a schema's names are read against.
struct Cx<'a> {
    names: &'a HashSet<String>,
    types_modules: &'a [(String, super::dry_types::BareNames)],
    constants: &'a HashMap<String, Expr>,
    classes: &'a HashSet<String>,
    structs: &'a HashMap<String, Option<String>>,
    /// Each class's parent, resolved: a constant is found on the ancestors
    /// after the lexical scopes.
    parents: &'a HashMap<String, String>,
}

pub(super) fn lower_dry_validation(
    app: &mut App,
    sources: &[crate::span::SourceFile],
    read_file: &dyn Fn(&str) -> Option<String>,
) -> super::IngestResult<()> {
    let names: HashSet<String> = app.library_classes.iter().map(|lc| lc.name.0.as_str().to_string()).collect();
    // Each contract and its contract parent (None under the gem's class).
    let mut parent_of: HashMap<String, Option<String>> = HashMap::new();
    loop {
        let before = parent_of.len();
        for lc in &app.library_classes {
            let name = lc.name.0.as_str();
            if lc.is_module || parent_of.contains_key(name) {
                continue;
            }
            let Some(parent) = &lc.parent else { continue };
            let written = parent.0.as_str().trim_start_matches("::");
            if written == "Dry::Validation::Contract" {
                parent_of.insert(name.to_string(), None);
            } else if let Some(resolved) = resolve(name, written, &names)
                && parent_of.contains_key(&resolved)
            {
                parent_of.insert(name.to_string(), Some(resolved));
            }
        }
        if parent_of.len() == before {
            break;
        }
    }
    let constants: HashMap<String, Expr> = app
        .library_classes
        .iter()
        .flat_map(|lc| {
            lc.constants.iter().map(move |(n, v)| (format!("{}::{}", lc.name.0.as_str(), n.as_str()), v.clone()))
        })
        .collect();
    let classes: HashSet<String> = app
        .models
        .iter()
        .map(|m| m.name.0.as_str().to_string())
        .chain(names.iter().cloned())
        .collect();
    let types_modules = super::dry_types::types_modules(sources);
    let no_structs: HashMap<String, Option<String>> = HashMap::new();
    // A type is named by a class or by a constant holding one.
    let type_names: HashSet<String> = names.iter().cloned().chain(constants.keys().cloned()).collect();
    let parents: HashMap<String, String> = app
        .library_classes
        .iter()
        .filter_map(|lc| {
            let name = lc.name.0.as_str();
            let written = lc.parent.as_ref()?.0.as_str().trim_start_matches("::");
            Some((name.to_string(), resolve(name, written, &names)?))
        })
        .collect();
    let cx = Cx {
        names: &type_names,
        types_modules: &types_modules,
        constants: &constants,
        classes: &classes,
        structs: &no_structs,
        parents: &parents,
    };
    // `Dry::Schema.Params do ... end` constants, read where they stand; one
    // naming another once that one is read.
    let mut schemas: HashMap<String, Result<Schema, String>> = HashMap::new();
    let mut schemas_read: HashMap<String, Schema> = HashMap::new();
    loop {
        let before = schemas_read.len();
        for (full, value) in &constants {
            if schemas_read.contains_key(full) {
                continue;
            }
            if let Some((processor, body)) = schema_builder(value) {
                let owner = full.rsplit_once("::").map_or("", |(o, _)| o);
                let read = read_schema(processor, body, owner, &cx, &schemas_read);
                if let Ok(schema) = &read {
                    schemas_read.insert(full.clone(), schema.clone());
                }
                schemas.insert(full.clone(), read);
            }
        }
        if schemas_read.len() == before {
            break;
        }
    }
    if parent_of.is_empty() && schemas.is_empty() {
        return Ok(());
    }
    let configs = read_configs(sources);

    let chain_of = |name: &str| -> Vec<String> {
        let mut chain = vec![name.to_string()];
        while let Some(Some(parent)) = parent_of.get(chain.last().unwrap()) {
            chain.push(parent.clone());
        }
        chain.reverse();
        chain
    };

    // Each contract's own schema, or why it is not read. Round by round:
    // a contract naming another's `.schema` reads once that one has.
    let mut own: HashMap<String, Contract> = HashMap::new();
    let mut refused: HashMap<String, String> = HashMap::new();
    let mut schemas_all = schemas_read.clone();
    let mut pending: Vec<String> = parent_of.keys().cloned().collect();
    pending.sort();
    let mut last: HashMap<String, String> = HashMap::new();
    loop {
        let before = pending.len();
        let mut still = Vec::new();
        for name in pending {
            let lc = app.library_classes.iter().find(|lc| lc.name.0.as_str() == name).expect("listed");
            if let Some(reason) = configs.get(&name).and_then(|c| c.refused.clone()) {
                refused.insert(name.clone(), reason);
                continue;
            }
            // A rule may call the contract's own methods, an ancestor's too.
            let methods: HashSet<String> = chain_of(&name)
                .iter()
                .filter_map(|n| app.library_classes.iter().find(|lc| lc.name.0.as_str() == n))
                .flat_map(|lc| lc.methods.iter().map(|m| m.name.as_str().to_string()))
                .collect();
            match read_contract(lc, &cx, &schemas_all, &methods) {
                Ok(contract) => {
                    own.insert(name.clone(), contract);
                }
                Err(reason) => {
                    last.insert(name.clone(), reason);
                    still.push(name);
                }
            }
        }
        for name in parent_of.keys() {
            let chain = chain_of(name);
            if chain.iter().all(|n| own.contains_key(n))
                && let Some(schema) = merged_schema(&chain, &own)
            {
                schemas_all.insert(format!("{name}.schema"), schema);
            }
        }
        pending = still;
        if pending.len() == before || pending.is_empty() {
            break;
        }
    }
    for name in pending {
        let reason = last.remove(&name).unwrap_or_default();
        refused.insert(name, reason);
    }
    // A schema is inherited, and so is the `call` it makes: a refused
    // contract under a lowered one would answer its parent's, and one over
    // a lowered child would be its parent. The topmost contract with a
    // schema above a refusal goes with it, and all below that.
    let has_schema = |name: &str| chain_of(name).iter().any(|n| own.get(n).is_some_and(|c| c.schema.is_some()));
    let mut refused_tops: HashSet<String> = HashSet::new();
    for name in refused.keys() {
        let chain = chain_of(name);
        let top = chain.iter().find(|n| has_schema(n)).cloned().unwrap_or_else(|| name.clone());
        refused_tops.insert(top);
    }
    let lowered = |name: &str| !chain_of(name).iter().any(|n| refused_tops.contains(n));
    // A refusal is a gap in survey mode and a failure otherwise, in a
    // repeatable order.
    let mut refusals: Vec<(&String, &String)> = refused.iter().collect();
    refusals.sort();
    for (name, reason) in refusals {
        survey::continue_or_fail(IngestError::Unsupported {
            file: name.clone(),
            message: format!("Dry::Validation not lowered: {reason}"),
        })?;
    }

    let mut generated: Vec<(String, String)> = Vec::new();
    let mut used: HashSet<&'static str> = HashSet::new();
    for name in parent_of.keys() {
        if !lowered(name) || !has_schema(name) {
            continue;
        }
        let chain = chain_of(name);
        let rules: Vec<RuleDecl> = chain.iter().filter_map(|n| own.get(n)).flat_map(|c| c.rules.iter().cloned()).collect();
        let schema = merged_schema(&chain, &own).expect("has a schema");
        let messages = messages_for(&chain, &configs, &constants, read_file, "dry_validation");
        let methods: HashSet<String> = chain
            .iter()
            .filter_map(|n| app.library_classes.iter().find(|lc| lc.name.0.as_str() == n))
            .flat_map(|lc| lc.methods.iter().map(|m| m.name.as_str().to_string()))
            .collect();
        generated.push((name.clone(), contract_source(name, &schema, &rules, &methods, &messages, &mut used)));
    }
    let mut schema_classes: Vec<(String, String, String)> = Vec::new();
    let mut schema_names: Vec<&String> = schemas.keys().collect();
    schema_names.sort();
    for full in schema_names {
        match &schemas[full] {
            Ok(schema) => {
                let (owner, constant) = full.rsplit_once("::").unwrap_or(("", full));
                let class = format!("{owner}::DrySchema{constant}");
                let messages = Messages::new(&Config::default(), None, read_file, "dry_schema");
                schema_classes.push((full.clone(), class.clone(), schema_source(&class, schema, &messages, &mut used)));
            }
            Err(reason) => survey::continue_or_fail(IngestError::Unsupported {
                file: full.clone(),
                message: format!("Dry::Schema not lowered: {reason}"),
            })?,
        }
    }

    // Build every class before committing to any.
    let mut built: HashMap<String, Vec<crate::dialect::MethodDef>> = HashMap::new();
    for (name, source) in &generated {
        if let Some(methods) = parse_methods(name, source) {
            built.insert(name.clone(), methods);
        }
    }
    let mut any = false;
    for lc in &mut app.library_classes {
        let name = lc.name.0.as_str().to_string();
        if !parent_of.contains_key(&name) || !lowered(&name) {
            continue;
        }
        if has_schema(&name) && !built.contains_key(&name) {
            continue;
        }
        lc.unknown_calls.retain(|call| !is_schema_definition(call) && !is_rule(call));
        if let Some(methods) = built.remove(&name) {
            let mut methods = methods;
            methods.append(&mut lc.methods);
            lc.methods = methods;
            lc.origin = Some(LibraryClassOrigin::DryValidation);
        }
        if let Some(Some(parent)) = parent_of.get(&name) {
            lc.parent = Some(ClassId(Symbol::from(parent.as_str())));
        }
        any = true;
    }
    for (constant, class, source) in &schema_classes {
        let (owner, local) = constant.rsplit_once("::").unwrap_or(("", constant));
        let Some(mut classes) = parse_classes(source) else { continue };
        let Some(holder) = app.library_classes.iter_mut().find(|lc| lc.name.0.as_str() == owner) else { continue };
        let Some(slot) = holder.constants.iter_mut().find(|(n, _)| n.as_str() == local) else { continue };
        let Some(instance) = parse_expr(&format!("::{class}.new")) else { continue };
        slot.1 = instance;
        for c in &mut classes {
            c.name = ClassId(Symbol::from(class.as_str()));
            c.origin = Some(LibraryClassOrigin::DryValidation);
        }
        app.library_classes.extend(classes);
        any = true;
    }
    // The gem is never emitted. The stand-ins hold what the lowered calls
    // answer, and the empty `Contract` keeps a refused contract's
    // declarations from being replayed onto nothing.
    if any || !parent_of.is_empty() {
        app.library_classes.extend(stand_ins(&used));
    }
    Ok(())
}

/// A contract's schema along its chain: a child's keys after its parent's,
/// its processor over theirs.
fn merged_schema(chain: &[String], own: &HashMap<String, Contract>) -> Option<Schema> {
    let mut schema: Option<Schema> = None;
    for n in chain {
        if let Some(Contract { schema: Some(s), .. }) = own.get(n) {
            match &mut schema {
                None => schema = Some(s.clone()),
                Some(acc) => {
                    acc.processor = s.processor;
                    acc.extend(s);
                }
            }
        }
    }
    schema
}

/// `Dry::Schema.Params do ... end`, `.JSON`, `.define`: the processor and
/// the block.
fn schema_builder(expr: &Expr) -> Option<(Processor, &Expr)> {
    let ExprNode::Send { recv: Some(recv), method, args, block: Some(block), .. } = &*expr.node else {
        return None;
    };
    if !args.is_empty() || !is_const(recv, "Dry::Schema") {
        return None;
    }
    let processor = match method.as_str() {
        "Params" => Processor::Params,
        "JSON" => Processor::Json,
        "define" => Processor::Schema,
        _ => return None,
    };
    Some((processor, block))
}

fn is_const(expr: &Expr, name: &str) -> bool {
    matches!(&*expr.node, ExprNode::Const { path }
        if path.iter().map(|s| s.as_str()).filter(|s| !s.is_empty()).collect::<Vec<_>>().join("::") == name)
}

fn is_rule(call: &Expr) -> bool {
    matches!(&*call.node, ExprNode::Send { recv: None, method, block: Some(_), .. } if method.as_str() == "rule")
}

fn is_schema_definition(call: &Expr) -> bool {
    matches!(&*call.node, ExprNode::Send { recv: None, method, block: Some(_), .. }
        if matches!(method.as_str(), "params" | "json" | "schema"))
}

/// A contract's own schema (None for a contract declaring none).
fn read_contract(
    lc: &LibraryClass,
    cx: &Cx<'_>,
    schemas: &HashMap<String, Schema>,
    methods: &HashSet<String>,
) -> Result<Contract, String> {
    let owner = lc.name.0.as_str();
    let mut schema: Option<Schema> = None;
    let mut rules = Vec::new();
    for call in &lc.unknown_calls {
        let ExprNode::Send { recv, method, args, block, .. } = &*call.node else {
            return Err("a non-declaration expression in the class body".into());
        };
        if recv.is_some() {
            return Err(format!("`{}` with a receiver in the class body", method.as_str()));
        }
        if method.as_str() == "rule" {
            let rule = read_rule_decl(args, block.as_ref())?;
            // Checked here, so a refusal is known before any is generated.
            rewrite_rule(&rule, methods, &mut |_| "nil".to_string())?;
            rules.push(rule);
            continue;
        }
        let processor = match method.as_str() {
            "params" => Processor::Params,
            "json" => Processor::Json,
            "schema" => Processor::Schema,
            other => return Err(format!("`{other}` in the class body")),
        };
        if schema.is_some() {
            return Err(format!("a second `{}`", method.as_str()));
        }
        let mut read = Schema { processor, keys: Vec::new() };
        for arg in args {
            read.extend(schema_ref(arg, owner, cx, schemas)?);
        }
        if let Some(block) = block {
            read.extend(&read_schema(processor, block, owner, cx, schemas)?);
        }
        schema = Some(read);
    }
    Ok(Contract { schema, rules })
}

/// A rule's keys: `:a`, `"a.b"`, `a: :b`.
fn read_rule_decl(args: &[Expr], block: Option<&Expr>) -> Result<RuleDecl, String> {
    let Some(block) = block else { return Err("a `rule` without a block".into()) };
    let ExprNode::Lambda { params, body, .. } = &*block.node else { return Err("a `rule` without a block".into()) };
    if !params.is_empty() {
        return Err("a `rule` block with parameters".into());
    }
    fn path(expr: &Expr) -> Option<Vec<String>> {
        match &*expr.node {
            ExprNode::Lit { value: Literal::Sym { value } } => Some(vec![value.as_str().to_string()]),
            ExprNode::Lit { value: Literal::Str { value } } => Some(value.split('.').map(str::to_string).collect()),
            _ => None,
        }
    }
    let mut paths = Vec::new();
    for arg in args {
        match &*arg.node {
            ExprNode::Hash { entries, .. } => {
                for (k, v) in entries {
                    let mut p = path(k).ok_or("a computed rule key")?;
                    p.extend(path(v).ok_or("a computed rule key")?);
                    paths.push(p);
                }
            }
            _ => paths.push(path(arg).ok_or_else(|| format!("rule key `{}`", crate::emit::ruby::emit_expr(arg)))?),
        }
    }
    if paths.is_empty() {
        return Err("a `rule` without keys".into());
    }
    Ok(RuleDecl { paths, body: body.clone() })
}

/// What a rule's evaluator answers that the lowering does not model.
const RULE_API: &[&str] = &[
    "rule_error?", "error?", "context", "_context", "macro", "keys", "path", "result", "_options", "base_rule_error?",
];

/// A rule's body as a method of the contract taking `dry_rule_values`,
/// `dry_rule_errors` and `dry_rule_schema_errors`: `value`, `values`,
/// `key?`, `schema_error?` read them, `key.failure`/`key(..).failure`/
/// `base.failure` write the message `failure` gives, `next` returns.
fn rewrite_rule(
    rule: &RuleDecl,
    methods: &HashSet<String>,
    failure: &mut dyn FnMut(Failure<'_>) -> String,
) -> Result<Expr, String> {
    fn ruby(src: &str) -> Expr {
        parse_expr(src).expect("generated Ruby parses")
    }
    fn sym_path(expr: &Expr) -> Option<Vec<String>> {
        match &*expr.node {
            ExprNode::Lit { value: Literal::Sym { value } } => Some(vec![value.as_str().to_string()]),
            ExprNode::Lit { value: Literal::Str { value } } => Some(value.split('.').map(str::to_string).collect()),
            ExprNode::Array { elements, .. } => {
                elements.iter().map(|e| sym_path(e).map(|p| p.join("."))).collect::<Option<Vec<_>>>()
                    .map(|v| v.into_iter().flat_map(|p| p.split('.').map(str::to_string).collect::<Vec<_>>()).collect())
            }
            _ => None,
        }
    }
    fn path_src(path: &[String]) -> String {
        format!("[{}]", path.iter().map(|k| format!(":{k}")).collect::<Vec<_>>().join(", "))
    }
    fn walk(
        expr: &Expr,
        rule: &RuleDecl,
        methods: &HashSet<String>,
        failure: &mut dyn FnMut(Failure<'_>) -> String,
    ) -> Result<Expr, String> {
        let first = &rule.paths[0];
        let shown = || crate::emit::ruby::emit_expr(expr).lines().next().unwrap_or_default().to_string();
        match &*expr.node {
            ExprNode::Ivar { .. } | ExprNode::SelfRef => return Err(format!("`{}` in a rule", shown())),
            ExprNode::Send { recv: None, method, args, block: None, .. } => match (method.as_str(), args.as_slice()) {
                ("value", []) => {
                    return Ok(ruby(&if first.len() == 1 {
                        format!("dry_rule_values[:{}]", first[0])
                    } else {
                        format!("dry_rule_values.dig({})", first.iter().map(|k| format!(":{k}")).collect::<Vec<_>>().join(", "))
                    }));
                }
                ("values", []) => return Ok(ruby("dry_rule_values")),
                ("key?", []) => return Ok(ruby(&format!("::Dry::Schema::Lowered.key_at?(dry_rule_values, {})", path_src(first)))),
                ("key?", [p]) => {
                    let p = sym_path(p).ok_or_else(|| format!("`{}` in a rule", shown()))?;
                    return Ok(ruby(&format!("::Dry::Schema::Lowered.key_at?(dry_rule_values, {})", path_src(&p))));
                }
                ("schema_error?", [p]) => {
                    let p = sym_path(p).ok_or_else(|| format!("`{}` in a rule", shown()))?;
                    return Ok(ruby(&format!("::Dry::Schema::Lowered.error_at?(dry_rule_schema_errors, {})", path_src(&p))));
                }
                (m, _) if matches!(m, "key" | "base" | "value" | "values" | "key?" | "schema_error?") || RULE_API.contains(&m) => {
                    return Err(format!("`{}` in a rule", shown()));
                }
                _ => {}
            },
            ExprNode::Send { recv: Some(target), method, args, block: None, .. } if method.as_str() == "failure" => {
                let path = match &*target.node {
                    ExprNode::Send { recv: None, method: m, args: a, block: None, .. } => match (m.as_str(), a.as_slice()) {
                        ("key", []) => Some(first.clone()),
                        ("key", [p]) => Some(sym_path(p).ok_or_else(|| format!("`{}` in a rule", shown()))?),
                        ("base", []) => None,
                        _ => return Err(format!("`{}` in a rule", shown())),
                    },
                    _ => return Err(format!("`{}` in a rule", shown())),
                };
                let message = match args.as_slice() {
                    [m] | [m, _] => match &*m.node {
                        ExprNode::Lit { value: Literal::Str { value } } if args.len() == 1 => FailureMessage::Text(value),
                        ExprNode::Lit { value: Literal::Sym { value } } => {
                            let mut tokens = Vec::new();
                            if let [_, opts] = args.as_slice() {
                                let ExprNode::Hash { entries, .. } = &*opts.node else {
                                    return Err(format!("`{}` in a rule", shown()));
                                };
                                for (k, v) in entries {
                                    let ExprNode::Lit { value: Literal::Sym { value: k } } = &*k.node else {
                                        return Err(format!("`{}` in a rule", shown()));
                                    };
                                    tokens.push((k.as_str().to_string(), walk(v, rule, methods, failure)?));
                                }
                            }
                            FailureMessage::Id(value.as_str(), tokens)
                        }
                        _ => return Err(format!("`{}` in a rule", shown())),
                    },
                    _ => return Err(format!("`{}` in a rule", shown())),
                };
                let target = match &path {
                    Some(p) => path_src(p),
                    None => "[nil]".to_string(),
                };
                let text = failure(Failure { path, message });
                return Ok(ruby(&format!("::Dry::Schema::Lowered.add_error(dry_rule_errors, {target}, {text})")));
            }
            _ => {}
        }
        let mut out = expr.clone();
        let mut err = None;
        out.node.for_each_child_mut(&mut |child| {
            if err.is_some() {
                return;
            }
            match walk(child, rule, methods, failure) {
                Ok(rewritten) => *child = rewritten,
                Err(e) => err = Some(e),
            }
        });
        match err {
            Some(e) => Err(e),
            None => Ok(out),
        }
    }
    let _ = methods;
    let body = walk(&rule.body, rule, methods, failure)?;
    Ok(super::dry_types::next_to_return(&body))
}

/// A schema constant named where a schema belongs.
fn schema_ref<'a>(
    expr: &Expr,
    owner: &str,
    cx: &Cx<'_>,
    schemas: &'a HashMap<String, Schema>,
) -> Result<&'a Schema, String> {
    // `OtherContract.schema`: that contract's, published once read.
    let (path, suffix) = match &*expr.node {
        ExprNode::Const { path } => (path, ""),
        ExprNode::Send { recv: Some(recv), method, args, block: None, .. }
            if method.as_str() == "schema" && args.is_empty() =>
        {
            match &*recv.node {
                ExprNode::Const { path } => (path, ".schema"),
                _ => return Err(format!("schema `{}`", crate::emit::ruby::emit_expr(expr))),
            }
        }
        _ => return Err(format!("schema `{}`", crate::emit::ruby::emit_expr(expr))),
    };
    let absolute = path.first().is_some_and(|s| s.as_str().is_empty());
    let written: Vec<&str> = path.iter().map(|s| s.as_str()).filter(|s| !s.is_empty()).collect();
    let name = written.join("::");
    // Lexically outward, then up the owner's ancestors, as Ruby looks.
    let candidates: Vec<String> = if absolute {
        vec![name.clone()]
    } else {
        let mut out = vec![format!("{owner}::{name}")];
        let mut scope: Vec<&str> = owner.split("::").collect();
        while !scope.is_empty() {
            scope.pop();
            out.push(if scope.is_empty() { name.clone() } else { format!("{}::{name}", scope.join("::")) });
        }
        let mut ancestor = cx.parents.get(owner);
        while let Some(a) = ancestor {
            out.push(format!("{a}::{name}"));
            ancestor = cx.parents.get(a);
        }
        out
    };
    candidates
        .iter()
        .find_map(|c| schemas.get(&format!("{c}{suffix}")))
        .ok_or_else(|| format!("schema `{name}{suffix}`"))
}

fn read_schema(
    processor: Processor,
    block: &Expr,
    owner: &str,
    cx: &Cx<'_>,
    schemas: &HashMap<String, Schema>,
) -> Result<Schema, String> {
    let ExprNode::Lambda { params, body, .. } = &*block.node else {
        return Err("a schema without a block".into());
    };
    if !params.is_empty() {
        return Err("a schema block with parameters".into());
    }
    let statements: Vec<&Expr> = match &*body.node {
        ExprNode::Seq { exprs } => exprs.iter().collect(),
        ExprNode::Lit { value: Literal::Nil } => Vec::new(),
        _ => vec![body],
    };
    let mut schema = Schema { processor, keys: Vec::new() };
    for statement in statements {
        let key = read_key(statement, processor, owner, cx, schemas)?;
        schema.extend(&Schema { processor, keys: vec![key] });
    }
    Ok(schema)
}

fn read_key(
    expr: &Expr,
    processor: Processor,
    owner: &str,
    cx: &Cx<'_>,
    schemas: &HashMap<String, Schema>,
) -> Result<Key, String> {
    let shown = || crate::emit::ruby::emit_expr(expr).lines().next().unwrap_or_default().to_string();
    // `required(:k) { int? & gt?(0) }`: the predicates alone.
    if let ExprNode::Send { recv: None, method: kind, args: key_args, block: Some(block), .. } = &*expr.node
        && matches!(kind.as_str(), "required" | "optional")
        && let [key] = key_args.as_slice()
        && let ExprNode::Lit { value: Literal::Sym { value: name } } = &*key.node
    {
        let preds = predicate_block(block).map_err(|e| format!("`{}`: {e}", name.as_str()))?;
        return Ok(Key {
            name: name.as_str().to_string(),
            required: kind.as_str() == "required",
            rule: Rule { macro_: Macro::Value, ty: Ty::Any, preds, nested: None, type_first: false },
        });
    }
    // `required(:k)` alone: present, whatever it holds.
    if let ExprNode::Send { recv: None, method: kind, args: key_args, block: None, .. } = &*expr.node
        && matches!(kind.as_str(), "required" | "optional")
        && let [key] = key_args.as_slice()
        && let ExprNode::Lit { value: Literal::Sym { value: name } } = &*key.node
    {
        return Ok(Key {
            name: name.as_str().to_string(),
            required: kind.as_str() == "required",
            rule: Rule { macro_: Macro::Value, ty: Ty::Any, preds: Vec::new(), nested: None, type_first: false },
        });
    }
    // `.value(:array, ...).each(...)`: the members' rule on the array's;
    // `.value(:hash).schema(S)`: the keys' on the hash's.
    let (expr, each, keys) = match &*expr.node {
        ExprNode::Send { recv: Some(recv), method, args, block, .. }
            if method.as_str() == "each" && matches!(&*recv.node, ExprNode::Send { recv: Some(_), .. }) =>
        {
            (recv, Some((args.as_slice(), block.as_ref())), None)
        }
        ExprNode::Send { recv: Some(recv), method, args, block, .. }
            if matches!(method.as_str(), "schema" | "hash") && matches!(&*recv.node, ExprNode::Send { recv: Some(_), .. }) =>
        {
            (recv, None, Some((args.as_slice(), block.as_ref(), method.as_str() == "hash")))
        }
        _ => (expr, None, None),
    };
    let ExprNode::Send { recv: Some(recv), method, args, block, .. } = &*expr.node else {
        return Err(format!("`{}` in a schema", shown()));
    };
    let ExprNode::Send { recv: None, method: kind, args: key_args, block: None, .. } = &*recv.node else {
        return Err(format!("`{}` in a schema", shown()));
    };
    let required = match kind.as_str() {
        "required" => true,
        "optional" => false,
        _ => return Err(format!("`{}` in a schema", shown())),
    };
    let [key] = key_args.as_slice() else { return Err(format!("`{}` in a schema", shown())) };
    let ExprNode::Lit { value: Literal::Sym { value: name } } = &*key.node else {
        return Err(format!("a key named by `{}`", crate::emit::ruby::emit_expr(key)));
    };
    let mut rule = read_rule(method.as_str(), args, block.as_ref(), processor, owner, cx, schemas)
        .map_err(|e| format!("`{}`: {e}", name.as_str()))?;
    if let Some((keys_args, keys_block, chained_hash)) = keys {
        // After `maybe` the gem's chain treats nil its own way (a second
        // `hash?`, or the keys read off nil): not modeled.
        if !matches!(rule.ty, Ty::Coerced("hash")) || rule.nested.is_some() || rule.macro_ == Macro::Maybe {
            return Err(format!("`{}`: a schema chained on what is not a hash", name.as_str()));
        }
        let Rule { nested, .. } = read_rule("hash", keys_args, keys_block, processor, owner, cx, schemas)
            .map_err(|e| format!("`{}`: {e}", name.as_str()))?;
        rule.nested = nested;
        rule.type_first = chained_hash;
    }
    if let Some((each_args, each_block)) = each {
        if !matches!(rule.ty, Ty::Coerced("array")) || rule.nested.is_some() {
            return Err(format!("`{}`: `each` on what is not an array", name.as_str()));
        }
        let member = match (each_args, each_block) {
            // `each do value(:array, size?: 3) end`: the member's own rule.
            ([], Some(block)) => {
                let ExprNode::Lambda { params, body, .. } = &*block.node else { unreachable!("a block") };
                let ExprNode::Send { recv: None, method, args, block, .. } = &*body.node else {
                    return Err(format!("`{}`: an `each` block of more than one rule", name.as_str()));
                };
                if !params.is_empty() || !matches!(method.as_str(), "value" | "filled" | "maybe" | "hash" | "schema" | "array") {
                    return Err(format!("`{}`: `{}` in an `each` block", name.as_str(), method.as_str()));
                }
                Box::new(
                    read_rule(method.as_str(), args, block.as_ref(), processor, owner, cx, schemas)
                        .map_err(|e| format!("`{}`: {e}", name.as_str()))?,
                )
            }
            _ => {
                let Some(Nested::Array(member)) =
                    read_rule("array", each_args, each_block, processor, owner, cx, schemas)
                        .map_err(|e| format!("`{}`: {e}", name.as_str()))?
                        .nested
                else {
                    unreachable!("an array rule")
                };
                member
            }
        };
        rule.nested = Some(Nested::Array(member));
    }
    Ok(Key { name: name.as_str().to_string(), required, rule })
}

/// A dry type a schema is read with here: a scalar whose coercion and
/// primitive are modeled, its constraints dry-logic's.
fn schema_type(ty: &super::dry_types::DryType) -> bool {
    use super::dry_types::Base;
    ty.fallback.is_none()
        && dry_primitive(&ty.base).is_some()
        && !matches!(ty.base, Base::Coerce("Hash" | "BigDecimal"))
        && ty.constraints.iter().all(|(p, _)| {
            matches!(
                p.as_str(),
                "gt" | "gteq" | "lt" | "lteq" | "min_size" | "max_size" | "size" | "format" | "included_in"
                    | "excluded_from" | "eql" | "filled"
            )
        })
}

/// The predicate a dry type's primitive is checked with (`""` for none).
fn dry_primitive(base: &super::dry_types::Base) -> Option<&'static str> {
    use super::dry_types::Base;
    Some(match base {
        Base::Coerce("String") | Base::Strict("String") => "str?",
        Base::Coerce("Integer") | Base::ParamsInteger | Base::Strict("Integer") => "int?",
        Base::Coerce("Float") | Base::Strict("Float") => "float?",
        Base::ParamsBool | Base::StrictBool => "bool?",
        Base::ParamsDecimal => "decimal?",
        Base::Parse("Date") => "date?",
        Base::Parse("DateTime") => "date_time?",
        Base::Parse("Time") => "time?",
        Base::Strict("Hash") => "hash?",
        Base::Strict("Array") => "array?",
        Base::Instance(_) => "type?",
        // `Instance(A) | Instance(B)`: either type, one message per side.
        Base::Sum(members)
            if members.iter().all(|m| {
                matches!(m.base, Base::Instance(_))
                    && !m.optional
                    && m.default.is_none()
                    && m.enumeration.is_none()
                    && m.constraints.is_empty()
                    && m.fallback.is_none()
            }) =>
        {
            "type_sum"
        }
        Base::Nominal => "",
        _ => return None,
    })
}

/// `{ int? & gt?(0) }`: the predicates in order.
fn predicate_block(block: &Expr) -> Result<Vec<(String, Option<Expr>)>, String> {
    let ExprNode::Lambda { params, body, .. } = &*block.node else { return Err("a block".into()) };
    if !params.is_empty() {
        return Err("a predicate block with parameters".into());
    }
    fn walk(expr: &Expr, out: &mut Vec<(String, Option<Expr>)>) -> Result<(), String> {
        let ExprNode::Send { recv, method, args, block: None, .. } = &*expr.node else {
            return Err(format!("predicate `{}`", crate::emit::ruby::emit_expr(expr)));
        };
        match (recv, method.as_str(), args.as_slice()) {
            (Some(left), "&", [right]) => {
                walk(left, out)?;
                walk(right, out)
            }
            (
                None,
                p @ ("str?" | "int?" | "float?" | "decimal?" | "bool?" | "date?" | "date_time?" | "time?"
                | "hash?" | "array?" | "number?" | "filled?"),
                [],
            ) => {
                out.push((p.to_string(), None));
                Ok(())
            }
            (
                None,
                p @ ("gt?" | "gteq?" | "lt?" | "lteq?" | "min_size?" | "max_size?" | "size?" | "format?"
                | "included_in?" | "excluded_from?" | "eql?"),
                [arg],
            ) if !super::dry_types::reads_self(arg) => {
                out.push((p.to_string(), Some(arg.clone())));
                Ok(())
            }
            _ => Err(format!("predicate `{}`", crate::emit::ruby::emit_expr(expr))),
        }
    }
    let mut out = Vec::new();
    walk(body, &mut out)?;
    Ok(out)
}

fn read_rule(
    macro_name: &str,
    args: &[Expr],
    block: Option<&Expr>,
    processor: Processor,
    owner: &str,
    cx: &Cx<'_>,
    schemas: &HashMap<String, Schema>,
) -> Result<Rule, String> {
    let (positional, options): (Vec<&Expr>, Vec<&Expr>) =
        args.iter().partition(|a| !matches!(&*a.node, ExprNode::Hash { .. }));
    let mut preds: Vec<(String, Option<Expr>)> = Vec::new();
    let mut type_of: Option<Expr> = None;
    for options in options {
        let ExprNode::Hash { entries, .. } = &*options.node else { unreachable!() };
        for (k, v) in entries {
            let ExprNode::Lit { value: Literal::Sym { value: pred } } = &*k.node else {
                return Err("a computed predicate".into());
            };
            let pred = pred.as_str();
            if pred == "type?" {
                type_of = Some(v.clone());
            } else if matches!(
                pred,
                "gt?" | "gteq?" | "lt?" | "lteq?" | "min_size?" | "max_size?" | "size?" | "format?"
                    | "included_in?" | "excluded_from?" | "eql?"
            ) {
                if super::dry_types::reads_self(v) {
                    return Err(format!("`{pred}` read with `self`"));
                }
                preds.push((pred.to_string(), Some(v.clone())));
            } else {
                return Err(format!("predicate `{pred}`"));
            }
        }
    }
    let spec = match positional.as_slice() {
        [] => None,
        [spec] => Some(*spec),
        _ => return Err("more than one type".into()),
    };
    let ty_of = |spec: Option<&Expr>| -> Result<Ty, String> {
        if let Some(t) = &type_of {
            if spec.is_some() {
                return Err("a type and `type?:`".into());
            }
            return Ok(Ty::TypeOf(t.clone()));
        }
        let Some(spec) = spec else { return Ok(Ty::Any) };
        let ExprNode::Lit { value: Literal::Sym { value } } = &*spec.node else {
            let scope = super::dry_types::Scope {
                types_modules: cx.types_modules,
                owner,
                names: cx.names,
                structs: cx.structs,
                constants: cx.constants,
                classes: cx.classes,
                depth: 0,
            };
            return super::dry_types::dry_type(spec, &scope)
                .filter(schema_type)
                .map(|t| Ty::Dry(Box::new(t)))
                .ok_or_else(|| format!("type `{}`", crate::emit::ruby::emit_expr(spec)));
        };
        Ok(match value.as_str() {
            "string" => Ty::Coerced("string"),
            "integer" => Ty::Coerced("integer"),
            "float" => Ty::Coerced("float"),
            "decimal" => Ty::Coerced("decimal"),
            "bool" => Ty::Coerced("bool"),
            "date" => Ty::Coerced("date"),
            "date_time" => Ty::Coerced("date_time"),
            "time" => Ty::Coerced("time"),
            "hash" => Ty::Coerced("hash"),
            "array" => Ty::Coerced("array"),
            "str?" => Ty::Predicate("str?"),
            "int?" => Ty::Predicate("int?"),
            "number?" => Ty::Predicate("number?"),
            "bool?" => Ty::Predicate("bool?"),
            "hash?" => Ty::Predicate("hash?"),
            "array?" => Ty::Predicate("array?"),
            other => return Err(format!("type `:{other}`")),
        })
    };
    let nested_block = |block: Option<&Expr>| -> Result<Option<Schema>, String> {
        block.map(|b| read_schema(processor, b, owner, cx, schemas)).transpose()
    };
    match macro_name {
        "value" | "filled" | "maybe" => {
            let macro_ = match macro_name {
                "value" => Macro::Value,
                "filled" => Macro::Filled,
                _ => Macro::Maybe,
            };
            // A schema where the type goes (`filled(AddressContract.schema)`):
            // a Hash of its keys. `X.schema` can be nothing else, so its
            // failure is the reason; a constant may be a type instead, and
            // the schema's failure is the reason only if it is not.
            let as_schema = match spec.map(|s| &*s.node) {
                Some(ExprNode::Send { method, args, .. }) if method.as_str() == "schema" && args.is_empty() => {
                    Some(schema_ref(spec.unwrap(), owner, cx, schemas))
                }
                Some(ExprNode::Const { .. }) => match schema_ref(spec.unwrap(), owner, cx, schemas) {
                    Ok(found) => Some(Ok(found)),
                    Err(reason) => ty_of(spec).is_err().then_some(Err(reason)),
                },
                _ => None,
            };
            if let Some(referenced) = as_schema {
                let mut schema = referenced?.clone();
                if let Some(s) = nested_block(block)? {
                    schema.extend(&s);
                }
                return Ok(Rule { macro_, ty: Ty::Coerced("hash"), preds, nested: Some(Nested::Hash(schema)), type_first: false });
            }
            let ty = ty_of(spec)?;
            let mut preds = preds;
            // A block on a hash holds its keys; on anything else, more
            // predicates (`value(:string) { min_size?(2) }`).
            let nested = match block {
                Some(b) if matches!(ty, Ty::Coerced("hash")) => nested_block(Some(b))?.map(Nested::Hash),
                Some(b) => {
                    preds.extend(predicate_block(b)?);
                    None
                }
                None => None,
            };
            Ok(Rule { macro_, ty, preds, nested, type_first: false })
        }
        // `hash do ... end`, `hash(Schema)`, `schema do ... end`.
        "hash" | "schema" => {
            let mut schema = Schema { processor, keys: Vec::new() };
            if let Some(spec) = spec {
                let referenced = schema_ref(spec, owner, cx, schemas)?;
                schema.processor = referenced.processor;
                schema.extend(referenced);
            }
            if let Some(s) = nested_block(block)? {
                schema.extend(&s);
            }
            if spec.is_none() && block.is_none() {
                return Ok(Rule { macro_: Macro::Value, ty: Ty::Coerced("hash"), preds, nested: None, type_first: false });
            }
            Ok(Rule { macro_: Macro::Value, ty: Ty::Coerced("hash"), preds, nested: Some(Nested::Hash(schema)), type_first: false })
        }
        // `array(:integer)`, `array(:hash) do ... end`, `array(Schema)`.
        "array" => {
            let member = match spec {
                // A schema object checks no `hash?` first: a member that is
                // not a Hash fails in the gem's `key?`, as it does here.
                Some(spec) if matches!(&*spec.node, ExprNode::Const { .. }) => {
                    let referenced = schema_ref(spec, owner, cx, schemas)?;
                    Rule {
                        macro_: Macro::Value,
                        ty: Ty::Any,
                        preds: Vec::new(),
                        nested: Some(Nested::Hash(referenced.clone())),
                        type_first: false,
                    }
                }
                spec => {
                    let ty = match spec {
                        Some(_) => ty_of(spec)?,
                        None => Ty::Any,
                    };
                    let nested = match nested_block(block)? {
                        Some(schema) if matches!(ty, Ty::Coerced("hash") | Ty::Any) => Some(Nested::Hash(schema)),
                        Some(_) => return Err("a block on `array` of another type".into()),
                        None => None,
                    };
                    let ty = if nested.is_some() { Ty::Coerced("hash") } else { ty };
                    Rule { macro_: Macro::Value, ty, preds: Vec::new(), nested, type_first: false }
                }
            };
            Ok(Rule { macro_: Macro::Value, ty: Ty::Coerced("array"), preds, nested: Some(Nested::Array(Box::new(member))), type_first: false })
        }
        other => Err(format!("`{other}`")),
    }
}

/// `config.messages.*` in each class body, read from the source: ingest
/// does not keep a call on `config`.
fn read_configs(sources: &[crate::span::SourceFile]) -> HashMap<String, Config> {
    struct Finder {
        nesting: Vec<String>,
        found: HashMap<String, Config>,
    }
    fn text(node: &ruby_prism::Node<'_>) -> String {
        String::from_utf8_lossy(node.location().as_slice()).to_string()
    }
    fn value(node: &ruby_prism::Node<'_>) -> Option<ConfigValue> {
        if let Some(s) = node.as_string_node() {
            return Some(ConfigValue::Literal(String::from_utf8_lossy(s.unescaped()).to_string()));
        }
        if let Some(s) = node.as_symbol_node() {
            return Some(ConfigValue::Literal(String::from_utf8_lossy(s.unescaped()).to_string()));
        }
        if node.as_constant_read_node().is_some() || node.as_constant_path_node().is_some() {
            return Some(ConfigValue::Constant(text(node).trim_start_matches("::").to_string()));
        }
        None
    }
    impl<'pr> ruby_prism::Visit<'pr> for Finder {
        fn visit_module_node(&mut self, node: &ruby_prism::ModuleNode<'pr>) {
            self.nesting.push(text(&node.constant_path()).trim_start_matches("::").to_string());
            ruby_prism::visit_module_node(self, node);
            self.nesting.pop();
        }
        fn visit_class_node(&mut self, node: &ruby_prism::ClassNode<'pr>) {
            self.nesting.push(text(&node.constant_path()).trim_start_matches("::").to_string());
            ruby_prism::visit_class_node(self, node);
            self.nesting.pop();
        }
        // A method's `config` is not the class's.
        fn visit_def_node(&mut self, _node: &ruby_prism::DefNode<'pr>) {}
        fn visit_call_node(&mut self, node: &ruby_prism::CallNode<'pr>) {
            let Some(recv) = node.receiver() else {
                ruby_prism::visit_call_node(self, node);
                return;
            };
            let recv_text = text(&recv);
            if !(recv_text == "config" || recv_text.starts_with("config.")) || self.nesting.is_empty() {
                ruby_prism::visit_call_node(self, node);
                return;
            }
            let owner = self.nesting.join("::");
            let config = self.found.entry(owner).or_default();
            let method = String::from_utf8_lossy(node.name().as_slice()).to_string();
            let args: Vec<ruby_prism::Node<'_>> =
                node.arguments().map(|a| a.arguments().iter().collect()).unwrap_or_default();
            match (recv_text.as_str(), method.as_str(), args.as_slice()) {
                ("config.messages", "top_namespace=", [v]) => match value(v) {
                    Some(v) => config.top_namespace = Some(v),
                    None => config.refused = Some("a computed `top_namespace`".into()),
                },
                ("config.messages", "default_locale=", [v]) => match value(v) {
                    Some(ConfigValue::Literal(v)) => config.default_locale = Some(v),
                    _ => config.refused = Some("a computed `default_locale`".into()),
                },
                ("config.messages.load_paths", "<<", [v]) => match value(v) {
                    // Read from the app only: an absolute path or one
                    // climbing out of it is not followed.
                    Some(ConfigValue::Literal(v))
                        if std::path::Path::new(&v).components().any(|c| {
                            !matches!(c, std::path::Component::Normal(_) | std::path::Component::CurDir)
                        }) =>
                    {
                        config.refused = Some(format!("`load_paths` entry `{v}` outside the app"))
                    }
                    Some(ConfigValue::Literal(v)) => config.load_paths.push(v),
                    _ => config.refused = Some("a computed `load_paths` entry".into()),
                },
                _ => {
                    config.refused = Some(format!("`{}`", text(&node.as_node()).lines().next().unwrap_or_default()))
                }
            }
        }
    }
    let mut finder = Finder { nesting: Vec::new(), found: HashMap::new() };
    for source in sources.iter().filter(|s| s.text.contains("config.")) {
        let parsed = super::prism::parse_silent(source.text.as_bytes());
        ruby_prism::Visit::visit(&mut finder, &parsed.node());
    }
    finder.found
}

/// The messages a contract reads: its chain's settings, a child's over a
/// parent's, load paths added along it.
fn messages_for(
    chain: &[String],
    configs: &HashMap<String, Config>,
    constants: &HashMap<String, Expr>,
    read_file: &dyn Fn(&str) -> Option<String>,
    default_namespace: &str,
) -> Messages {
    let mut merged = Config::default();
    let mut holder = None;
    for name in chain {
        let Some(c) = configs.get(name) else { continue };
        if c.top_namespace.is_some() {
            merged.top_namespace = c.top_namespace.clone();
            holder = Some(name.as_str());
        }
        if c.default_locale.is_some() {
            merged.default_locale = c.default_locale.clone();
        }
        merged.load_paths.extend(c.load_paths.iter().cloned());
    }
    // A namespace constant, as Ruby finds it from the class that set it.
    if let (Some(ConfigValue::Constant(name)), Some(holder)) = (&merged.top_namespace, holder) {
        let mut scope: Vec<&str> = holder.split("::").collect();
        let mut found = None;
        loop {
            let candidate = if scope.is_empty() { name.clone() } else { format!("{}::{name}", scope.join("::")) };
            if let Some(v) = constants.get(&candidate) {
                found = Some(v.clone());
                break;
            }
            if scope.is_empty() {
                break;
            }
            scope.pop();
        }
        merged.top_namespace = found.and_then(|v| match &*v.node {
            ExprNode::Lit { value: Literal::Str { value } } => Some(ConfigValue::Literal(value.clone())),
            ExprNode::Lit { value: Literal::Sym { value } } => Some(ConfigValue::Literal(value.as_str().to_string())),
            _ => None,
        });
    }
    let namespace = match &merged.top_namespace {
        Some(ConfigValue::Literal(ns)) => Some(ns.clone()),
        _ => None,
    };
    Messages::new(&merged, namespace, read_file, default_namespace)
}

/// dry-schema's YAML backend, as configured.
struct Messages {
    data: HashMap<String, String>,
    root: String,
    namespace: String,
}

impl Messages {
    fn new(
        config: &Config,
        namespace: Option<String>,
        read_file: &dyn Fn(&str) -> Option<String>,
        default_namespace: &str,
    ) -> Messages {
        let namespace = namespace.unwrap_or_else(|| default_namespace.to_string());
        let locale = config.default_locale.clone().unwrap_or_else(|| "en".into());
        let mut data = HashMap::new();
        // dry-schema's own file under any other namespace than its own.
        for (k, v) in flat_yaml(DRY_SCHEMA_ERRORS) {
            data.insert(k.replace("dry_schema", &namespace), v);
        }
        if default_namespace == "dry_validation" {
            data.extend(flat_yaml(DRY_VALIDATION_ERRORS));
        }
        for path in &config.load_paths {
            if let Some(text) = read_file(path) {
                data.extend(flat_yaml(&text));
            }
        }
        Messages { data, root: format!("{locale}.{namespace}.errors"), namespace }
    }

    /// The word joining the messages of a sum's sides.
    fn or_word(&self) -> String {
        let locale = self.root.split('.').next().unwrap_or("en");
        self.data.get(&format!("{locale}.{}.or", self.namespace)).cloned().unwrap_or_else(|| "or".into())
    }

    /// The template for `predicate` on the key `path`, as dry-schema's
    /// lookup paths find it.
    fn template(&self, predicate: &str, path: &str, arg_type: &str, val_type: &str) -> Option<&str> {
        let root = &self.root;
        [
            format!("{root}.rules.{path}.{predicate}.arg.{arg_type}"),
            format!("{root}.rules.{path}.{predicate}"),
            format!("{root}.{predicate}.failure"),
            format!("{root}.{predicate}.value.{path}"),
            format!("{root}.{predicate}.value.{val_type}.arg.{arg_type}"),
            format!("{root}.{predicate}.value.{val_type}"),
            format!("{root}.{predicate}.arg.{arg_type}"),
            format!("{root}.{predicate}"),
        ]
        .into_iter()
        .find_map(|key| self.data.get(&key).map(|s| s.as_str()))
    }
}

/// dry-schema's `flat_hash`: dotted keys to texts; a Hash with a `text`
/// is that text.
fn flat_yaml(text: &str) -> Vec<(String, String)> {
    fn walk(value: &serde_yaml_ng::Value, path: &mut Vec<String>, out: &mut Vec<(String, String)>) {
        let serde_yaml_ng::Value::Mapping(map) = value else { return };
        for (k, v) in map {
            let key = match k {
                serde_yaml_ng::Value::String(s) => s.clone(),
                serde_yaml_ng::Value::Number(n) => n.to_string(),
                serde_yaml_ng::Value::Bool(b) => b.to_string(),
                _ => continue,
            };
            path.push(key.clone());
            match v {
                serde_yaml_ng::Value::String(s) if key != "text" => out.push((path.join("."), s.clone())),
                serde_yaml_ng::Value::Mapping(m) => {
                    if let Some(serde_yaml_ng::Value::String(t)) = m.get("text") {
                        out.push((path.join("."), t.clone()));
                    }
                    walk(v, path, out);
                }
                _ => {}
            }
            path.pop();
        }
    }
    let Ok(doc) = serde_yaml_ng::from_str::<serde_yaml_ng::Value>(text) else { return Vec::new() };
    let mut out = Vec::new();
    walk(&doc, &mut Vec::new(), &mut out);
    out
}

/// A Ruby String literal of a message template, as dry-schema's YAML
/// backend makes one: it turns `%` into `#` and evaluates `"#{text}"`, so
/// a token (`%{num}`) is interpolated from the expression given, `#@x`
/// and `#@@x` read nothing (nil), `#$x` reads the global and any other
/// `#{...}` is Ruby, run where the message is built.
/// ponytail: a backslash stays literal; the gem would read it as an
/// escape, which a message file has not been seen to rely on.
fn template_literal(template: &str, tokens: &HashMap<String, String>) -> String {
    let text = template.replace('%', "#");
    let ident = |s: &str| s.find(|c: char| !(c.is_alphanumeric() || c == '_')).unwrap_or(s.len());
    let mut out = String::from("\"");
    let mut rest = text.as_str();
    while let Some(at) = rest.find('#') {
        out.push_str(&escape(&rest[..at]));
        let after = &rest[at + 1..];
        if let Some(body) = after.strip_prefix('{')
            && let Some(end) = body.find('}')
        {
            let inner = &body[..end];
            match tokens.get(inner) {
                Some(expr) => out.push_str(&format!("#{{{expr}}}")),
                None => out.push_str(&format!("#{{{inner}}}")),
            }
            rest = &body[end + 1..];
        } else if let Some(name) = after.strip_prefix("@@").or_else(|| after.strip_prefix('@'))
            && ident(name) > 0
        {
            rest = &name[ident(name)..];
        } else if let Some(name) = after.strip_prefix('$')
            && ident(name) > 0
        {
            out.push_str(&format!("#{{${}}}", &name[..ident(name)]));
            rest = &name[ident(name)..];
        } else {
            out.push_str("\\#");
            rest = after;
        }
    }
    out.push_str(&escape(rest));
    out.push('"');
    out
}

/// A Ruby String literal of `text` as written: nothing in it interpolates.
fn literal(text: &str) -> String {
    format!("\"{}\"", escape(text))
}

fn escape(s: &str) -> String {
    // Every `#`: `#@ivar`, `#@@cvar` and `#$global` interpolate too.
    s.replace('\\', "\\\\").replace('"', "\\\"").replace('#', "\\#")
}

/// What generates one class's methods.
struct Gen<'a> {
    messages: &'a Messages,
    helpers: Vec<String>,
    /// The `Dry::Schema::Lowered` methods called.
    used: &'a mut HashSet<&'static str>,
}

impl Gen<'_> {
    /// The message for `predicate` failing on `value` at `path`, as an
    /// expression: a template chosen by the input's class where the files
    /// tell them apart, or the gem's `MissingMessageError`.
    fn message(&self, predicate: &str, path: &str, args: &[(&str, &Expr)], value: &str) -> String {
        let arg = args.first().map(|(_, e)| *e);
        let arg_range = arg.map(|e| match &*e.node {
            ExprNode::Range { .. } => Some(true),
            ExprNode::Lit { .. } | ExprNode::Array { .. } => Some(false),
            _ => None,
        });
        let pick = |arg_type: &str, val_type: &str| -> String {
            match self.messages.template(predicate, path, arg_type, val_type) {
                Some(t) => {
                    let mut tokens: HashMap<String, String> = HashMap::new();
                    for (name, e) in args {
                        let src = crate::emit::ruby::emit_expr(e);
                        if *name == "type" {
                            tokens.insert("type".into(), src);
                        } else if arg_type == "range" {
                            tokens.insert(format!("{name}_left"), format!("({src}).first"));
                            tokens.insert(format!("{name}_right"), format!("({src}).last"));
                        } else {
                            tokens.insert(name.to_string(), format!("dry_schema_token({src})"));
                        }
                    }
                    // dry-schema's stopgap: a `size` token is a `num` too.
                    let mapped: Vec<(String, String)> = tokens
                        .iter()
                        .filter(|(k, _)| k.contains("size"))
                        .map(|(k, v)| (k.replace("size", "num"), v.clone()))
                        .collect();
                    tokens.extend(mapped);
                    template_literal(t, &tokens)
                }
                None => format!(
                    "raise(::Dry::Schema::MissingMessageError, {})",
                    literal(&format!("Message template for :{path} under \"\" was not found"))
                ),
            }
        };
        let by_val = |arg_type: &str| -> String {
            let string = pick(arg_type, "string");
            let range = pick(arg_type, "range");
            let default = pick(arg_type, "default");
            if string == default && range == default {
                default
            } else {
                format!("({value}.is_a?(String) ? {string} : ({value}.is_a?(Range) ? {range} : {default}))")
            }
        };
        match arg_range {
            None | Some(Some(false)) => by_val("default"),
            Some(Some(true)) => by_val("range"),
            Some(None) => {
                let range = by_val("range");
                let default = by_val("default");
                if range == default {
                    default
                } else {
                    let src = crate::emit::ruby::emit_expr(arg.unwrap());
                    format!("(({src}).is_a?(Range) ? {range} : {default})")
                }
            }
        }
    }

    /// A rule's predicates as (test, message) pairs.
    fn preds_conds(&mut self, rule: &Rule, path: &str, value: &str) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = Vec::new();
        for (pred, arg) in &rule.preds {
            let Some(arg) = arg else {
                let test = self.unary(pred, value);
                let message = self.message(pred, path, &[], value);
                out.push((test, message));
                continue;
            };
            let a = crate::emit::ruby::emit_expr(arg);
            let (test, token) = match pred.as_str() {
                "gt?" => (format!("{value} > ({a})"), "num"),
                "gteq?" => (format!("!({value} < ({a}))"), "num"),
                "lt?" => (format!("{value} < ({a})"), "num"),
                "lteq?" => (format!("!({value} > ({a}))"), "num"),
                "min_size?" => (format!("{value}.size >= ({a})"), "num"),
                "max_size?" => (format!("{value}.size <= ({a})"), "num"),
                "size?" => {
                    self.use_helper("size?");
                    (format!("::Dry::Schema::Lowered.size?(({a}), {value})"), "size")
                }
                "format?" => (format!("(!{value}.nil? && ({a}).match?({value}))"), "regex"),
                "included_in?" => (format!("({a}).include?({value})"), "list"),
                "excluded_from?" => (format!("!({a}).include?({value})"), "list"),
                _ => (format!("({a}).eql?({value})"), "left"),
            };
            let message = self.message(pred, path, &[(token, arg)], value);
            out.push((test, message));
        }
        out
    }

    /// The text a rule's `failure` writes: a String as given, an id looked
    /// up as dry-validation's resolver looks it up (`rules.<id>` for
    /// `base`), its tokens interpolated.
    fn failure_message(&mut self, failure: Failure<'_>) -> String {
        let (id, tokens) = match failure.message {
            FailureMessage::Text(text) => return literal(text),
            FailureMessage::Id(id, tokens) => (id, tokens),
        };
        let tokens: HashMap<String, String> = tokens
            .iter()
            .map(|(k, v)| (k.clone(), format!("dry_schema_token({})", crate::emit::ruby::emit_expr(v))))
            .collect();
        let template = match &failure.path {
            None => self.messages.template(&format!("rules.{id}"), "", "default", "default").map(str::to_string),
            Some(path) => self
                .messages
                .template(id, &path.join("."), "default", "default")
                .or_else(|| self.messages.template(id, path.last().map_or("", |s| s.as_str()), "default", "default"))
                .map(str::to_string),
        };
        match template {
            Some(t) => template_literal(&t, &tokens),
            None => format!(
                "raise(::Dry::Validation::MissingMessageError, {})",
                literal(&format!(
                    "Message template for :{id} under \"{}\" was not found",
                    failure.path.map(|p| p.join(".")).unwrap_or_default()
                ))
            ),
        }
    }

    /// The test a unary predicate is.
    fn unary(&mut self, pred: &str, value: &str) -> String {
        match pred {
            "str?" => format!("{value}.is_a?(String)"),
            "int?" => format!("{value}.is_a?(Integer)"),
            "float?" => format!("{value}.is_a?(Float)"),
            "decimal?" => format!("{value}.is_a?(BigDecimal)"),
            "bool?" => format!("({value} == true || {value} == false)"),
            "date?" => format!("{value}.is_a?(Date)"),
            "date_time?" => format!("{value}.is_a?(DateTime)"),
            "time?" => format!("{value}.is_a?(Time)"),
            "hash?" => format!("{value}.is_a?(Hash)"),
            "array?" => format!("{value}.is_a?(Array)"),
            "nil?" => format!("{value}.nil?"),
            "filled?" => {
                self.use_helper("filled?");
                format!("::Dry::Schema::Lowered.filled?({value})")
            }
            _ => {
                self.use_helper("number?");
                format!("::Dry::Schema::Lowered.number?({value})")
            }
        }
    }

    /// A `Dry::Schema::Lowered` method, and those it calls.
    fn use_helper(&mut self, name: &'static str) {
        if self.used.insert(name) {
            for dep in LOWERED.iter().find(|(n, _, _)| *n == name).map_or(&[][..], |(_, _, deps)| deps) {
                self.use_helper(dep);
            }
        }
    }

    /// The coerced `value` as `processor` coerces this rule's type.
    fn coerce(&mut self, processor: Processor, rule: &Rule, value: &str) -> String {
        if let Ty::Dry(dry) = &rule.ty {
            use super::dry_types::Base;
            let value = if rule.macro_ == Macro::Maybe && processor == Processor::Params {
                self.use_helper("params_nil");
                format!("::Dry::Schema::Lowered.params_nil({value})")
            } else {
                value.to_string()
            };
            let helper = match &dry.base {
                Base::Coerce("String") => "coerce_string",
                Base::Coerce("Integer") => "coerce_integer",
                Base::Coerce("Float") => "coerce_float",
                Base::ParamsInteger => "params_integer",
                Base::ParamsBool => "params_bool",
                Base::ParamsDecimal => "params_decimal",
                Base::Parse("Date") => "json_date",
                Base::Parse("DateTime") => "json_date_time",
                Base::Parse("Time") => "json_time",
                _ => return value,
            };
            self.use_helper(helper);
            return format!("::Dry::Schema::Lowered.{helper}({value})");
        }
        let Ty::Coerced(ty) = &rule.ty else { return value.to_string() };
        // `maybe` under `params` reads an empty String as nil first.
        let value = if rule.macro_ == Macro::Maybe && processor == Processor::Params {
            self.use_helper("params_nil");
            format!("::Dry::Schema::Lowered.params_nil({value})")
        } else {
            value.to_string()
        };
        let value = value.as_str();
        let helper = match (processor, *ty) {
            (Processor::Schema, _) | (Processor::Params, "string") => return value.to_string(),
            (Processor::Json, "date" | "date_time" | "time" | "decimal") => coercion(&format!("json_{ty}")),
            (Processor::Json, _) => return value.to_string(),
            (Processor::Params, _) => coercion(&format!("params_{ty}")),
        };
        self.use_helper(helper);
        format!("::Dry::Schema::Lowered.{helper}({value})")
    }

    /// Statements checking `value` (already coerced) at `path` and writing
    /// what it gives into `values[slot]` / `errors[slot]`.
    #[allow(clippy::too_many_arguments)]
    fn check(
        &mut self,
        processor: Processor,
        rule: &Rule,
        path: &str,
        value: &str,
        values: &str,
        errors: &str,
        slot: &str,
        depth: usize,
    ) -> String {
        let mut conds: Vec<(String, String)> = Vec::new();
        if rule.macro_ == Macro::Filled {
            self.use_helper("filled?");
            conds.push((format!("::Dry::Schema::Lowered.filled?({value})"), self.message("filled?", path, &[], value)));
        }
        let type_check = |g: &Gen<'_>, pred: &str, test: String| (test, g.message(pred, path, &[], value));
        let filled_at = conds.len();
        match &rule.ty {
            Ty::Coerced(t) => {
                let (pred, test) = match *t {
                    "string" => ("str?", format!("{value}.is_a?(String)")),
                    "integer" => ("int?", format!("{value}.is_a?(Integer)")),
                    "float" => ("float?", format!("{value}.is_a?(Float)")),
                    "decimal" => ("decimal?", format!("{value}.is_a?(BigDecimal)")),
                    "bool" => ("bool?", format!("({value} == true || {value} == false)")),
                    "date" => ("date?", format!("{value}.is_a?(Date)")),
                    "date_time" => ("date_time?", format!("{value}.is_a?(DateTime)")),
                    "time" => ("time?", format!("{value}.is_a?(Time)")),
                    "hash" => ("hash?", format!("{value}.is_a?(Hash)")),
                    _ => ("array?", format!("{value}.is_a?(Array)")),
                };
                conds.push(type_check(self, pred, test));
            }
            Ty::Predicate(p) => {
                let test = self.unary(p, value);
                conds.push(type_check(self, p, test));
            }
            Ty::TypeOf(class) => {
                let src = crate::emit::ruby::emit_expr(class);
                conds.push((format!("{value}.is_a?({src})"), self.message("type?", path, &[("type", class)], value)));
            }
            Ty::Dry(dry) => {
                use super::dry_types::Base;
                match (dry_primitive(&dry.base), &dry.base) {
                    (Some("type?"), Base::Instance(class)) => {
                        let class_expr = parse_expr(class).unwrap_or_else(|| Expr::new(
                            crate::span::Span::synthetic(),
                            ExprNode::Const { path: vec![Symbol::from(class.trim_start_matches("::"))] },
                        ));
                        conds.push((format!("{value}.is_a?({class})"), self.message("type?", path, &[("type", &class_expr)], value)));
                    }
                    (Some("type_sum"), Base::Sum(members)) => {
                        let classes: Vec<&str> = members
                            .iter()
                            .filter_map(|m| match &m.base {
                                Base::Instance(c) => Some(c.as_str()),
                                _ => None,
                            })
                            .collect();
                        let test = classes.iter().map(|c| format!("{value}.is_a?({c})")).collect::<Vec<_>>().join(" || ");
                        let messages: Vec<String> = classes
                            .iter()
                            .map(|c| {
                                let class_expr = parse_expr(c).expect("a class name parses");
                                self.message("type?", path, &[("type", &class_expr)], value)
                            })
                            .collect();
                        let or = self.messages.or_word();
                        let joined = messages.join(&format!(" + \" {} \" + ", escape(&or)));
                        conds.push((format!("({test})"), format!("({joined})")));
                    }
                    (Some(""), _) | (None, _) => {}
                    (Some(p), _) => {
                        let test = self.unary(p, value);
                        conds.push(type_check(self, p, test));
                    }
                }
                if let Some(values) = &dry.enumeration {
                    let list = Expr::new(
                        crate::span::Span::synthetic(),
                        ExprNode::Array { elements: values.clone(), style: Default::default() },
                    );
                    let src = crate::emit::ruby::emit_expr(&list);
                    conds.push((format!("({src}).include?({value})"), self.message("included_in?", path, &[("list", &list)], value)));
                }
                let constraints: Vec<(String, Option<Expr>)> = dry
                    .constraints
                    .iter()
                    .map(|(p, arg)| if p.as_str() == "filled" { ("filled?".to_string(), None) } else { (format!("{}?", p.as_str()), Some(arg.clone())) })
                    .collect();
                let rule = Rule { macro_: Macro::Value, ty: Ty::Any, preds: constraints, nested: None, type_first: false };
                // The constraints read like a predicate list.
                let inner = self.preds_conds(&rule, path, value);
                conds.extend(inner);
            }
            Ty::Any => {}
        }
        if rule.type_first && filled_at == 1 && conds.len() > 1 {
            let type_cond = conds.remove(1);
            conds.insert(0, type_cond);
        }
        conds.extend(self.preds_conds(rule, path, value));
        let failed = format!("dry_schema_message_{depth}");
        let _ = (values, errors);
        let mut out = String::new();
        if conds.is_empty() {
            out.push_str(&format!("{failed} = nil\n"));
        } else {
            out.push_str(&format!("{failed} = if !({})\n  {}\n", conds[0].0, conds[0].1));
            for (test, message) in &conds[1..] {
                out.push_str(&format!("elsif !({test})\n  {message}\n"));
            }
            out.push_str("end\n");
        }
        let optional_type = matches!(&rule.ty, Ty::Dry(dry) if dry.optional);
        if rule.macro_ == Macro::Maybe || (optional_type && rule.macro_ == Macro::Value) {
            out = format!("{failed} = nil\nunless {value}.nil?\n{out}end\n");
        }
        // The value passed: what it holds is checked in turn.
        let nested = match &rule.nested {
            None => None,
            Some(Nested::Hash(schema)) => {
                let helper = self.schema_helper(schema);
                Some(format!(
                    "dry_schema_values_{depth} = {{}}\ndry_schema_errors_{depth} = {{}}\n{helper}({value}, dry_schema_values_{depth}, dry_schema_errors_{depth})\n{values}[{slot}] = dry_schema_values_{depth}\n{errors}[{slot}] = dry_schema_errors_{depth} unless dry_schema_errors_{depth}.empty?\n"
                ))
            }
            Some(Nested::Array(member)) => {
                let m = format!("dry_schema_member_{depth}");
                let index = format!("dry_schema_index_{depth}");
                let coerced = self.coerce(processor, member, &m);
                let inner = self.check(
                    processor,
                    member,
                    "0",
                    &m,
                    &format!("dry_schema_members_{depth}"),
                    &format!("dry_schema_member_errors_{depth}"),
                    &index,
                    depth + 1,
                );
                Some(format!(
                    "dry_schema_members_{depth} = []\ndry_schema_member_errors_{depth} = {{}}\n{value}.each_with_index do |{m}, {index}|\n  {m} = {coerced}\n{inner}end\n"
                ))
            }
        };
        match nested {
            None => {
                out.push_str(&format!("{values}[{slot}] = {value}\n{errors}[{slot}] = [{failed}] if {failed}\n"));
            }
            Some(body) => {
                let array = matches!(rule.nested, Some(Nested::Array(_)));
                let finish = if array {
                    format!(
                        "{values}[{slot}] = dry_schema_members_{depth}\n{errors}[{slot}] = dry_schema_member_errors_{depth} unless dry_schema_member_errors_{depth}.empty?\n"
                    )
                } else {
                    String::new()
                };
                // nil passes `maybe` (and an optional type) unchecked.
                let nil = if rule.macro_ == Macro::Maybe || optional_type {
                    format!("elsif {value}.nil?\n  {values}[{slot}] = nil\n")
                } else {
                    String::new()
                };
                out.push_str(&format!(
                    "if {failed}\n  {values}[{slot}] = {value}\n  {errors}[{slot}] = [{failed}]\n{nil}else\n{body}{finish}end\n"
                ));
            }
        }
        out
    }

    /// A private method filling `values` and `errors` from `input` by
    /// `schema`, named by its index.
    fn schema_helper(&mut self, schema: &Schema) -> String {
        let name = format!("dry_schema_{}", self.helpers.len());
        self.helpers.push(String::new());
        let mut body = String::new();
        for key in &schema.keys {
            let k = &key.name;
            let fetch = match schema.processor {
                Processor::Schema => format!("if input.key?(:{k})\n  dry_schema_value = input[:{k}]\n"),
                _ => format!(
                    "if input.key?(\"{k}\") || input.key?(:{k})\n  dry_schema_value = input.key?(\"{k}\") ? input[\"{k}\"] : input[:{k}]\n"
                ),
            };
            body.push_str(&fetch);
            body.push_str(&format!("  dry_schema_value = {}\n", self.coerce(schema.processor, &key.rule, "dry_schema_value")));
            body.push_str(&self.check(
                schema.processor,
                &key.rule,
                k,
                "dry_schema_value",
                "values",
                "errors",
                &format!(":{k}"),
                0,
            ));
            // A dry type's default stands in for a missing key, required
            // or not.
            if let Ty::Dry(dry) = &key.rule.ty
                && let Some(default) = &dry.default
            {
                let value = match default {
                    super::dry_types::DefaultValue::Value(v) => crate::emit::ruby::emit_expr(v),
                    super::dry_types::DefaultValue::Block(b) => format!("(begin\n{}\nend)", crate::emit::ruby::emit_expr(b)),
                };
                body.push_str(&format!("else\n  values[:{k}] = {value}\n"));
            } else if key.required {
                let missing = self.message("key?", k, &[], "nil");
                body.push_str(&format!("else\n  errors[:{k}] = [{missing}]\n"));
            }
            body.push_str("end\n");
        }
        let slot = self.helpers.iter().position(|h| h.is_empty()).expect("reserved");
        self.helpers[slot] = format!("  def {name}(input, values, errors)\n{body}    values\n  end\n");
        name
    }
}

/// A contract class's `call` and its helpers.
fn contract_source(
    owner: &str,
    schema: &Schema,
    rules: &[RuleDecl],
    methods: &HashSet<String>,
    messages: &Messages,
    used: &mut HashSet<&'static str>,
) -> String {
    class_source(owner, schema, rules, methods, messages, "::Dry::Validation::Result", used)
}

fn schema_source(owner: &str, schema: &Schema, messages: &Messages, used: &mut HashSet<&'static str>) -> String {
    class_source(owner, schema, &[], &HashSet::new(), messages, "::Dry::Schema::Result", used)
}

#[allow(clippy::too_many_arguments)]
fn class_source(
    owner: &str,
    schema: &Schema,
    rules: &[RuleDecl],
    methods: &HashSet<String>,
    messages: &Messages,
    result: &str,
    used: &mut HashSet<&'static str>,
) -> String {
    let mut g = Gen { messages, helpers: Vec::new(), used };
    let top = g.schema_helper(schema);
    let _ = &messages.namespace;
    // The rules, each skipped where the schema failed one of its keys.
    let mut run_rules = String::new();
    if !rules.is_empty() {
        for helper in ["deep_dup", "error_at?", "add_error", "key_at?"] {
            g.use_helper(helper);
        }
        run_rules.push_str("    dry_rule_schema_errors = ::Dry::Schema::Lowered.deep_dup(errors)\n");
    }
    for (i, rule) in rules.iter().enumerate() {
        let body = rewrite_rule(rule, methods, &mut |f| g.failure_message(f)).expect("checked when read");
        g.helpers.push(format!(
            "  def dry_rule_{i}(dry_rule_values, dry_rule_errors, dry_rule_schema_errors)\n{}\n  end\n",
            crate::emit::ruby::emit_expr(&body)
        ));
        let guards: Vec<String> = rule
            .paths
            .iter()
            .map(|p| {
                format!(
                    "::Dry::Schema::Lowered.error_at?(dry_rule_schema_errors, [{}])",
                    p.iter().map(|k| format!(":{k}")).collect::<Vec<_>>().join(", ")
                )
            })
            .collect();
        run_rules.push_str(&format!(
            "    dry_rule_{i}(values, errors, dry_rule_schema_errors) unless {}\n",
            guards.join(" || ")
        ));
    }
    let mut out = format!("class {}\n", owner.rsplit("::").next().unwrap_or(owner));
    out.push_str(&format!(
        "  def call(input)\n    raise(ArgumentError, \"Input must be a Hash. #{{input.class}} was given.\") unless input.is_a?(Hash)\n    values = {{}}\n    errors = {{}}\n    {top}(input, values, errors)\n{run_rules}    {result}.new(values, errors)\n  end\n\n  private\n\n"
    ));
    out.push_str("  def dry_schema_token(value)\n    value.is_a?(Array) ? value.join(\", \") : value\n  end\n\n");
    for helper in &g.helpers {
        out.push_str(helper);
        out.push('\n');
    }
    out.push_str("end\n");
    out
}

fn parse_classes(source: &str) -> Option<Vec<LibraryClass>> {
    let (parsed, diags) = crate::ingest::prism::scope(|| {
        crate::ingest::ingest_library_classes(source.as_bytes(), "<dry_validation>")
    });
    match parsed {
        Ok(classes) if diags.is_empty() => Some(classes),
        Ok(_) => None,
        Err(err) => {
            survey::record(&err);
            None
        }
    }
}

fn parse_methods(owner: &str, source: &str) -> Option<Vec<crate::dialect::MethodDef>> {
    let classes = parse_classes(source);
    if classes.is_none() {
        survey::record(&IngestError::Unsupported {
            file: owner.to_string(),
            message: "Dry::Validation lowering did not parse".into(),
        });
    }
    Some(classes?.into_iter().flat_map(|c| c.methods).collect())
}

fn parse_expr(source: &str) -> Option<Expr> {
    let wrapped = format!("class DryValidationExpr\n  X = {source}\nend\n");
    let classes = parse_classes(&wrapped)?;
    classes.into_iter().next()?.constants.into_iter().next().map(|(_, v)| v)
}

/// `Dry::Schema::Lowered`'s methods: name, definition, the ones it calls.
/// Only those a lowered schema calls are emitted: a target without
/// `Time.parse` has no use for it otherwise.
const LOWERED: &[(&str, &str, &[&str])] = &[
    ("params_nil", "def self.params_nil(value)\n  value == \"\" ? nil : value\nend", &[]),
    (
        "add_error",
        "def self.add_error(errors, path, text)\n  node = errors\n  path[0...-1].each { |key| node = (node[key] ||= {}) }\n  (node[path.last] ||= []) << text\n  errors\nend",
        &[],
    ),
    (
        "error_at?",
        "def self.error_at?(errors, path)\n  node = errors\n  path.each do |key|\n    return true if node.is_a?(Array)\n    return false unless node.is_a?(Hash) && node.key?(key)\n    node = node[key]\n  end\n  true\nend",
        &[],
    ),
    (
        "key_at?",
        "def self.key_at?(values, path)\n  node = values\n  path.each do |key|\n    return false unless node.is_a?(Hash) && node.key?(key)\n    node = node[key]\n  end\n  true\nend",
        &[],
    ),
    (
        "deep_dup",
        "def self.deep_dup(errors)\n  errors.transform_values { |value| value.is_a?(Hash) ? deep_dup(value) : value.dup }\nend",
        &[],
    ),
    ("coerce_string", "def self.coerce_string(value)\n  String(value)\nrescue ArgumentError, TypeError\n  value\nend", &[]),
    (
        "coerce_integer",
        "def self.coerce_integer(value)\n  Integer(value)\nrescue ArgumentError, TypeError, FloatDomainError\n  value\nend",
        &[],
    ),
    ("coerce_float", "def self.coerce_float(value)\n  Float(value)\nrescue ArgumentError, TypeError\n  value\nend", &[]),
    (
        "params_integer",
        "def self.params_integer(value)\n  value.is_a?(String) ? Integer(value, 10) : Integer(value)\nrescue ArgumentError, TypeError, FloatDomainError\n  value\nend",
        &[],
    ),
    (
        "params_float",
        "def self.params_float(value)\n  Float(value)\nrescue ArgumentError, TypeError\n  value\nend",
        &[],
    ),
    ("params_decimal", "def self.params_decimal(value)\n  json_decimal(value)\nend", &["json_decimal"]),
    (
        "params_bool",
        "def self.params_bool(value)\n  text = value.to_s\n  return true if %w[1 on On ON t true True TRUE T y yes Yes YES Y].include?(text)\n  return false if %w[0 off Off OFF f false False FALSE F n no No NO N].include?(text)\n  value\nend",
        &[],
    ),
    ("params_date", "def self.params_date(value)\n  json_date(value)\nend", &["json_date"]),
    ("params_date_time", "def self.params_date_time(value)\n  json_date_time(value)\nend", &["json_date_time"]),
    ("params_time", "def self.params_time(value)\n  json_time(value)\nend", &["json_time"]),
    ("params_hash", "def self.params_hash(value)\n  value == \"\" ? {} : value\nend", &[]),
    ("params_array", "def self.params_array(value)\n  value == \"\" ? [] : value\nend", &[]),
    (
        "json_decimal",
        "def self.json_decimal(value)\n  Float(value)\n  case value\n  when Float then BigDecimal(value, 0)\n  when String then BigDecimal(value)\n  when BigDecimal then value\n  else BigDecimal(value)\n  end\nrescue ArgumentError, TypeError\n  value\nend",
        &[],
    ),
    (
        "json_date",
        "def self.json_date(value)\n  return value unless value.is_a?(String)\n  Date.parse(value)\nrescue ArgumentError, RangeError\n  value\nend",
        &[],
    ),
    (
        "json_date_time",
        "def self.json_date_time(value)\n  return value unless value.is_a?(String)\n  DateTime.parse(value)\nrescue ArgumentError, RangeError\n  value\nend",
        &[],
    ),
    (
        "json_time",
        "def self.json_time(value)\n  return value unless value.is_a?(String)\n  Time.parse(value)\nrescue ArgumentError, RangeError\n  value\nend",
        &[],
    ),
    (
        "filled?",
        "def self.filled?(value)\n  case value\n  when String, Array, Hash then !value.empty?\n  when nil then false\n  else true\n  end\nend",
        &[],
    ),
    ("number?", "def self.number?(value)\n  Float(value)\n  true\nrescue ArgumentError, TypeError\n  false\nend", &[]),
    (
        "size?",
        "def self.size?(size, value)\n  size.is_a?(Integer) ? size.equal?(value.size) : size.include?(value.size)\nend",
        &[],
    ),
];

/// The `Dry::Schema::Lowered` method a coercion is.
fn coercion(name: &str) -> &'static str {
    LOWERED.iter().find(|(n, _, _)| *n == name).map(|(n, _, _)| *n).expect("a modeled coercion")
}

/// What the lowered calls answer, once the gems are not there, and the
/// coercions and checks they call.
fn stand_ins(used: &HashSet<&'static str>) -> Vec<LibraryClass> {
    const SOURCE: &str = r#"module Dry
  module Validation
    class Contract
    end

    class MissingMessageError < StandardError
    end

    class MessageSet
      def initialize(errors)
        @errors = errors
      end

      def to_h
        @errors
      end

      def to_hash
        @errors
      end

      def empty?
        @errors.empty?
      end

      def messages
        ::Dry::Validation::MessageSet.flatten(@errors, [])
      end

      def each(&block)
        messages.each(&block)
      end

      def map(&block)
        messages.map(&block)
      end

      def count
        messages.size
      end

      def self.flatten(errors, path)
        out = []
        errors.each do |key, value|
          if value.is_a?(Hash)
            out.concat(flatten(value, path + [key]))
          else
            value.each { |text| out << ::Dry::Schema::Message.new(text, path + [key]) }
          end
        end
        out
      end
    end

    class Result
      def initialize(values, errors)
        @values = values
        @errors = errors
      end

      def success?
        @errors.empty?
      end

      def failure?
        !@errors.empty?
      end

      def to_h
        @values
      end

      def values
        @values
      end

      def [](key)
        @values[key]
      end

      def errors
        ::Dry::Validation::MessageSet.new(@errors)
      end
    end
  end

  module Schema
    class MissingMessageError < StandardError
    end

    class Message
      def initialize(text, path)
        @text = text
        @path = path
      end

      def text
        @text
      end

      def path
        @path
      end

      def to_s
        @text
      end
    end

    class Result
      def initialize(values, errors)
        @values = values
        @errors = errors
      end

      def success?
        @errors.empty?
      end

      def failure?
        !@errors.empty?
      end

      def to_h
        @values
      end

      def [](key)
        @values[key]
      end

      def errors
        ::Dry::Validation::MessageSet.new(@errors)
      end
    end
"#;
    let mut source = SOURCE.to_string();
    let methods: Vec<&str> =
        LOWERED.iter().filter(|(n, _, _)| used.contains(n)).map(|(_, def, _)| *def).collect();
    if !methods.is_empty() {
        source.push_str("\n    module Lowered\n");
        for def in methods {
            for line in def.lines() {
                source.push_str(&format!("      {line}\n"));
            }
            source.push('\n');
        }
        source.push_str("    end\n");
    }
    source.push_str("  end\nend\n");
    let mut classes = crate::ingest::ingest_library_classes(source.as_bytes(), "<dry_validation>")
        .expect("the dry-validation stand-ins parse");
    // Scanned with the lowered classes for stdlib a target lacks.
    for c in &mut classes {
        c.origin = Some(LibraryClassOrigin::DryValidation);
    }
    classes
}
