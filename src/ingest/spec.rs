//! RSpec spec-file ingestion: every `*_spec.rb` file under a folder that
//! `roundhouse.yml` lists in `test_paths` (`spec/models`, `spec/lib`, ...).
//!
//! RSpec's own model (`RSpec::Core::ExampleGroup`) is what this mirrors:
//! every `describe`/`context` is a subclass of the group around it, `let`
//! is a memoized instance method on that subclass (an inner `let`
//! overrides an outer one), `before` hooks run outer to inner, and every
//! example runs on a fresh instance. Ruby has no anonymous nesting in the
//! emitted test tree, so each group that owns examples becomes one
//! `TestModule` that carries what RSpec would give its examples: the lets
//! of the whole ancestor chain, every enclosing `before` hook in order,
//! the helper `def`s. Everything after ingest (analysis, the shared
//! lowering, `inline_assertions`, the per-target test emit) then serves a
//! spec exactly as it serves a Minitest file.
//!
//! Expectations are read into the assertion vocabulary the test lowering
//! already inlines: `expect(a).to eq(b)` is `assert(a == b)`, with RSpec's
//! own operand order (`eq` is `actual == expected`) and evaluation order
//! (the actual value first).
//!
//! The supported subset, everything else refused by name (an error in
//! strict mode, a survey gap with `--continue`):
//!
//! - groups: `RSpec.describe` / `describe` / `context`, with a constant,
//!   a string, or both, and `type: :model | :request | :lib` metadata;
//! - `let`, `let!`, `subject`, `subject(:name)`, `described_class`, the
//!   implicit `subject` (`described_class.new`);
//! - `before` / `before(:each)` / `before(:example)`;
//! - `it` / `specify` / `example` with a block; `xit` and block-less
//!   examples are pending in RSpec, so they are not tests here either;
//! - helper `def`s in a group body;
//! - `expect(x).to` / `not_to` / `to_not` and `is_expected.to` with `eq`,
//!   `eql`, `equal`, `be(x)`, `be_nil`, `be_truthy`, `be_falsey`, `be_a` /
//!   `be_an` / `be_kind_of` / `be_a_kind_of`, `be_instance_of` /
//!   `be_an_instance_of`, `be_<predicate>`, `have_<predicate>` and
//!   `include` of plain values.

use std::collections::HashMap;

use ruby_prism::Node;

use crate::ingest::prism_api::PrismLocation;

use crate::dialect::{AccessorKind, MethodDef, MethodReceiver, MethodVisibility, Test, TestModule};
use crate::effect::EffectSet;
use crate::expr::{Expr, ExprNode, LValue, Literal};
use crate::span::Span;
use crate::{ClassId, Symbol};

use super::expr::ingest_expr;
use super::library_class::ingest_library_method;
use super::util::{constant_id_str, constant_path_of, flatten_statements, node_span};
use super::{IngestError, IngestResult};

/// Every example group in the file that owns examples, each as its own
/// `TestModule`, outer groups first. A file with no example group (a
/// support file under a listed folder) gives none.
pub fn ingest_spec_files(source: &[u8], file: &str) -> IngestResult<Vec<TestModule>> {
    super::sources::register(file, &String::from_utf8_lossy(source));
    let result = super::prism::parse(source, file);
    let root = result.node();
    let mut reader = Reader {
        file,
        source,
        prefix: file_prefix(file),
        modules: Vec::new(),
        module_names: HashMap::new(),
    };
    let Some(program) = root.as_program_node() else { return Ok(Vec::new()) };
    for stmt in program.statements().body().iter() {
        if let Some(call) = stmt.as_call_node() {
            if is_top_level_group(&call) {
                reader.group(&call, &[])?;
                continue;
            }
            // `require "rails_helper"` and friends: the spec helper is not
            // read yet, so nothing it configures reaches the examples.
            let name = constant_id_str(&call.name());
            if call.receiver().is_none() && matches!(name, "require" | "require_relative") {
                continue;
            }
        }
        reader.refuse(&stmt, "statement outside an example group in a spec file")?;
    }
    Ok(reader.modules)
}

/// Whether `source` is an RSpec file: one of its top-level statements is
/// an example group (`RSpec.describe` / `describe ... do`) or an
/// `RSpec.shared_*` definition.
pub fn is_rspec_source(source: &[u8]) -> bool {
    let result = super::prism::parse_silent(source);
    let root = result.node();
    let Some(program) = root.as_program_node() else { return false };
    program.statements().body().iter().any(|stmt| {
        let Some(call) = stmt.as_call_node() else { return false };
        is_top_level_group(&call)
            || call.receiver().and_then(|r| constant_path_of(&r)).is_some_and(|p| p == ["RSpec"])
    })
}

/// `RSpec.describe ... do` or a bare top-level `describe ... do`.
fn is_top_level_group(call: &ruby_prism::CallNode<'_>) -> bool {
    if constant_id_str(&call.name()) != "describe" || call.block().is_none() {
        return false;
    }
    match call.receiver() {
        None => true,
        Some(recv) => constant_path_of(&recv).is_some_and(|p| p == ["RSpec"]),
    }
}

/// What a spec's location under `spec/` contributes to its module names:
/// `spec/models/article_spec.rb` is `ModelsArticle`. File paths are unique,
/// so two files that both `describe Article` still give two classes.
fn file_prefix(file: &str) -> String {
    let path = std::path::Path::new(file);
    let parts: Vec<String> = path.iter().map(|p| p.to_string_lossy().into_owned()).collect();
    let after_spec = match parts.iter().rposition(|p| p == "spec") {
        Some(i) => parts[i + 1..].to_vec(),
        None => parts.last().cloned().into_iter().collect(),
    };
    after_spec
        .iter()
        .map(|p| p.strip_suffix(".rb").unwrap_or(p))
        .map(|p| p.strip_suffix("_spec").unwrap_or(p))
        .map(|p| camel_words(p))
        .collect()
}

/// `"without a title"` -> `WithoutATitle`, `"#full_name"` -> `FullName`.
fn camel_words(text: &str) -> String {
    text.split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut chars = w.chars();
            let first = chars.next().unwrap().to_ascii_uppercase();
            std::iter::once(first).chain(chars).collect::<String>()
        })
        .collect()
}

/// The spec type, which picks the test case the module is written against.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SpecType {
    /// `type: :model`, `type: :lib`, or no type: a plain test case.
    Unit,
    /// `type: :request`: an integration test, with the request helpers.
    Request,
}

impl SpecType {
    fn parent(self) -> ClassId {
        ClassId(Symbol::from(match self {
            SpecType::Unit => "ActiveSupport::TestCase",
            SpecType::Request => "ActionDispatch::IntegrationTest",
        }))
    }
}

/// One example group on the way down: what it adds to the groups around it.
#[derive(Clone)]
struct Frame {
    /// The words this group adds to the module name (`WithoutATitle`).
    name_part: String,
    /// The constant the group describes, when it names one.
    described: Option<Vec<String>>,
    spec_type: Option<SpecType>,
    /// `let` / `subject` definitions in declaration order: name, body, span.
    lets: Vec<(String, Expr, Span)>,
    /// This group's `before` hooks and `let!` calls, in declaration order.
    hooks: Vec<Expr>,
    defs: Vec<MethodDef>,
}

struct Reader<'a> {
    file: &'a str,
    source: &'a [u8],
    prefix: String,
    modules: Vec<TestModule>,
    module_names: HashMap<String, usize>,
}

impl Reader<'_> {
    /// Refuse a construct: an error in strict mode, a survey gap (and the
    /// construct skipped) with `--continue`.
    fn refuse(&self, node: &Node<'_>, what: &str) -> IngestResult<()> {
        super::survey::continue_or_fail(self.unsupported(node, what))
    }

    fn unsupported(&self, node: &Node<'_>, what: &str) -> IngestError {
        IngestError::Unsupported {
            file: self.file.into(),
            message: format!("RSpec: {what} is not supported (line {})", self.line(node)),
        }
    }

    fn line(&self, node: &Node<'_>) -> usize {
        let offset = node.location().start_offset().min(self.source.len());
        1 + self.source[..offset].iter().filter(|b| **b == b'\n').count()
    }

    fn text(&self, node: &Node<'_>) -> String {
        let loc = node.location();
        String::from_utf8_lossy(&self.source[loc.start_offset()..loc.end_offset()]).into_owned()
    }

    /// Read one example group, then the groups nested in it. Lets, hooks
    /// and defs anywhere in a group's body apply to every example in it,
    /// whichever comes first in the source (they are methods and hooks
    /// of the group's class), so the whole body is read before a nested
    /// group is.
    fn group(&mut self, call: &ruby_prism::CallNode<'_>, outer: &[Frame]) -> IngestResult<()> {
        let call_node = call.as_node();
        let Some(mut frame) = self.group_header(call, outer)? else { return Ok(()) };
        let Some(block) = call.block().and_then(|b| b.as_block_node()) else {
            return self.refuse(&call_node, "an example group without a block");
        };
        if block.parameters().is_some() {
            return self.refuse(&call_node, "block parameters on an example group");
        }
        let owner = ClassId(Symbol::from("RSpecGroup"));
        let mut examples: Vec<(String, Node<'_>, ExampleBody<'_>)> = Vec::new();
        let mut children = Vec::new();
        let mut refused = false;
        let stmts = block.body().map(flatten_statements).unwrap_or_default();
        for stmt in &stmts {
            if let Some(def) = stmt.as_def_node() {
                frame.defs.retain(|d| d.name.as_str() != constant_id_str(&def.name()));
                frame.defs.push(ingest_library_method(&def, &owner, self.file)?);
                continue;
            }
            let Some(item) = stmt.as_call_node() else {
                self.refuse(stmt, "this statement in an example group")?;
                refused = true;
                continue;
            };
            let name = constant_id_str(&item.name());
            if item.receiver().is_some() {
                self.refuse(stmt, &format!("`{}` in an example group", self.text(stmt)))?;
                refused = true;
                continue;
            }
            let ok = match name {
                "describe" | "context" => {
                    children.push(item);
                    true
                }
                "let" | "let!" => self.read_let(&item, &mut frame, name == "let!")?,
                "subject" => self.read_subject(&item, &mut frame)?,
                "before" => self.read_before(&item, &mut frame)?,
                "it" | "specify" | "example" => match self.example(&item)? {
                    Some((desc, body)) => {
                        examples.push((desc, item.as_node(), body));
                        true
                    }
                    None => false,
                },
                // Pending in RSpec: reported, never run, never a pass.
                "xit" | "xspecify" | "xexample" | "xdescribe" | "xcontext" => true,
                _ => {
                    self.refuse(stmt, &format!("`{name}` in an example group"))?;
                    false
                }
            };
            refused |= !ok;
        }
        // In survey mode a refused group-level item leaves the group's
        // examples without something they may need; none of them is
        // claimed to run.
        if refused {
            return Ok(());
        }
        let mut chain: Vec<Frame> = outer.to_vec();
        chain.push(frame);
        if !examples.is_empty() {
            self.emit(&chain, examples)?;
        }
        for child in children {
            self.group(&child, &chain)?;
        }
        Ok(())
    }

    /// The group's arguments: an optional constant, an optional string,
    /// an optional `type:`. `None` when the header was refused (survey).
    fn group_header(
        &self,
        call: &ruby_prism::CallNode<'_>,
        outer: &[Frame],
    ) -> IngestResult<Option<Frame>> {
        let node = call.as_node();
        let mut described = None;
        let mut words = Vec::new();
        let mut spec_type = None;
        let args: Vec<Node<'_>> = call.arguments().map(|a| a.arguments().iter().collect()).unwrap_or_default();
        for arg in &args {
            if let Some(path) = constant_path_of(arg) {
                if described.is_some() || !words.is_empty() {
                    self.refuse(&node, "a constant after the first group argument")?;
                    return Ok(None);
                }
                words.push(path.join(""));
                described = Some(path);
            } else if let Some(s) = arg.as_string_node() {
                words.push(camel_words(&String::from_utf8_lossy(s.unescaped())));
            } else if let Some(hash) = arg.as_keyword_hash_node() {
                for element in hash.elements().iter() {
                    let Some(assoc) = element.as_assoc_node() else {
                        self.refuse(&node, "a splat in example group metadata")?;
                        return Ok(None);
                    };
                    let key = assoc.key().as_symbol_node().map(|s| String::from_utf8_lossy(s.unescaped()).into_owned());
                    let value = assoc.value().as_symbol_node().map(|s| String::from_utf8_lossy(s.unescaped()).into_owned());
                    match (key.as_deref(), value.as_deref()) {
                        (Some("type"), Some("model" | "lib")) => spec_type = Some(SpecType::Unit),
                        (Some("type"), Some("request")) => spec_type = Some(SpecType::Request),
                        _ => {
                            self.refuse(&node, &format!("example group metadata `{}`", self.text(&element)))?;
                            return Ok(None);
                        }
                    }
                }
            } else {
                self.refuse(&node, &format!("example group argument `{}`", self.text(arg)))?;
                return Ok(None);
            }
        }
        // `describe` inside a group may name a method (`"#title"`) or a
        // new subject; only a constant replaces the described class.
        let described = described.or_else(|| outer.iter().rev().find_map(|f| f.described.clone()));
        Ok(Some(Frame {
            name_part: words.concat(),
            described,
            spec_type,
            lets: Vec::new(),
            hooks: Vec::new(),
            defs: Vec::new(),
        }))
    }

    /// `let(:name) { body }` / `let!(:name) { body }`. `let!` is the `let`
    /// plus a call at its declaration point among the group's hooks.
    fn read_let(&self, call: &ruby_prism::CallNode<'_>, frame: &mut Frame, bang: bool) -> IngestResult<bool> {
        let node = call.as_node();
        let Some(name) = single_symbol_arg(call) else {
            self.refuse(&node, "a `let` whose argument is not one symbol")?;
            return Ok(false);
        };
        let Some(body) = self.block_body(call, "let")? else { return Ok(false) };
        if !self.let_name_is_free(&node, &name)? {
            return Ok(false);
        }
        let span = node_span(&node, self.file);
        frame.lets.push((name.clone(), body, span));
        if bang {
            frame.hooks.push(self_call(&name, span));
        }
        Ok(true)
    }

    /// `subject { body }` or `subject(:name) { body }`. The named form
    /// defines both `name` and `subject`, the same memoized value.
    fn read_subject(&self, call: &ruby_prism::CallNode<'_>, frame: &mut Frame) -> IngestResult<bool> {
        let node = call.as_node();
        let name = match call.arguments() {
            None => None,
            Some(_) => match single_symbol_arg(call) {
                Some(n) => Some(n),
                None => {
                    self.refuse(&node, "a `subject` whose argument is not one symbol")?;
                    return Ok(false);
                }
            },
        };
        let Some(body) = self.block_body(call, "subject")? else { return Ok(false) };
        let span = node_span(&node, self.file);
        match name {
            None => frame.lets.push(("subject".into(), body, span)),
            Some(name) => {
                if !self.let_name_is_free(&node, &name)? {
                    return Ok(false);
                }
                frame.lets.push((name.clone(), body, span));
                frame.lets.push(("subject".into(), self_call(&name, span), span));
            }
        }
        Ok(true)
    }

    /// A let name that would become a `def test_*` is a test to the
    /// runner, and RSpec allows it; refused until it is renamed.
    fn let_name_is_free(&self, node: &Node<'_>, name: &str) -> IngestResult<bool> {
        if name.starts_with("test_") {
            self.refuse(node, &format!("a `let` named `{name}` (it would run as a test)"))?;
            return Ok(false);
        }
        Ok(true)
    }

    /// `before do ... end`, `before(:each)`, `before(:example)`. A hook
    /// that runs once per group (`:all`, `:context`) is refused: every
    /// example here runs on a fresh instance, as `:each` hooks need.
    fn read_before(&self, call: &ruby_prism::CallNode<'_>, frame: &mut Frame) -> IngestResult<bool> {
        let node = call.as_node();
        if call.arguments().is_some() && !matches!(single_symbol_arg(call).as_deref(), Some("each" | "example")) {
            self.refuse(&node, &format!("`{}`", self.text(&node).lines().next().unwrap_or_default()))?;
            return Ok(false);
        }
        let Some(body) = self.block_body(call, "before")? else { return Ok(false) };
        frame.hooks.push(body);
        Ok(true)
    }

    /// An example's description and body. `None` when the example was
    /// refused (survey mode).
    fn example<'pr>(
        &self,
        call: &ruby_prism::CallNode<'pr>,
    ) -> IngestResult<Option<(String, ExampleBody<'pr>)>> {
        let node = call.as_node();
        let args: Vec<Node<'_>> = call.arguments().map(|a| a.arguments().iter().collect()).unwrap_or_default();
        let desc = match args.as_slice() {
            [] => format!("example at line {}", self.line(&node)),
            [s] if s.as_string_node().is_some() => {
                String::from_utf8_lossy(s.as_string_node().unwrap().unescaped()).into_owned()
            }
            _ => {
                self.refuse(&node, "example arguments other than one description string")?;
                return Ok(None);
            }
        };
        let Some(block) = call.block() else {
            return Ok(Some((desc, ExampleBody::Pending)));
        };
        let Some(block) = block.as_block_node() else {
            self.refuse(&node, "an example given a block argument")?;
            return Ok(None);
        };
        if block.parameters().is_some() {
            self.refuse(&node, "block parameters on an example")?;
            return Ok(None);
        }
        Ok(Some((desc, match block.body() {
            Some(body) => ExampleBody::Body(body),
            None => ExampleBody::Empty,
        })))
    }

    /// The ingested body of `name { ... }`, or `None` with the construct
    /// refused when there is no block or it takes parameters.
    fn block_body(&self, call: &ruby_prism::CallNode<'_>, what: &str) -> IngestResult<Option<Expr>> {
        let node = call.as_node();
        let Some(block) = call.block().and_then(|b| b.as_block_node()) else {
            self.refuse(&node, &format!("`{what}` without a block"))?;
            return Ok(None);
        };
        if block.parameters().is_some() {
            self.refuse(&node, &format!("block parameters on `{what}`"))?;
            return Ok(None);
        }
        Ok(Some(match block.body() {
            Some(body) => ingest_expr(&body, self.file)?,
            None => nil(Span::synthetic()),
        }))
    }

    /// One `TestModule` for the innermost group of `chain`.
    fn emit(&mut self, chain: &[Frame], examples: Vec<(String, Node<'_>, ExampleBody<'_>)>) -> IngestResult<()> {
        let mut base = self.prefix.clone();
        for frame in &chain[1..] {
            base.push_str(&frame.name_part);
        }
        base.push_str("Spec");
        if !base.starts_with(|c: char| c.is_ascii_uppercase()) {
            base = format!("Spec{base}");
        }
        let n = self.module_names.entry(base.clone()).or_insert(0);
        *n += 1;
        let name = if *n == 1 { base } else { format!("{base}{n}") };
        let class = ClassId(Symbol::from(name.as_str()));
        let described = chain.iter().rev().find_map(|f| f.described.clone());
        let spec_type = chain.iter().rev().find_map(|f| f.spec_type).unwrap_or_else(|| {
            let parts: Vec<_> = std::path::Path::new(self.file).iter().collect();
            if parts.windows(2).any(|w| w[0] == "spec" && w[1] == "requests") {
                SpecType::Request
            } else {
                SpecType::Unit
            }
        });

        // Lets: an inner definition replaces an outer one of the same name.
        let mut lets: Vec<(String, Expr, Span)> = Vec::new();
        if let Some(path) = &described {
            let span = Span::synthetic();
            let new = Expr::new(
                span,
                ExprNode::Send {
                    recv: Some(constant(path, span)),
                    method: Symbol::from("new"),
                    args: Vec::new(),
                    block: None,
                    parenthesized: false,
                },
            );
            lets.push(("subject".into(), new, span));
        }
        for frame in chain {
            for (let_name, body, span) in &frame.lets {
                lets.retain(|(n, _, _)| n != let_name);
                lets.push((let_name.clone(), body.clone(), *span));
            }
        }
        let mut defs: Vec<MethodDef> = Vec::new();
        for frame in chain {
            for def in &frame.defs {
                defs.retain(|d| d.name != def.name);
                defs.push(def.clone());
            }
        }

        let rewrite = |e: &Expr| -> Result<Expr, String> { desugar(e, described.as_deref()) };
        let mut helpers = Vec::new();
        for (let_name, body, span) in &lets {
            let body = match rewrite(body) {
                Ok(b) => b,
                Err(what) => {
                    super::survey::continue_or_fail(IngestError::Unsupported {
                        file: self.file.into(),
                        message: format!("RSpec: {what} in `let(:{let_name})` is not supported"),
                    })?;
                    return Ok(());
                }
            };
            helpers.push(let_method(let_name, body, *span, &class));
        }
        for mut def in defs {
            match rewrite(&def.body) {
                Ok(b) => def.body = b,
                Err(what) => {
                    super::survey::continue_or_fail(IngestError::Unsupported {
                        file: self.file.into(),
                        message: format!("RSpec: {what} in `def {}` is not supported", def.name.as_str()),
                    })?;
                    return Ok(());
                }
            }
            def.enclosing_class = Some(class.0.clone());
            helpers.push(def);
        }

        let mut setup_stmts = Vec::new();
        for hook in chain.iter().flat_map(|f| f.hooks.iter()) {
            match rewrite(hook) {
                Ok(h) => setup_stmts.push(h),
                Err(what) => {
                    super::survey::continue_or_fail(IngestError::Unsupported {
                        file: self.file.into(),
                        message: format!("RSpec: {what} in a `before` hook is not supported"),
                    })?;
                    return Ok(());
                }
            }
        }
        let setup = (!setup_stmts.is_empty())
            .then(|| Expr::new(Span::synthetic(), ExprNode::Seq { exprs: setup_stmts }));

        let mut tests = Vec::new();
        let mut seen: HashMap<String, usize> = HashMap::new();
        for (desc, node, body) in examples {
            let body = match body {
                ExampleBody::Pending => continue,
                ExampleBody::Empty => nil(node_span(&node, self.file)),
                ExampleBody::Body(body) => ingest_expr(&body, self.file)?,
            };
            let body = match rewrite(&body) {
                Ok(b) => b,
                Err(what) => {
                    self.refuse(&node, &what)?;
                    continue;
                }
            };
            let count = seen.entry(desc.clone()).or_insert(0);
            *count += 1;
            let name = if *count == 1 { desc } else { format!("{desc} {count}") };
            tests.push(Test { name, body });
        }
        if tests.is_empty() {
            return Ok(());
        }

        self.modules.push(TestModule {
            name: class,
            parent: Some(spec_type.parent()),
            target: described.map(|p| ClassId(Symbol::from(p.join("::")))),
            tests,
            setup,
            inner_classes: Vec::new(),
            helpers,
            constants: Vec::new(),
            includes: Vec::new(),
        });
        Ok(())
    }
}

/// What an example runs.
enum ExampleBody<'pr> {
    /// No block: pending in RSpec, reported and never run, so not a test.
    Pending,
    /// `it "x" do end`: passes in RSpec, as an empty test does.
    Empty,
    Body(Node<'pr>),
}

/// The one symbol argument of `let(:name)`, `before(:each)`, ...
fn single_symbol_arg(call: &ruby_prism::CallNode<'_>) -> Option<String> {
    let args = call.arguments()?;
    let mut iter = args.arguments().iter();
    let first = iter.next()?;
    if iter.next().is_some() {
        return None;
    }
    let sym = first.as_symbol_node()?;
    Some(String::from_utf8_lossy(sym.unescaped()).into_owned())
}

/// `def <name>` memoizing `body` the way RSpec's `let` does: evaluated on
/// the first call in an example, the same value on every later call, nil
/// and false included (a `||=` would evaluate those again).
///
/// ```ruby
/// def article
///   unless @__let_set_article
///     @__let_article = <body>
///     @__let_set_article = true
///   end
///   @__let_article
/// end
/// ```
fn let_method(name: &str, body: Expr, span: Span, owner: &ClassId) -> MethodDef {
    let stem = name.replace('?', "_p").replace('!', "_b");
    let value = Symbol::from(format!("__let_{stem}"));
    let set = Symbol::from(format!("__let_set_{stem}"));
    let syn = Span::synthetic();
    let ivar = |n: &Symbol| Expr::new(syn, ExprNode::Ivar { name: n.clone() });
    let assign = |n: &Symbol, v: Expr| {
        Expr::new(syn, ExprNode::Assign { target: LValue::Ivar { name: n.clone() }, value: v })
    };
    let first_call = Expr::new(
        syn,
        ExprNode::Seq {
            exprs: vec![
                assign(&value, body),
                assign(&set, Expr::new(syn, ExprNode::Lit { value: Literal::Bool { value: true } })),
            ],
        },
    );
    let memo = Expr::new(
        syn,
        ExprNode::If { cond: ivar(&set), then_branch: nil(syn), else_branch: first_call },
    );
    MethodDef {
        name_span: span,
        name: Symbol::from(name),
        receiver: MethodReceiver::Instance,
        visibility: MethodVisibility::Public,
        params: Vec::new(),
        unsupported_formals: None,
        has_anonymous_block: false,
        body: Expr::new(syn, ExprNode::Seq { exprs: vec![memo, ivar(&value)] }),
        signature: None,
        effects: EffectSet::default(),
        enclosing_class: Some(owner.0.clone()),
        kind: AccessorKind::Method,
        is_async: false,
        mutates_self: false,
        block_param: None,
    }
}

fn nil(span: Span) -> Expr {
    Expr::new(span, ExprNode::Lit { value: Literal::Nil })
}

fn constant(path: &[String], span: Span) -> Expr {
    Expr::new(span, ExprNode::Const { path: path.iter().map(|s| Symbol::from(s.as_str())).collect() })
}

fn self_call(name: &str, span: Span) -> Expr {
    send(None, name, Vec::new(), span)
}

fn send(recv: Option<Expr>, method: &str, args: Vec<Expr>, span: Span) -> Expr {
    let parenthesized = recv.is_none() && !args.is_empty();
    Expr::new(
        span,
        ExprNode::Send { recv, method: Symbol::from(method), args, block: None, parenthesized },
    )
}

/// Read RSpec's runtime vocabulary in `expr` into plain Ruby and test
/// assertions: expectations become `assert` / `refute`, `described_class`
/// the constant. `Err` names the first construct this subset does not
/// read; the caller refuses the example (or hook, or let) that holds it.
fn desugar(expr: &Expr, described: Option<&[String]>) -> Result<Expr, String> {
    if let Some(rewritten) = expectation(expr, described)? {
        return Ok(rewritten);
    }
    if let ExprNode::Send { recv: None, method, args, block: None, .. } = &*expr.node {
        match method.as_str() {
            "described_class" if args.is_empty() => {
                return match described {
                    Some(path) => Ok(constant(path, expr.span)),
                    None => Err("`described_class` in a group that describes no constant".into()),
                };
            }
            "expect" | "is_expected" | "expect_any_instance_of" | "allow" | "allow_any_instance_of"
            | "double" | "instance_double" | "class_double" | "spy" | "pending" | "skip"
            | "aggregate_failures" => {
                return Err(format!("`{}` here", method.as_str()));
            }
            _ => {}
        }
    }
    if let ExprNode::Send { recv: None, method, block: Some(_), .. } = &*expr.node {
        if method.as_str() == "expect" {
            return Err("block expectations (`expect { ... }`)".into());
        }
    }
    let mut out = expr.clone();
    let mut failure: Option<String> = None;
    out.node.for_each_child_mut(&mut |child| {
        if failure.is_some() {
            return;
        }
        match desugar(child, described) {
            Ok(c) => *child = c,
            Err(e) => failure = Some(e),
        }
    });
    match failure {
        Some(e) => Err(e),
        None => Ok(out),
    }
}

/// `expect(actual).to matcher`, `expect(actual).not_to matcher`,
/// `is_expected.to matcher`: the assertion that means the same. `Ok(None)`
/// when `expr` is not an expectation.
fn expectation(expr: &Expr, described: Option<&[String]>) -> Result<Option<Expr>, String> {
    let ExprNode::Send { recv: Some(target), method, args, block, .. } = &*expr.node else {
        return Ok(None);
    };
    let positive = match method.as_str() {
        "to" => true,
        "not_to" | "to_not" => false,
        _ => return Ok(None),
    };
    let ExprNode::Send { recv: None, method: target_method, args: target_args, block: target_block, .. } =
        &*target.node
    else {
        return Ok(None);
    };
    let actual = match (target_method.as_str(), target_args.as_slice(), target_block) {
        ("expect", [actual], None) => desugar(actual, described)?,
        ("is_expected", [], None) => self_call("subject", target.span),
        ("expect", [], Some(_)) => return Err("block expectations (`expect { ... }`)".into()),
        ("expect" | "is_expected", _, _) => return Err("this form of `expect`".into()),
        _ => return Ok(None),
    };
    if block.is_some() {
        return Err("a block passed to `to` / `not_to`".into());
    }
    let [matcher] = args.as_slice() else {
        return Err("`to` / `not_to` without exactly one matcher".into());
    };
    let span = expr.span;
    let conditions = matcher_conditions(matcher, actual, described)?;
    // `include(a, b)` holds when every value is included; its negation
    // when none is, so each value is its own assertion either way.
    let mut asserts: Vec<Expr> = conditions
        .into_iter()
        .map(|(cond, truthy)| send(None, if truthy == positive { "assert" } else { "refute" }, vec![cond], span))
        .collect();
    Ok(Some(if asserts.len() == 1 {
        asserts.pop().unwrap()
    } else {
        Expr::new(span, ExprNode::Seq { exprs: asserts })
    }))
}

/// What `matcher` checks of `actual`, as conditions that hold when the
/// matcher matches: `(cond, true)` means the match is `cond` being truthy,
/// `(cond, false)` that it is `cond` being falsy (`be_falsey`).
fn matcher_conditions(
    matcher: &Expr,
    actual: Expr,
    described: Option<&[String]>,
) -> Result<Vec<(Expr, bool)>, String> {
    let ExprNode::Send { recv: None, method, args, block: None, .. } = &*matcher.node else {
        return Err("this matcher".into());
    };
    let span = matcher.span;
    let args: Vec<Expr> = args.iter().map(|a| desugar(a, described)).collect::<Result<_, _>>()?;
    let name = method.as_str();
    let call = |m: &str, args: Vec<Expr>| send(Some(actual.clone()), m, args, span);
    let one = |args: &[Expr]| -> Result<Expr, String> {
        match args {
            [a] => Ok(a.clone()),
            _ => Err(format!("`{name}` without exactly one argument")),
        }
    };
    Ok(match name {
        "eq" => vec![(call("==", vec![one(args.as_slice())?]), true)],
        "eql" => vec![(call("eql?", vec![one(args.as_slice())?]), true)],
        "equal" | "be" if !args.is_empty() => vec![(call("equal?", vec![one(args.as_slice())?]), true)],
        "be_nil" if args.is_empty() => vec![(call("nil?", Vec::new()), true)],
        "be_truthy" if args.is_empty() => vec![(actual, true)],
        "be_falsey" | "be_falsy" if args.is_empty() => vec![(actual, false)],
        "be_a" | "be_an" | "be_kind_of" | "be_a_kind_of" => vec![(call("kind_of?", vec![one(args.as_slice())?]), true)],
        "be_instance_of" | "be_an_instance_of" => vec![(call("instance_of?", vec![one(args.as_slice())?]), true)],
        "include" if !args.is_empty() => {
            if args.iter().any(|a| matches!(&*a.node, ExprNode::Hash { .. })) {
                return Err("`include` with a Hash (key/value inclusion)".into());
            }
            args.into_iter().map(|a| (call("include?", vec![a]), true)).collect()
        }
        // `be_true` / `be_false` left RSpec in 3.0; `be_truthy` replaced them.
        "be_true" | "be_false" => return Err(format!("`{name}` (removed in RSpec 3)")),
        // Matchers RSpec, rspec-rails and Capybara define under a `be_` or
        // `have_` name; the dynamic predicate rule below would read them
        // as a call to a predicate the object does not have.
        "be_a_new" | "be_within" | "be_routable" | "have_attributes" | "have_http_status"
        | "have_received" | "have_been_enqueued" | "have_enqueued_job" | "have_enqueued_mail"
        | "have_been_performed" | "have_performed_job" | "have_broadcasted_to" | "have_been_made"
        | "have_content" | "have_text" | "have_css" | "have_selector" | "have_current_path"
        | "have_link" | "have_button" | "have_field" | "have_rendered" | "have_enqueued_sidekiq_job" => {
            return Err(format!("the `{name}` matcher"));
        }
        _ => {
            if let Some(pred) = name.strip_prefix("be_") {
                vec![(call(&format!("{pred}?"), args), true)]
            } else if let Some(pred) = name.strip_prefix("have_") {
                vec![(call(&format!("has_{pred}?"), args), true)]
            } else {
                return Err(format!("the `{name}` matcher"));
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const ARTICLE: &str = r#"
require "rails_helper"

RSpec.describe Article, type: :model do
  let(:title) { "Hello" }
  let(:article) { Article.new(title: title, body: "Long enough body") }

  it "is valid" do
    expect(article).to be_valid
  end

  context "without a title" do
    let(:title) { nil }

    before { article.validate }

    it "is invalid" do
      expect(article).not_to be_valid
      expect(article.errors[:title]).to include("can't be blank")
    end
  end
end
"#;

    fn ingest(src: &str) -> Vec<TestModule> {
        ingest_spec_files(src.as_bytes(), "/app/spec/models/article_spec.rb").expect("ingest")
    }

    fn helper<'a>(tm: &'a TestModule, name: &str) -> &'a MethodDef {
        tm.helpers.iter().find(|h| h.name.as_str() == name).unwrap_or_else(|| panic!("no helper {name}"))
    }

    #[test]
    fn every_group_with_examples_is_a_module_named_by_file_and_group() {
        let tms = ingest(ARTICLE);
        let names: Vec<&str> = tms.iter().map(|tm| tm.name.0.as_str()).collect();
        assert_eq!(names, ["ModelsArticleSpec", "ModelsArticleWithoutATitleSpec"]);
        assert!(tms.iter().all(|tm| tm.target.as_ref().map(|t| t.0.as_str()) == Some("Article")));
        assert!(tms.iter().all(|tm| tm.parent.as_ref().map(|p| p.0.as_str()) == Some("ActiveSupport::TestCase")));
        assert_eq!(tms[0].tests.len(), 1);
        assert_eq!(tms[1].tests[0].name, "is invalid");
    }

    // An inner `let` replaces the outer one in the inner group only.
    #[test]
    fn an_inner_let_overrides_the_outer_one() {
        let tms = ingest(ARTICLE);
        let outer = format!("{:?}", helper(&tms[0], "title").body);
        let inner = format!("{:?}", helper(&tms[1], "title").body);
        assert!(outer.contains("Hello"), "{outer}");
        assert!(inner.contains("Nil") && !inner.contains("Hello"), "{inner}");
        // Both groups still see `article`, and the implicit subject.
        helper(&tms[1], "article");
        helper(&tms[1], "subject");
    }

    #[test]
    fn hooks_of_enclosing_groups_become_the_setup() {
        let tms = ingest(ARTICLE);
        assert!(tms[0].setup.is_none());
        let setup = format!("{:?}", tms[1].setup.as_ref().expect("setup"));
        assert!(setup.contains("validate"), "{setup}");
    }

    // `expect(a).to eq(b)` keeps RSpec's operand order: `a == b`.
    #[test]
    fn expectations_become_assertions() {
        let src = "RSpec.describe Article do\n  it \"x\" do\n    expect(1 + 1).to eq(2)\n    expect(nil).not_to be_truthy\n  end\nend\n";
        let tms = ingest(src);
        let body = &tms[0].tests[0].body;
        let ExprNode::Seq { exprs } = &*body.node else { panic!("{body:?}") };
        let ExprNode::Send { recv: None, method, args, .. } = &*exprs[0].node else { panic!("{:?}", exprs[0]) };
        assert_eq!(method.as_str(), "assert");
        let ExprNode::Send { recv: Some(lhs), method: eq, .. } = &*args[0].node else { panic!("{:?}", args[0]) };
        assert_eq!(eq.as_str(), "==");
        assert!(matches!(&*lhs.node, ExprNode::Send { method, .. } if method.as_str() == "+"));
        let ExprNode::Send { method, .. } = &*exprs[1].node else { panic!() };
        assert_eq!(method.as_str(), "refute");
    }

    #[test]
    fn let_bang_runs_at_its_place_among_the_hooks() {
        let src = "RSpec.describe Article do\n  before { setup_one }\n  let!(:article) { Article.create }\n  before { setup_two }\n  it \"x\" do\n    expect(article).to be_persisted\n  end\nend\n";
        let tms = ingest(src);
        let setup = tms[0].setup.as_ref().unwrap();
        let ExprNode::Seq { exprs } = &*setup.node else { panic!("{setup:?}") };
        let methods: Vec<String> = exprs
            .iter()
            .map(|e| match &*e.node {
                ExprNode::Send { method, .. } => method.as_str().to_string(),
                other => format!("{other:?}"),
            })
            .collect();
        assert_eq!(methods, ["setup_one", "article", "setup_two"]);
    }

    #[test]
    fn duplicate_descriptions_get_distinct_test_names() {
        let src = "describe Article do\n  it \"works\" do\n    expect(1).to eq(1)\n  end\n  it \"works\" do\n    expect(2).to eq(2)\n  end\nend\n";
        let tms = ingest(src);
        let names: Vec<&str> = tms[0].tests.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, ["works", "works 2"]);
    }

    #[test]
    fn pending_examples_are_not_tests() {
        let src = "describe Article do\n  it \"later\"\n  xit \"off\" do\n    raise\n  end\n  it \"now\" do\n    expect(1).to eq(1)\n  end\nend\n";
        let tms = ingest(src);
        let names: Vec<&str> = tms[0].tests.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, ["now"]);
    }

    #[test]
    fn an_unsupported_matcher_is_refused_by_name() {
        let src = "describe Article do\n  it \"x\" do\n    expect(Article.all).to contain_exactly(1)\n  end\nend\n";
        let err = ingest_spec_files(src.as_bytes(), "/app/spec/models/article_spec.rb").unwrap_err();
        assert!(err.to_string().contains("contain_exactly"), "{err}");
    }

    #[test]
    fn mocks_and_block_expectations_are_refused() {
        for body in ["allow(Article).to receive(:count).and_return(1)", "expect { Article.create! }.to raise_error"] {
            let src = format!("describe Article do\n  it \"x\" do\n    {body}\n  end\nend\n");
            assert!(ingest_spec_files(src.as_bytes(), "/app/spec/models/article_spec.rb").is_err(), "{body}");
        }
    }

    #[test]
    fn group_metadata_other_than_type_is_refused() {
        let src = "describe Article, :site_manager do\n  it \"x\" do\n    expect(1).to eq(1)\n  end\nend\n";
        assert!(ingest_spec_files(src.as_bytes(), "/app/spec/models/article_spec.rb").is_err());
    }

    // A `*_spec.rb` holding a test class (not an example group) is not
    // an RSpec file; ingest reads it as before.
    #[test]
    fn only_example_groups_make_an_rspec_file() {
        assert!(is_rspec_source(ARTICLE.as_bytes()));
        assert!(is_rspec_source(b"RSpec.shared_examples \"x\" do\nend\n"));
        assert!(!is_rspec_source(b"class ThingSpec\n  def path\n    1\n  end\nend\n"));
    }

    #[test]
    fn a_request_spec_is_found_by_its_folder_without_a_leading_slash() {
        let src = "describe \"Articles\" do\n  it \"x\" do\n    expect(1).to eq(1)\n  end\nend\n";
        let tms = ingest_spec_files(src.as_bytes(), "spec/requests/articles_spec.rb").expect("ingest");
        assert_eq!(tms[0].parent.as_ref().map(|p| p.0.as_str()), Some("ActionDispatch::IntegrationTest"));
    }

    #[test]
    fn request_specs_are_integration_tests() {
        let src = "RSpec.describe \"Articles\", type: :request do\n  it \"x\" do\n    expect(1).to eq(1)\n  end\nend\n";
        let tms = ingest_spec_files(src.as_bytes(), "/app/spec/requests/articles_spec.rb").expect("ingest");
        assert_eq!(tms[0].name.0.as_str(), "RequestsArticlesSpec");
        assert_eq!(tms[0].parent.as_ref().unwrap().0.as_str(), "ActionDispatch::IntegrationTest");
        assert!(tms[0].target.is_none());
    }
}
