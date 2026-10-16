//! dry-monads' Result and do notation, lowered into the plain Ruby they
//! stand for.
//!
//! ```ruby
//! class CreateUser
//!   include Dry::Monads[:result, :do]
//!
//!   def call(params)
//!     user = yield validate(params)
//!     Success(user)
//!   end
//! end
//! ```
//!
//! `include Dry::Monads[:result]` becomes an include of a stand-in mixin
//! whose `Success(v)` and `Failure(v)` build stand-in results answering
//! what dry-monads' do: `success?`, `failure?`, `value!` (a Failure's
//! raises `UnwrapError`), `success`, `failure`, `bind`, `fmap`, `tee`,
//! `or`, `value_or`, `either`, `to_monad`, `==`, `to_s`/`inspect`.
//!
//! `:do` wraps a class's instance methods (its subclasses' and includers'
//! too, as `Do::All` does): in each that `yield`s, `yield m` becomes
//! `Do.bind([m])`, which unwraps a Success and raises `Halt` with a
//! Failure, and the body rescues `Halt` into its Failure. Called with a
//! block, the gem's wrapper steps aside: `yield` stays the caller's then.
//! A method that takes a block or asks `block_given?` itself is left
//! alone, as under the wrapper it always sees one.
//!
//! `include Dry::Monads::Do.for(:call)` wraps just those methods, defined
//! after it in that class, as the gem's `method_added` hook does.
//!
//! Checked against dry-monads 1.9. Other monads (`:maybe`, `:try`,
//! `:list`) are not modeled: an include naming one the app builds
//! (`Some(..)`, `Try { }`, `List[..]`) stays in the class body, ledgered,
//! beside what it names that is; one the app never builds changes
//! nothing and is dropped.

use std::collections::{HashMap, HashSet};

use crate::App;
use crate::dialect::{LibraryClass, LibraryClassOrigin, MethodReceiver};
use crate::expr::{Expr, ExprNode, Literal, RescueClause};
use crate::ident::{ClassId, Symbol, VarId};

use super::IngestError;
use super::survey;

const RESULT_MIXIN: &str = "Dry::Monads::Result::Mixin";

pub(super) fn lower_dry_monads(app: &mut App, sources: &[crate::span::SourceFile]) -> super::IngestResult<()> {
    let names: HashSet<String> = app.library_classes.iter().map(|lc| lc.name.0.as_str().to_string()).collect();
    let mut any = false;
    let mut do_marked: HashSet<String> = HashSet::new();
    // `Do.for(...)`: per class, the methods and where the include stands.
    let mut do_for: HashMap<String, HashMap<String, crate::span::Span>> = HashMap::new();
    // A monad the app never builds is an include that changes nothing.
    let unused = |feature: &str| -> bool {
        let constructors: &[&str] = match feature {
            "maybe" => &["Maybe", "Some", "None"],
            "try" => &["Try"],
            "list" => &["List"],
            _ => return false,
        };
        !sources.iter().any(|s| {
            constructors.iter().any(|c| {
                s.text.match_indices(c).any(|(at, _)| {
                    let before = s.text[..at].chars().next_back();
                    let after = &s.text[at + c.len()..];
                    // `Some(1)`, `Some (1)`, `List [1]`, `Try { }`, `Try do`.
                    let spaced = after.trim_start_matches([' ', '\t']);
                    !before.is_some_and(|b| b.is_alphanumeric() || b == '_' || b == ':')
                        && (spaced.starts_with('(')
                            || spaced.starts_with('[')
                            || (spaced.len() < after.len() && (spaced.starts_with('{') || spaced.starts_with("do"))))
                })
            })
        })
    };
    // A gap in survey mode, a failed ingest otherwise.
    let mut refusals: Vec<IngestError> = Vec::new();
    for lc in &mut app.library_classes {
        let mut result = false;
        let mut do_notation = false;
        let mut refused = None;
        // Each named method and the first include naming it.
        let mut for_methods: Option<HashMap<String, crate::span::Span>> = None;
        lc.unknown_calls.retain(|call| match features(call) {
            None => true,
            // What is modeled applies; the include stays for the rest.
            Some(Ok(fs)) => {
                let mut kept = false;
                for f in fs {
                    match f.as_str() {
                        "result" => result = true,
                        "do" => do_notation = true,
                        other if other.starts_with("for:") => {
                            for_methods
                                .get_or_insert_with(HashMap::new)
                                .entry(other["for:".len()..].to_string())
                                .or_insert(call.span);
                        }
                        other if unused(other) => {}
                        other => {
                            refused = Some(format!("`:{other}`"));
                            kept = true;
                        }
                    }
                }
                kept
            }
            Some(Err(reason)) => {
                refused = Some(reason);
                true
            }
        });
        if let Some(reason) = refused {
            refusals.push(IngestError::Unsupported {
                file: lc.name.0.as_str().to_string(),
                message: format!("Dry::Monads not lowered: {reason}"),
            });
        }
        if result {
            let mixin = ClassId(Symbol::from(RESULT_MIXIN));
            if !lc.includes.contains(&mixin) {
                lc.includes.push(mixin);
            }
            any = true;
        }
        if do_notation {
            do_marked.insert(lc.name.0.as_str().to_string());
            any = true;
        }
        if let Some(found) = for_methods {
            do_for.insert(lc.name.0.as_str().to_string(), found);
            any = true;
        }
    }
    for err in refusals {
        survey::continue_or_fail(err)?;
    }
    let validation_monads = sources.iter().any(|s| s.text.contains("load_extensions(:monads)"));
    if !any && !validation_monads {
        return Ok(());
    }

    // `Do::All` follows the class to its subclasses and includers.
    let mut in_scope = do_marked.clone();
    loop {
        let before = in_scope.len();
        for lc in &app.library_classes {
            let name = lc.name.0.as_str();
            if in_scope.contains(name) {
                continue;
            }
            let parent = lc.parent.as_ref().and_then(|p| {
                super::dry_types::resolve(name, p.0.as_str().trim_start_matches("::"), &names)
            });
            let included = lc.includes.iter().any(|i| in_scope.contains(i.0.as_str()));
            if parent.is_some_and(|p| in_scope.contains(&p)) || included {
                in_scope.insert(name.to_string());
            }
        }
        if in_scope.len() == before {
            break;
        }
    }
    for lc in &mut app.library_classes {
        let all = in_scope.contains(lc.name.0.as_str());
        // `Do.for` wraps the named methods the class defines after the
        // include (its `method_added` hook), in the class itself only: a
        // subclass's own definition is found before the wrapper.
        let listed = do_for.get(lc.name.0.as_str()).cloned();
        if !all && listed.is_none() {
            continue;
        }
        for method in &mut lc.methods {
            if method.receiver != MethodReceiver::Instance || !yields(&method.body) {
                continue;
            }
            let wrapped = all
                || listed.as_ref().and_then(|named| named.get(method.name.as_str())).is_some_and(|at| {
                    method.name_span.file == at.file && method.name_span.start > at.start
                });
            if !wrapped {
                continue;
            }
            if method.block_param.is_some() || method.has_anonymous_block || asks_block_given(&method.body) {
                survey::continue_or_fail(IngestError::Unsupported {
                    file: lc.name.0.as_str().to_string(),
                    message: format!("Dry::Monads do notation not lowered: `{}` takes a block", method.name.as_str()),
                })?;
                continue;
            }
            let span = method.body.span;
            let body = unwrap_yields(&method.body);
            method.body = Expr::new(
                span,
                ExprNode::BeginRescue {
                    body,
                    rescues: vec![RescueClause {
                        classes: vec![constant(span, &["Dry", "Monads", "Do", "Halt"])],
                        binding: Some(Symbol::from("dry_monads_halt")),
                        body: Expr::new(
                            span,
                            ExprNode::Send {
                                recv: Some(Expr::new(
                                    span,
                                    ExprNode::Var { id: VarId(0), name: Symbol::from("dry_monads_halt") },
                                )),
                                method: Symbol::from("result"),
                                args: Vec::new(),
                                block: None,
                                parenthesized: false,
                            },
                        ),
                    }],
                    else_branch: None,
                    ensure: None,
                    implicit: false,
                },
            );
        }
    }

    let mut classes = stand_ins();
    // dry-validation's `:monads` extension: a result is a Success or a
    // Failure of itself.
    if validation_monads
        && let Some(result) = app.library_classes.iter_mut().find(|lc| lc.name.0.as_str() == "Dry::Validation::Result")
    {
        let source = "class Result\n  def to_monad\n    success? ? ::Dry::Monads::Result::Success.new(self) : ::Dry::Monads::Result::Failure.new(self)\n  end\nend\n";
        if let Ok(parsed) = crate::ingest::ingest_library_classes(source.as_bytes(), "<dry_monads>") {
            result.methods.extend(parsed.into_iter().flat_map(|c| c.methods));
        }
    }
    for c in &mut classes {
        c.origin = Some(LibraryClassOrigin::DryMonads);
    }
    let existing: HashSet<String> = app.library_classes.iter().map(|lc| lc.name.0.as_str().to_string()).collect();
    app.library_classes.extend(classes.into_iter().filter(|c| !existing.contains(c.name.0.as_str())));
    Ok(())
}

/// `include Dry::Monads[:result, :do]`, `include Dry::Monads::Do`,
/// `include Dry::Monads::Result::Mixin`: the features it asks for.
fn features(call: &Expr) -> Option<Result<Vec<String>, String>> {
    let ExprNode::Send { recv: None, method, args, block: None, .. } = &*call.node else { return None };
    if method.as_str() != "include" {
        return None;
    }
    let [arg] = args.as_slice() else { return None };
    let path = |e: &Expr| match &*e.node {
        ExprNode::Const { path } => Some(path.iter().map(|s| s.as_str()).filter(|s| !s.is_empty()).collect::<Vec<_>>().join("::")),
        _ => None,
    };
    match &*arg.node {
        ExprNode::Send { recv: Some(recv), method, args, .. } if method.as_str() == "[]" && path(recv).as_deref() == Some("Dry::Monads") => {
            let mut out = Vec::new();
            for a in args {
                match &*a.node {
                    ExprNode::Lit { value: Literal::Sym { value } } => out.push(value.as_str().to_string()),
                    _ => return Some(Err(format!("`{}`", crate::emit::ruby::emit_expr(arg)))),
                }
            }
            Some(Ok(out))
        }
        // `Do.for(:call, :run)`: those methods only.
        ExprNode::Send { recv: Some(recv), method, args, .. }
            if method.as_str() == "for" && path(recv).as_deref() == Some("Dry::Monads::Do") =>
        {
            let mut out = Vec::new();
            for a in args {
                match &*a.node {
                    ExprNode::Lit { value: Literal::Sym { value } } => out.push(format!("for:{}", value.as_str())),
                    _ => return Some(Err(format!("`include {}`", crate::emit::ruby::emit_expr(arg)))),
                }
            }
            Some(Ok(out))
        }
        // `Result(Error)` and the like: forms of the gem this does not model.
        ExprNode::Send { recv: Some(recv), .. }
            if path(recv).is_some_and(|p| p == "Dry::Monads" || p.starts_with("Dry::Monads::")) =>
        {
            Some(Err(format!("`include {}`", crate::emit::ruby::emit_expr(arg))))
        }
        _ => match path(arg).as_deref() {
            Some("Dry::Monads::Do" | "Dry::Monads::Do::All") => Some(Ok(vec!["do".into()])),
            Some("Dry::Monads::Result::Mixin") => Some(Ok(vec!["result".into()])),
            Some(other) if other.starts_with("Dry::Monads") => Some(Err(format!("`include {other}`"))),
            _ => None,
        },
    }
}

/// `::Dry::Monads::...` at an app source's span: marked generated, as no
/// constant is written there for the resolver to find.
fn constant(span: crate::span::Span, path: &[&str]) -> Expr {
    let mut segments = vec![Symbol::from("")];
    segments.extend(path.iter().map(|s| Symbol::from(*s)));
    let mut expr = Expr::new(span, ExprNode::Const { path: segments });
    expr.decisions |= crate::expr::GENERATED_CONST_REF;
    expr
}

fn yields(expr: &Expr) -> bool {
    let mut found = matches!(&*expr.node, ExprNode::Yield { .. });
    expr.node.for_each_child(&mut |c| found |= yields(c));
    found
}

fn asks_block_given(expr: &Expr) -> bool {
    let mut found = matches!(&*expr.node, ExprNode::Send { recv: None, method, .. } if matches!(method.as_str(), "block_given?" | "iterator?"));
    expr.node.for_each_child(&mut |c| found |= asks_block_given(c));
    found
}

/// `yield a, b` as `block_given? ? yield(a, b) : Do.bind([a, b])`: called
/// with a block, the gem's wrapper steps aside and `yield` is the
/// caller's; without one it unwraps.
fn unwrap_yields(expr: &Expr) -> Expr {
    let mut out = expr.clone();
    fn walk(expr: &mut Expr) {
        expr.node.for_each_child_mut(&mut |c| walk(c));
        if let ExprNode::Yield { args } = &*expr.node {
            let span = expr.span;
            let monads = Expr::new(span, ExprNode::Array { elements: args.clone(), style: Default::default() });
            let bind = Expr::new(
                span,
                ExprNode::Send {
                    recv: Some(constant(span, &["Dry", "Monads", "Do"])),
                    method: Symbol::from("bind"),
                    args: vec![monads],
                    block: None,
                    parenthesized: true,
                },
            );
            let block_given = Expr::new(
                span,
                ExprNode::Send {
                    recv: None,
                    method: Symbol::from("block_given?"),
                    args: Vec::new(),
                    block: None,
                    parenthesized: false,
                },
            );
            let caller = Expr::new(span, ExprNode::Yield { args: args.clone() });
            *expr.node = ExprNode::If { cond: block_given, then_branch: caller, else_branch: bind };
        }
    }
    walk(&mut out);
    out
}

/// What the lowered code calls once dry-monads is not there.
fn stand_ins() -> Vec<LibraryClass> {
    const SOURCE: &str = r#"module Dry
  module Monads
    class UnwrapError < StandardError
      def initialize(receiver)
        super("value! was called on #{receiver.inspect}")
      end
    end

    class UnitClass
      def inspect
        "Unit"
      end

      def to_s
        "Unit"
      end
    end

    Unit = ::Dry::Monads::UnitClass.new

    class Result
      def to_monad
        self
      end

      def to_result
        self
      end

      def dry_monads_value
        @value
      end

      def ==(other)
        other.class == self.class && other.dry_monads_value == @value
      end

      class Success < Result
        def initialize(value)
          @value = value
        end

        def value!
          @value
        end

        def success
          @value
        end

        def failure
          nil
        end

        def success?
          true
        end

        def failure?
          false
        end

        def bind(*args)
          block_given? ? yield(@value, *args) : args[0].call(@value, *args[1..])
        end

        def fmap(*args, &block)
          ::Dry::Monads::Result::Success.new(bind(*args, &block))
        end

        def tee(*args, &block)
          bind(*args, &block).bind { self }
        end

        def or(*args)
          self
        end

        def value_or(value = nil)
          @value
        end

        def either(on_success, on_failure)
          on_success.call(@value)
        end

        def to_s
          ::Dry::Monads::Unit.equal?(@value) ? "Success()" : "Success(#{@value.inspect})"
        end

        def inspect
          to_s
        end
      end

      class Failure < Result
        def initialize(value)
          @value = value
        end

        def value!
          raise ::Dry::Monads::UnwrapError.new(self)
        end

        def success
          nil
        end

        def failure
          @value
        end

        def success?
          false
        end

        def failure?
          true
        end

        def bind(*args)
          self
        end

        def fmap(*args)
          self
        end

        def tee(*args)
          self
        end

        def or(*args)
          block_given? ? yield(@value, *args) : args[0]
        end

        def value_or(value = nil)
          block_given? ? yield(@value) : value
        end

        def either(on_success, on_failure)
          on_failure.call(@value)
        end

        def to_s
          ::Dry::Monads::Unit.equal?(@value) ? "Failure()" : "Failure(#{@value.inspect})"
        end

        def inspect
          to_s
        end
      end

      module Mixin
        def Success(value = ::Dry::Monads::Unit)
          ::Dry::Monads::Result::Success.new(value)
        end

        def Failure(value = ::Dry::Monads::Unit)
          ::Dry::Monads::Result::Failure.new(value)
        end
      end
    end

    module Do
      class Halt < StandardError
        def initialize(result)
          super()
          @result = result
        end

        def result
          @result
        end
      end

      def self.bind(monads)
        values = monads.map do |monad|
          result = monad.to_monad
          raise ::Dry::Monads::Do::Halt.new(result) if result.failure?
          result.value!
        end
        monads.size == 1 ? values[0] : values
      end
    end
  end
end
"#;
    crate::ingest::ingest_library_classes(SOURCE.as_bytes(), "<dry_monads>").expect("the dry-monads stand-ins parse")
}
