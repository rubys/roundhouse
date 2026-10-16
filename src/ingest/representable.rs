//! `Representable::Decorator` - a bounded subset of the representable
//! gem's declarative JSON representers, expanded before inference into
//! ordinary methods, the way `alba.rs` expands an Alba resource.
//!
//! ```ruby
//! class ArticleRepresenter < Representable::Decorator
//!   include Representable::JSON
//!
//!   property :id
//!   property :title, getter: ->(**) { title.upcase }
//!   property :excerpt, exec_context: :decorator
//!   collection :comments, extend: CommentRepresenter
//!
//!   def excerpt = represented.body.to_s[0, 20]
//! end
//! ```
//!
//! becomes a plain class with `initialize(represented)`, a
//! `representable_<name>` reader per property (the value Representable
//! would read), `to_hash` (String keys, a nil value left out, as
//! Representable's default `render_nil: false` does) and `as_json_str`,
//! the same object written as JSON text for `render json:` - the shared
//! runtime has no reflective encoder on the spinel lane.
//!
//! The gem itself is never needed at run time: the class loses its
//! `Representable::Decorator` parent and its `include`, and the
//! class-body calls ingest captured for replay are dropped.
//!
//! STRICT, like Alba: a declaration or option outside the subset fails
//! the class (a survey line under `--continue`), never a partial
//! lowering. Supported options: `getter:` (a stabby lambda taking `(**)`
//! or `(represented:, **)`), `exec_context: :decorator`, `extend:` /
//! `decorator:` naming another lowered representer, `class:` (read and
//! ignored: it only matters when parsing JSON back), `as:` and
//! `render_nil:`.
//!
//! `as_json_str` encodes every non-nested value with
//! `JsonBuilder.encode_value`; `lower::as_json_poro` settles the encoder
//! per value from its analyzed type after inference, and removes the
//! writer when a value has no encoding there.

use std::collections::{HashMap, HashSet};

use crate::dialect::{LibraryClass, LibraryClassOrigin, RepresentableField};
use crate::expr::Expr;
use crate::ident::{ClassId, Symbol};
use crate::span::{SourceFile, Span};

use super::util::{constant_id_str, flatten_statements, symbol_value};
use super::{IngestError, IngestResult};

const DECORATOR: &str = "Representable::Decorator";
const JSON_MODULE: &str = "Representable::JSON";

/// Calls a getter body may make without a receiver that are Kernel's,
/// not the represented object's.
const KERNEL: &[&str] = &[
    "raise", "format", "sprintf", "puts", "p", "Integer", "Float", "String", "Array", "Hash",
    "lambda", "proc", "loop", "rand", "block_given?",
];

struct Field {
    name: String,
    key: String,
    /// Ruby source of the value, already in the decorator's context.
    value: String,
    nested: Option<(ClassId, bool)>,
    render_nil: bool,
}

struct Decl {
    fields: Vec<Field>,
    span: Span,
}

pub(super) fn lower_representable_decorators(
    app: &mut crate::App,
    sources: &[SourceFile],
) -> IngestResult<()> {
    let selected: HashSet<ClassId> = app
        .library_classes
        .iter()
        .filter(|c| c.parent.as_ref().is_some_and(|p| p.0.as_str() == DECORATOR))
        .map(|c| c.name.clone())
        .collect();
    if selected.is_empty() {
        return Ok(());
    }
    // Per class: its declarations, or why it is outside the subset.
    let mut decls: HashMap<ClassId, Decl> = HashMap::new();
    let mut refused: HashMap<ClassId, IngestError> = HashMap::new();
    for source in sources {
        if !source.text.contains(DECORATOR) {
            continue;
        }
        let parsed = ruby_prism::parse(source.text.as_bytes());
        let mut found = Vec::new();
        collect_classes(&parsed.node(), &mut Vec::new(), &mut found);
        for (path, class) in found {
            let id = ClassId(Symbol::from(path.join("::")));
            if !selected.contains(&id) || refused.contains_key(&id) {
                continue;
            }
            let offset = class.location().start_offset();
            if decls.remove(&id).is_some() {
                refused.insert(id, refuse(source, offset, "a reopened representer is not supported"));
                continue;
            }
            match collect_fields(&class, source, &selected) {
                Ok(fields) => {
                    let span = Span {
                        file: super::sources::file_id(&source.path),
                        start: offset as u32,
                        end: offset as u32,
                    };
                    decls.insert(id, Decl { fields, span });
                }
                Err(e) => {
                    refused.insert(id, e);
                }
            }
        }
    }
    for id in &selected {
        if !decls.contains_key(id) && !refused.contains_key(id) {
            refused.insert(
                id.clone(),
                IngestError::Unsupported {
                    file: "<representable>".into(),
                    message: format!("Representable subset: cannot find the source of {}", id.0),
                },
            );
        }
    }
    // A representer that renders through a refused one cannot be lowered
    // either: its generated writer would call a method nobody defines.
    loop {
        let blocked: Vec<(ClassId, ClassId)> = decls
            .iter()
            .filter_map(|(id, d)| {
                d.fields
                    .iter()
                    .filter_map(|f| f.nested.as_ref().map(|(t, _)| t))
                    .find(|t| refused.contains_key(*t))
                    .map(|t| (id.clone(), t.clone()))
            })
            .collect();
        if blocked.is_empty() {
            break;
        }
        for (id, target) in blocked {
            decls.remove(&id);
            refused.insert(
                id.clone(),
                IngestError::Unsupported {
                    file: "<representable>".into(),
                    message: format!(
                        "Representable subset: {} renders through {}, which is outside the subset",
                        id.0, target.0
                    ),
                },
            );
        }
    }
    // Strict ingest stops at the first refusal. Survey mode records each
    // and leaves that class as it was (its body replayed for the gem), so
    // one unsupported representer does not stop the rest of the app.
    if !refused.is_empty() {
        if !super::survey::is_active() {
            let first = refused
                .into_iter()
                .min_by(|a, b| a.0 .0.as_str().cmp(b.0 .0.as_str()))
                .expect("non-empty");
            return Err(first.1);
        }
        let mut ordered: Vec<_> = refused.iter().collect();
        ordered.sort_by(|a, b| a.0 .0.as_str().cmp(b.0 .0.as_str()));
        for (_, e) in ordered {
            super::survey::record(e);
        }
    }
    for class in &mut app.library_classes {
        if let Some(decl) = decls.get(&class.name) {
            synthesize(class, decl)?;
        }
    }
    Ok(())
}

/// Every class in the file with its fully qualified path, through
/// `module` and `class` nesting.
fn collect_classes<'pr>(
    node: &ruby_prism::Node<'pr>,
    scope: &mut Vec<String>,
    out: &mut Vec<(Vec<String>, ruby_prism::ClassNode<'pr>)>,
) {
    if let Some(program) = node.as_program_node() {
        for stmt in program.statements().body().iter() {
            collect_classes(&stmt, scope, out);
        }
    } else if let Some(statements) = node.as_statements_node() {
        for stmt in statements.body().iter() {
            collect_classes(&stmt, scope, out);
        }
    } else if let Some(module) = node.as_module_node() {
        let Some(name) = super::util::constant_path_of(&module.constant_path()) else { return };
        let depth = scope.len();
        scope.extend(name);
        if let Some(body) = module.body() {
            collect_classes(&body, scope, out);
        }
        scope.truncate(depth);
    } else if let Some(class) = node.as_class_node() {
        let Some(name) = super::util::constant_path_of(&class.constant_path()) else { return };
        let depth = scope.len();
        scope.extend(name);
        let body = class.body();
        out.push((scope.clone(), class));
        if let Some(body) = body {
            collect_classes(&body, scope, out);
        }
        scope.truncate(depth);
    }
}

fn collect_fields(
    class: &ruby_prism::ClassNode<'_>,
    source: &SourceFile,
    selected: &HashSet<ClassId>,
) -> IngestResult<Vec<Field>> {
    let mut fields: Vec<Field> = Vec::new();
    let mut included = false;
    let Some(body) = class.body() else { return Ok(fields) };
    for stmt in flatten_statements(body) {
        let offset = stmt.location().start_offset();
        let fail = |reason: &str| refuse(source, offset, reason);
        if let Some(def) = stmt.as_def_node() {
            // The decorator's own methods are ingested as methods already,
            // unless the generated ones would replace them (Ruby keeps the
            // later definition) or they sit on the class side.
            if def.receiver().is_some() {
                return Err(fail("a class method on a representer is outside the Representable subset"));
            }
            let name = constant_id_str(&def.name());
            if GENERATED.contains(&name) || name.starts_with("representable_") {
                return Err(fail(&format!("`def {name}` would be replaced by the generated method")));
            }
            continue;
        }
        let Some(call) = stmt.as_call_node() else {
            return Err(fail("a representer body may hold only declarations and methods"));
        };
        if call.receiver().is_some() || call.block().is_some() {
            return Err(fail("a receiver or block on a representer declaration"));
        }
        let args: Vec<_> = call.arguments().map(|a| a.arguments().iter().collect()).unwrap_or_default();
        let method = constant_id_str(&call.name());
        match method {
            "include" => {
                let path = args.first().and_then(super::util::constant_path_of).map(|p| p.join("::"));
                if args.len() != 1 || path.as_deref() != Some(JSON_MODULE) || included {
                    return Err(fail("only a single `include Representable::JSON` is supported"));
                }
                included = true;
            }
            "property" | "collection" => {
                let field = declaration(method == "collection", &args, source, selected)
                    .map_err(|reason| fail(&reason))?;
                if fields.iter().any(|f| f.key == field.key || f.name == field.name) {
                    return Err(fail("a property declared twice"));
                }
                fields.push(field);
            }
            _ => return Err(fail(&format!("`{method}` is outside the Representable subset"))),
        }
    }
    if !included {
        return Err(refuse(
            source,
            class.location().start_offset(),
            "a representer must `include Representable::JSON`",
        ));
    }
    Ok(fields)
}

fn declaration(
    collection: bool,
    args: &[ruby_prism::Node<'_>],
    source: &SourceFile,
    selected: &HashSet<ClassId>,
) -> Result<Field, String> {
    let name = args.first().and_then(symbol_value).ok_or("a property needs a Symbol name")?;
    if !is_identifier(&name) {
        return Err(format!("property name `{name}` is not a plain identifier"));
    }
    let mut key = name.clone();
    let mut getter_node: Option<ruby_prism::Node<'_>> = None;
    let mut decorator_context = false;
    let mut nested: Option<ClassId> = None;
    let mut render_nil = false;
    if let Some(options) = args.get(1) {
        let hash = options.as_keyword_hash_node().ok_or("property options must be keywords")?;
        for element in hash.elements().iter() {
            let assoc = element.as_assoc_node().ok_or("property options must be keywords")?;
            let option = symbol_value(&assoc.key()).ok_or("property options must be Symbol keys")?;
            let value = assoc.value();
            match option.as_str() {
                "getter" => getter_node = Some(value),
                "exec_context" => {
                    if symbol_value(&value).as_deref() != Some("decorator") {
                        return Err("only `exec_context: :decorator` is supported".into());
                    }
                    decorator_context = true;
                }
                "extend" | "decorator" => {
                    let path = super::util::constant_path_of(&value)
                        .ok_or("`extend:` must name a representer constant")?;
                    let id = ClassId(Symbol::from(path.join("::")));
                    if !selected.contains(&id) {
                        return Err(format!("`{}` is not a lowered Representable::Decorator", id.0));
                    }
                    nested = Some(id);
                }
                "class" => {}
                "as" => {
                    key = symbol_value(&value)
                        .or_else(|| value.as_string_node().map(|s| String::from_utf8_lossy(s.unescaped()).into_owned()))
                        .ok_or("`as:` must be a Symbol or String literal")?;
                    if !is_identifier(&key) {
                        return Err(format!("key `{key}` would need escaping"));
                    }
                }
                "render_nil" => {
                    render_nil = if value.as_true_node().is_some() {
                        true
                    } else if value.as_false_node().is_some() {
                        false
                    } else {
                        return Err("`render_nil:` must be true or false".into());
                    };
                }
                other => return Err(format!("option `{other}:` is outside the Representable subset")),
            }
        }
    }
    if args.len() > 2 {
        return Err("a property takes a name and options".into());
    }
    let getter = getter_node.map(|n| getter_source(&n, source, decorator_context)).transpose()?;
    let value = match (getter, decorator_context) {
        (Some(body), _) => body,
        (None, true) => name.clone(),
        (None, false) => format!("represented.{name}"),
    };
    Ok(Field { name, key, value, nested: nested.map(|id| (id, collection)), render_nil })
}

/// The Ruby source a `getter:` lambda's value comes from, in the
/// generated method's context (`self` is the decorator, `represented`
/// its reader).
///
/// `->(represented:, **) { … }` reads `represented` itself: the body is
/// taken as written. `->(**) { … }` is evaluated on the represented
/// object (Representable's default `exec_context`), so each call it
/// makes without a receiver is given `represented.` - a local, a block
/// parameter and Kernel's own methods excepted. `next value` becomes
/// `return value`, since the body now lives in a method.
fn getter_source(
    node: &ruby_prism::Node<'_>,
    source: &SourceFile,
    decorator_context: bool,
) -> Result<String, String> {
    let lambda = node.as_lambda_node().ok_or("`getter:` must be a stabby lambda")?;
    let params = lambda
        .parameters()
        .and_then(|p| p.as_block_parameters_node())
        .and_then(|p| p.parameters());
    let mut reads_represented = false;
    if let Some(params) = params {
        if params.requireds().iter().next().is_some()
            || params.optionals().iter().next().is_some()
            || params.rest().is_some()
            || params.posts().iter().next().is_some()
            || params.block().is_some()
            || params.keyword_rest().is_none()
        {
            return Err("a getter must take `(**)` or `(represented:, **)`".into());
        }
        for kw in params.keywords().iter() {
            let required = kw.as_required_keyword_parameter_node().ok_or("a getter keyword must be required")?;
            if constant_id_str(&required.name()) != "represented" {
                return Err("a getter may only take the `represented:` keyword".into());
            }
            reads_represented = true;
        }
    } else {
        return Err("a getter must take `(**)` or `(represented:, **)`".into());
    }
    let body = lambda.body().ok_or("an empty getter")?;
    let start = body.location().start_offset();
    let end = body.location().end_offset();
    let mut edits: Vec<(usize, usize, String)> = Vec::new();
    // Under `exec_context: :decorator` the body already runs on the
    // decorator, as the generated method does.
    let mut rebase = RebaseVisitor { prefix: !reads_represented && !decorator_context, depth: 0, edits: &mut edits };
    ruby_prism::Visit::visit(&mut rebase, &body);
    let mut text = source.text[start..end].to_string();
    edits.sort_by(|a, b| b.0.cmp(&a.0));
    for (from, to, replacement) in edits {
        text.replace_range(from - start..to - start, &replacement);
    }
    Ok(text)
}

/// Methods `synthesize` writes on the expanded class; a decorator `def` of
/// one of these names would be silently replaced.
const GENERATED: [&str; 4] = ["initialize", "represented", "to_hash", "as_json_str"];

struct RebaseVisitor<'a> {
    /// Give receiverless calls `represented.` (the `(**)` form).
    prefix: bool,
    /// Blocks and lambdas entered below the getter's own body: a `next`
    /// there ends that block's iteration, not the getter.
    depth: usize,
    edits: &'a mut Vec<(usize, usize, String)>,
}

impl<'pr> ruby_prism::Visit<'pr> for RebaseVisitor<'_> {
    fn visit_call_node(&mut self, node: &ruby_prism::CallNode<'pr>) {
        if self.prefix && node.receiver().is_none() {
            let name = constant_id_str(&node.name());
            if !KERNEL.contains(&name) && name != "represented" {
                let at = node.location().start_offset();
                self.edits.push((at, at, "represented.".to_string()));
            }
        }
        ruby_prism::visit_call_node(self, node);
    }
    fn visit_self_node(&mut self, node: &ruby_prism::SelfNode<'pr>) {
        if self.prefix {
            let loc = node.location();
            self.edits.push((loc.start_offset(), loc.end_offset(), "represented".to_string()));
        }
    }
    fn visit_next_node(&mut self, node: &ruby_prism::NextNode<'pr>) {
        if self.depth == 0 {
            let at = node.location().start_offset();
            self.edits.push((at, at + "next".len(), "return".to_string()));
        }
        ruby_prism::visit_next_node(self, node);
    }
    fn visit_block_node(&mut self, node: &ruby_prism::BlockNode<'pr>) {
        self.depth += 1;
        ruby_prism::visit_block_node(self, node);
        self.depth -= 1;
    }
    fn visit_lambda_node(&mut self, node: &ruby_prism::LambdaNode<'pr>) {
        self.depth += 1;
        ruby_prism::visit_lambda_node(self, node);
        self.depth -= 1;
    }
}

fn synthesize(class: &mut LibraryClass, decl: &Decl) -> IngestResult<()> {
    let mut src = String::new();
    src.push_str(&format!("class {}\n", class.name.0));
    src.push_str("  def initialize(represented)\n    @represented = represented\n  end\n\n");
    src.push_str("  def represented\n    @represented\n  end\n");
    for f in &decl.fields {
        src.push_str(&format!("\n  def representable_{}\n    {}\n  end\n", f.name, f.value));
    }
    // to_hash: Representable's own answer, String keys, nil left out
    // unless `render_nil: true`.
    src.push_str("\n  def to_hash\n    h = {}\n");
    for f in &decl.fields {
        let local = format!("v_{}", f.name);
        let value = match &f.nested {
            Some((r, true)) => format!("{local}.map {{ |item| {}.new(item).to_hash }}", r.0),
            Some((r, false)) => format!("{}.new({local}).to_hash", r.0),
            None => local.clone(),
        };
        src.push_str(&format!("    {local} = representable_{}\n", f.name));
        let key = serde_json::to_string(&f.key).unwrap();
        if f.render_nil && f.nested.is_none() {
            src.push_str(&format!("    h[{key}] = {value}\n"));
        } else {
            src.push_str(&format!("    h[{key}] = {value} unless {local}.nil?\n"));
        }
    }
    src.push_str("    h\n  end\n");
    // as_json_str: the same object as JSON text. Per-pair parts joined
    // with "," - whether a key is present is only known at run time.
    src.push_str("\n  def as_json_str\n    parts = []\n");
    for f in &decl.fields {
        let local = format!("j_{}", f.name);
        let open = serde_json::to_string(&format!("\"{}\":", f.key)).unwrap();
        let text = match &f.nested {
            Some((r, true)) => format!(
                "{open} + \"[\" + {local}.map {{ |item| {}.new(item).as_json_str }}.join(\",\") + \"]\"",
                r.0
            ),
            Some((r, false)) => format!("{open} + {}.new({local}).as_json_str", r.0),
            None => format!("{open} + JsonBuilder.encode_value({local})"),
        };
        src.push_str(&format!("    {local} = representable_{}\n", f.name));
        if f.render_nil && f.nested.is_none() {
            src.push_str(&format!("    parts << {text}\n"));
        } else {
            src.push_str(&format!("    parts << {text} unless {local}.nil?\n"));
        }
    }
    src.push_str("    \"{\" + parts.join(\",\") + \"}\"\n  end\nend\n");

    let (parsed, diags) =
        super::prism::scope(|| super::ingest_library_classes(src.as_bytes(), "<representable>"));
    let mut parsed = parsed?;
    if !diags.is_empty() || parsed.is_empty() {
        return Err(IngestError::Parse {
            file: "<representable>".into(),
            message: format!("invalid synthesized representer for {}: {diags:?}\n{src}", class.name.0),
        });
    }
    // Every generated node points at the declaration, so a diagnostic
    // lands on the class, except constants: the analyzer resolves a
    // constant with a source position by looking it up in the source
    // index at that position, where a generated `JsonBuilder` is not
    // written. A synthetic position resolves it by name instead, as the
    // other generated writers do.
    fn mark(expr: &mut Expr, span: Span) {
        expr.span = if matches!(&*expr.node, crate::expr::ExprNode::Const { .. }) {
            Span::synthetic()
        } else {
            span
        };
        expr.node.for_each_child_mut(&mut |child| mark(child, span));
    }
    let span = decl.span;
    let mut methods = parsed.remove(0).methods;
    for method in &mut methods {
        mark(&mut method.body, span);
        method.name_span = span;
    }
    class.methods.append(&mut methods);
    class.parent = None;
    class.includes.retain(|i| i.0.as_str() != JSON_MODULE);
    class.unknown_calls.clear();
    class.origin = Some(LibraryClassOrigin::RepresentableDecorator {
        declaration_span: span,
        fields: decl
            .fields
            .iter()
            .map(|f| RepresentableField {
                name: Symbol::from(f.name.as_str()),
                key: f.key.clone(),
                nested: f.nested.as_ref().map(|(id, _)| id.clone()),
            })
            .collect(),
    });
    Ok(())
}

fn is_identifier(s: &str) -> bool {
    let mut chars = s.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn refuse(source: &SourceFile, offset: usize, reason: &str) -> IngestError {
    let (line, column) = source.line_col(offset as u32);
    IngestError::Unsupported {
        file: source.path.clone(),
        message: format!("{line}:{column}: Representable subset: {reason}"),
    }
}
