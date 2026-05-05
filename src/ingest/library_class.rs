//! Library-class ingestion for files under `app/models/` whose class
//! does not extend `ApplicationRecord` / `ActiveRecord::Base` — for
//! example `ArticleCommentsProxy` produced by has_many specialization.
//! The model ingest's table-name/columns/associations/validations
//! machinery doesn't apply; we just collect methods and `include`
//! directives.

use ruby_prism::parse;

use crate::dialect::{LibraryClass, MethodDef, MethodReceiver, Param};
use crate::effect::EffectSet;
use crate::expr::{Expr, ExprNode, LValue};
use crate::ident::VarId;
use crate::span::Span;
use crate::{ClassId, Symbol};

use super::expr::ingest_expr;
use super::util::{
    class_name_path, constant_id_str, constant_path_of, find_all_classes, find_all_modules,
    find_first_class, flatten_statements, module_name_path, symbol_value,
};
use super::{IngestError, IngestResult};

pub fn ingest_library_class(
    source: &[u8],
    file: &str,
) -> IngestResult<Option<LibraryClass>> {
    let result = parse(source);
    let root = result.node();
    let Some(class) = find_first_class(&root) else {
        return Ok(None);
    };
    Ok(Some(library_class_from_node(&class, file)?))
}

/// Plural variant — returns one `LibraryClass` per class declaration
/// AND per module-as-namespace (a module whose body contains direct
/// `def`s) in the file, descending through nested classes and modules.
/// Used by the library-shape ingest path where a file like
/// `runtime/active_record/errors.rb` declares several classes side by
/// side inside one module, or like `runtime/inflector.rb` declares a
/// module-with-self-methods.
///
/// Modules-as-namespaces are lowered to `LibraryClass` with `parent:
/// None` (per the YAGNI-on-round-trip decision: surface
/// module-vs-class distinction is sacrificed for downstream
/// uniformity, which is fine when callers only use the module as a
/// dotted-call namespace). Mixin modules (whose instance methods get
/// `include`d into a class) are NOT handled by this path yet.
pub fn ingest_library_classes(
    source: &[u8],
    file: &str,
) -> IngestResult<Vec<LibraryClass>> {
    let result = parse(source);
    let root = result.node();
    let mut out = Vec::new();
    for class in find_all_classes(&root) {
        out.push(library_class_from_node(&class, file)?);
    }
    for module in find_all_modules(&root) {
        out.push(library_class_from_module_node(&module, file)?);
    }
    Ok(out)
}

pub(super) fn library_class_from_node(
    class: &ruby_prism::ClassNode<'_>,
    file: &str,
) -> IngestResult<LibraryClass> {
    let name_path = class_name_path(class).ok_or_else(|| IngestError::Unsupported {
        file: file.into(),
        message: "library class name must be a simple constant or path".into(),
    })?;
    let owner = ClassId(Symbol::from(name_path.join("::")));

    let parent = class.superclass().and_then(|n| {
        constant_path_of(&n).map(|p| ClassId(Symbol::from(p.join("::"))))
    });

    let (includes, methods) = walk_decl_body(class.body(), &owner, file, false)?;
    Ok(LibraryClass {
        name: owner,
        is_module: false,
        parent,
        includes,
        methods,
        origin: None,
    })
}

/// Same as `library_class_from_node` but for module-as-namespace
/// declarations — modules whose body has at least one direct `def`,
/// surfaced via `find_all_modules`. Lowered to a `LibraryClass` with
/// `is_module: true` and `parent: None`. The `is_module` flag is
/// load-bearing: callers using `include` on the result need it to be
/// emitted as `module`, not `class`, or Ruby will raise TypeError.
fn library_class_from_module_node(
    module: &ruby_prism::ModuleNode<'_>,
    file: &str,
) -> IngestResult<LibraryClass> {
    let name_path = module_name_path(module).ok_or_else(|| IngestError::Unsupported {
        file: file.into(),
        message: "library module name must be a simple constant or path".into(),
    })?;
    let owner = ClassId(Symbol::from(name_path.join("::")));

    let (includes, methods) = walk_decl_body(module.body(), &owner, file, false)?;
    Ok(LibraryClass {
        name: owner,
        is_module: true,
        parent: None,
        includes,
        methods,
        origin: None,
    })
}

/// Walk a class or module body, collecting `include` directives and
/// method definitions (with `attr_*` lowered to synthesized methods).
/// Other top-level calls (alias_method, etc.) and nested class/module
/// declarations are dropped — those surface separately via the plural
/// ingest entry points.
///
/// `force_class_receiver` is true when we're recursing into a
/// `class << self` block; it overrides every synthesized method's
/// receiver to `Class`, so e.g. `attr_accessor :adapter` inside
/// `class << self` produces class-level getter/setter pairs.
fn walk_decl_body<'pr>(
    body: Option<ruby_prism::Node<'pr>>,
    owner: &ClassId,
    file: &str,
    force_class_receiver: bool,
) -> IngestResult<(Vec<ClassId>, Vec<MethodDef>)> {
    let mut includes: Vec<ClassId> = Vec::new();
    let mut methods: Vec<MethodDef> = Vec::new();
    // `module_function` (called bare inside a module body) marks every
    // subsequent direct `def` as a module-function — both an instance
    // method AND a class method. For our targets (which call these as
    // `Mod.x(...)`), we only need the class-method form, so flip the
    // receiver to Class. Doesn't affect nested `class`/`module` bodies
    // — they get their own walk_decl_body recursion.
    let mut module_function_active = false;

    let Some(b) = body else {
        return Ok((includes, methods));
    };

    for stmt in flatten_statements(b) {
        if let Some(def) = stmt.as_def_node() {
            let mut m = ingest_library_method(&def, owner, file)?;
            if force_class_receiver || module_function_active {
                m.receiver = MethodReceiver::Class;
            }
            methods.push(m);
            continue;
        }
        // `class << self ... end` — singleton class block. Body
        // defines class-level methods on the enclosing scope.
        if let Some(sc) = stmt.as_singleton_class_node() {
            let (inner_includes, inner_methods) =
                walk_decl_body(sc.body(), owner, file, true)?;
            includes.extend(inner_includes);
            methods.extend(inner_methods);
            continue;
        }
        if let Some(call) = stmt.as_call_node() {
            if call.receiver().is_none() {
                let kw = constant_id_str(&call.name());
                match kw {
                    "include" => {
                        if let Some(args) = call.arguments() {
                            for arg in args.arguments().iter() {
                                if let Some(path) = constant_path_of(&arg) {
                                    includes.push(ClassId(Symbol::from(path.join("::"))));
                                }
                            }
                        }
                    }
                    "attr_reader" | "attr_writer" | "attr_accessor" => {
                        // Lower to method definitions at ingest time
                        // (per the YAGNI-on-round-trip decision):
                        //   attr_reader :foo  → def foo; @foo; end
                        //   attr_writer :foo  → def foo=(v); @foo = v; end
                        //   attr_accessor :foo → both
                        let mut names: Vec<Symbol> = Vec::new();
                        if let Some(args) = call.arguments() {
                            for arg in args.arguments().iter() {
                                if let Some(s) = symbol_value(&arg) {
                                    names.push(Symbol::from(s));
                                }
                            }
                        }
                        let recv = if force_class_receiver {
                            MethodReceiver::Class
                        } else {
                            MethodReceiver::Instance
                        };
                        for name in &names {
                            let want_reader = matches!(kw, "attr_reader" | "attr_accessor");
                            let want_writer = matches!(kw, "attr_writer" | "attr_accessor");
                            if want_reader {
                                methods.push(synth_attr_reader(owner, name, recv));
                            }
                            if want_writer {
                                methods.push(synth_attr_writer(owner, name, recv));
                            }
                        }
                    }
                    "module_function" => {
                        // Bare `module_function` (no args) — flip the
                        // flag for every subsequent direct `def` in
                        // this body. The arg-bearing form
                        // (`module_function :foo, :bar`) isn't yet
                        // handled; add when a runtime file uses it.
                        if call.arguments().is_none() {
                            module_function_active = true;
                        }
                    }
                    _ => {
                        // Other top-level calls (alias_method, etc.) —
                        // drop on the floor for now.
                    }
                }
            }
        }
        // Nested class/module declarations also fall through here; they
        // surface as separate entries via the plural API.
    }

    Ok((includes, methods))
}

/// Synthesize `def <name>; @<name>; end` (instance receiver) or
/// `def self.<name>; @<name>; end` (class receiver).
fn synth_attr_reader(owner: &ClassId, name: &Symbol, receiver: MethodReceiver) -> MethodDef {
    let body = Expr::new(
        Span::synthetic(),
        ExprNode::Ivar { name: name.clone() },
    );
    MethodDef {
        name: name.clone(),
        receiver,
        params: Vec::new(),
        body,
        signature: None,
        effects: EffectSet::default(),
        enclosing_class: Some(owner.0.clone()),
        kind: crate::dialect::AccessorKind::AttributeReader,
    }
}

/// Synthesize the writer pair for `attr_writer` / `attr_accessor`,
/// honoring the receiver (Instance vs Class).
fn synth_attr_writer(owner: &ClassId, name: &Symbol, receiver: MethodReceiver) -> MethodDef {
    let value_param = Symbol::from("value");
    let rhs = Expr::new(
        Span::synthetic(),
        ExprNode::Var {
            id: VarId(0),
            name: value_param.clone(),
        },
    );
    let body = Expr::new(
        Span::synthetic(),
        ExprNode::Assign {
            target: LValue::Ivar { name: name.clone() },
            value: rhs,
        },
    );
    let setter_name = Symbol::from(format!("{}=", name.as_str()));
    MethodDef {
        name: setter_name,
        receiver,
        params: vec![Param::positional(value_param)],
        body,
        signature: None,
        effects: EffectSet::default(),
        enclosing_class: Some(owner.0.clone()),
        kind: crate::dialect::AccessorKind::AttributeWriter,
    }
}

pub(super) fn ingest_library_method(
    def: &ruby_prism::DefNode<'_>,
    owner: &ClassId,
    file: &str,
) -> IngestResult<crate::dialect::MethodDef> {
    use crate::dialect::{MethodDef, MethodReceiver};

    let name = Symbol::from(constant_id_str(&def.name()));
    let receiver = if def.receiver().is_some() {
        MethodReceiver::Class
    } else {
        MethodReceiver::Instance
    };

    // Collect parameters across all kinds Ruby supports. Mirrors
    // runtime_src::method_params; the flat list loses the kind
    // distinction (re-derived from the def node when needed by emit).
    // Bodies under app/models/ legitimately use optionals (`attrs = {}`)
    // and keywords (`columns:`); the model ingest doesn't need them yet
    // but library classes do.
    let mut params: Vec<Param> = Vec::new();
    if let Some(pn) = def.parameters() {
        for req in pn.requireds().iter() {
            if let Some(rp) = req.as_required_parameter_node() {
                params.push(Param::positional(Symbol::from(constant_id_str(&rp.name()))));
            }
        }
        for opt in pn.optionals().iter() {
            if let Some(op) = opt.as_optional_parameter_node() {
                let name = Symbol::from(constant_id_str(&op.name()));
                // Capture the default Expr so per-target emit can
                // produce `name: T = <default>` signatures. Without
                // it, `def label(field, opts = {})` lowers to
                // `label(field, opts?: Record<...>)` and callers
                // omitting `opts` see `undefined`, breaking
                // downstream `Object.entries(opts)` /
                // `opts.merge(...)` chains in framework code.
                let default = ingest_expr(&op.value(), file)?;
                params.push(Param::with_default(name, default));
            }
        }
        if let Some(rest) = pn.rest() {
            if let Some(rp) = rest.as_rest_parameter_node() {
                if let Some(loc) = rp.name() {
                    if let Ok(s) = std::str::from_utf8(loc.as_slice()) {
                        params.push(Param::positional(Symbol::from(s)));
                    }
                }
            }
        }
        for post in pn.posts().iter() {
            if let Some(pp) = post.as_required_parameter_node() {
                params.push(Param::positional(Symbol::from(constant_id_str(&pp.name()))));
            }
        }
        for kw in pn.keywords().iter() {
            if let Some(rkp) = kw.as_required_keyword_parameter_node() {
                if let Ok(s) = std::str::from_utf8(rkp.name().as_slice()) {
                    params.push(Param::positional(Symbol::from(s)));
                }
            } else if let Some(okp) = kw.as_optional_keyword_parameter_node() {
                if let Ok(s) = std::str::from_utf8(okp.name().as_slice()) {
                    // Capture the default Expr so emit can produce
                    // `status: T = :found` rather than `status?: T`
                    // (which binds undefined when the caller omits
                    // the kwarg). action_controller/base.rb's
                    // `redirect_to(path, notice: nil, alert: nil,
                    // status: :found)` is the load-bearing case —
                    // without the default, every redirect loses
                    // its 302 status and the test client sees 200.
                    let default = ingest_expr(&okp.value(), file)?;
                    params.push(Param::with_default(Symbol::from(s), default));
                }
            }
        }
        if let Some(krest) = pn.keyword_rest() {
            if let Some(krp) = krest.as_keyword_rest_parameter_node() {
                if let Some(loc) = krp.name() {
                    if let Ok(s) = std::str::from_utf8(loc.as_slice()) {
                        params.push(Param::positional(Symbol::from(s)));
                    }
                }
            }
        }
        if let Some(block) = pn.block() {
            if let Some(loc) = block.name() {
                if let Ok(s) = std::str::from_utf8(loc.as_slice()) {
                    params.push(Param::positional(Symbol::from(s)));
                }
            }
        }
    }

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
        effects: crate::effect::EffectSet::default(),
        enclosing_class: Some(owner.0.clone()),
        // Source-defined `def` lands as Method by default; ingest
        // for `attr_*` calls sets AttributeReader/Writer above. A
        // future refinement could pattern-match on body shape
        // (zero-arg `@ivar` body → AttributeReader) for source code
        // that didn't use the attr_* sugar.
        kind: crate::dialect::AccessorKind::Method,
    })
}

/// Quick classifier: does the file's first class extend
/// `ApplicationRecord` or `ActiveRecord::Base`? If yes the file is a
/// model; otherwise it's a library class. Files with no class at all
/// return `None`.
pub fn classify_class_file(source: &[u8]) -> Option<ClassKind> {
    let result = parse(source);
    let root = result.node();
    let class = find_first_class(&root)?;
    let parent_path = class
        .superclass()
        .and_then(|n| constant_path_of(&n))
        .map(|p| p.join("::"));

    Some(match parent_path.as_deref() {
        Some("ApplicationRecord") | Some("ActiveRecord::Base") => ClassKind::Model,
        _ => ClassKind::LibraryClass,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClassKind {
    Model,
    LibraryClass,
}
