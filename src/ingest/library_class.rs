//! Library-class ingestion for files under `app/models/` whose class
//! does not extend `ApplicationRecord` / `ActiveRecord::Base` — for
//! example `ArticleCommentsProxy` produced by has_many specialization.
//! The model ingest's table-name/columns/associations/validations
//! machinery doesn't apply; we just collect methods and `include`
//! directives.

use ruby_prism::parse;

use crate::dialect::{LibraryClass, MethodDef, MethodReceiver, Param};
use crate::effect::EffectSet;
use crate::expr::{Expr, ExprNode, LValue, Literal};
use crate::ident::VarId;
use crate::span::Span;
use crate::{ClassId, Symbol};

use super::expr::ingest_expr;
use super::util::{
    class_name_path, constant_id_str, constant_path_of, find_all_classes_with_scope,
    find_all_modules_with_scope, find_first_class, flatten_statements, module_name_path,
    symbol_value,
};
use super::{IngestError, IngestResult};

pub fn ingest_library_class(
    source: &[u8],
    file: &str,
) -> IngestResult<Option<LibraryClass>> {
    super::sources::register(file, &String::from_utf8_lossy(source));
    let result = super::prism::parse(source, file);
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
    super::sources::register(file, &String::from_utf8_lossy(source));
    let result = super::prism::parse(source, file);
    let root = result.node();
    let mut out = Vec::new();
    for (scope, class) in find_all_classes_with_scope(&root) {
        let (lc, struct_base) = library_class_and_struct_base(&class, &scope, file)?;
        // BEFORE the class it serves: a superclass has to be defined
        // when the `class X < Y` line runs, and these two share a file.
        if let Some(base) = struct_base {
            out.push(base);
        }
        out.push(lc);
    }
    for (scope, module) in find_all_modules_with_scope(&root) {
        // A nested `ClassMethods` is not a namespace of its own — it's
        // ActiveSupport::Concern's class-side carrier, already folded
        // into its parent as Class-receiver methods by `walk_decl_body`.
        // Emitting it separately would define every method twice.
        if !scope.is_empty()
            && module_name_path(&module).as_deref() == Some(&["ClassMethods".to_string()])
        {
            continue;
        }
        out.push(library_class_from_module_node_with_scope(
            &module, &scope, file,
        )?);
    }
    // Constants written at FILE level, outside any class — lobsters'
    // `search_parser.rb` opens with `MYISAM_STOPWORDS = %w[…]` and the
    // parser's `rule(:stopword)` reads it. Ruby puts these on Object,
    // so every class in the file sees them; we hoist them into the
    // first class instead, ahead of its own constants.
    //
    // That is an approximation in exactly one direction: a file-level
    // constant read UNQUALIFIED from a different file would resolve
    // under Ruby and won't here. Reading it from inside the owning
    // class — including from inside a block in its body, which is what
    // a class-body DSL is — resolves either way, because a block
    // carries the lexical scope it was written in. The corpus has no
    // cross-file reader; a new one surfaces as a NameError naming the
    // constant, not as a silent wrong answer.
    if let Some(first) = out.first_mut() {
        let mut file_constants = file_level_constants(&root, file)?;
        if !file_constants.is_empty() {
            file_constants.extend(std::mem::take(&mut first.constants));
            first.constants = file_constants;
        }
    }
    Ok(out)
}

/// `NAME = <expr>` statements at the top level of a file, in source
/// order. Only direct program-body statements — anything inside a
/// class or module body is that declaration's own constant and is
/// collected by `walk_decl_body`.
fn file_level_constants(
    root: &ruby_prism::Node<'_>,
    file: &str,
) -> IngestResult<Vec<(Symbol, Expr)>> {
    let mut out = Vec::new();
    let Some(prog) = root.as_program_node() else {
        return Ok(out);
    };
    for stmt in prog.statements().body().iter() {
        let Some(cw) = stmt.as_constant_write_node() else { continue };
        if is_sorbet_type_alias(&cw.value()) {
            continue;
        }
        out.push((
            Symbol::from(constant_id_str(&cw.name())),
            ingest_expr(&cw.value(), file)?,
        ));
    }
    Ok(out)
}

pub(super) fn library_class_from_node(
    class: &ruby_prism::ClassNode<'_>,
    file: &str,
) -> IngestResult<LibraryClass> {
    library_class_from_node_with_scope(class, &[], file)
}

/// `class << Rails.application ... end` — the site-wide-settings idiom
/// in config/application.rb: config methods (`read_only?`, `name`,
/// `domain`) defined on the application *instance's* singleton at the
/// top level of the file, outside the Application class body. Returns
/// the def'd methods with Instance receivers — callers reach them as
/// `Rails.application.<m>`, so once the class is emitted as a
/// `Rails::Application` reopen they're plain instance methods (the
/// application object is a singleton, making instance-vs-singleton
/// definition indistinguishable to callers). Empty when the file has
/// no such block.
pub fn ingest_rails_application_singleton_methods(
    source: &[u8],
    file: &str,
) -> IngestResult<Vec<MethodDef>> {
    let result = super::prism::parse(source, file);
    let root = result.node();
    let owner = ClassId(Symbol::from("Rails::Application"));
    let mut out: Vec<MethodDef> = Vec::new();
    let Some(prog) = root.as_program_node() else {
        return Ok(out);
    };
    for stmt in prog.statements().body().iter() {
        let Some(sc) = stmt.as_singleton_class_node() else { continue };
        let Some(call) = sc.expression().as_call_node() else { continue };
        if constant_id_str(&call.name()) != "application" || call.arguments().is_some() {
            continue;
        }
        let Some(recv) = call.receiver() else { continue };
        let Some(path) = constant_path_of(&recv) else { continue };
        if path.join("::") != "Rails" {
            continue;
        }
        let (_includes, methods, _constants, _unknown) =
            walk_decl_body(sc.body(), &owner, file, false)?;
        out.extend(methods);
    }
    Ok(out)
}

/// Build a LibraryClass for a class declaration, prepending the
/// enclosing module path (`scope`) to the class's own constant-path
/// name. `module ActiveRecord; class Base` becomes `ClassId
/// ("ActiveRecord::Base")`. Top-level classes (empty scope) keep
/// their bare name. The fully-qualified ClassId aligns with the
/// shape RBS scope tracking now produces — body-typer registry
/// keys + RBS-derived `Ty::Class { id }` use the same path string.
pub(super) fn library_class_from_node_with_scope(
    class: &ruby_prism::ClassNode<'_>,
    scope: &[String],
    file: &str,
) -> IngestResult<LibraryClass> {
    library_class_and_struct_base(class, scope, file).map(|(lc, _)| lc)
}

/// The class, plus the synthesized base a `Struct.new(...)` superclass
/// expression turned into (`None` for every ordinary class).
pub(super) fn library_class_and_struct_base(
    class: &ruby_prism::ClassNode<'_>,
    scope: &[String],
    file: &str,
) -> IngestResult<(LibraryClass, Option<LibraryClass>)> {
    let name_path = class_name_path(class).ok_or_else(|| IngestError::Unsupported {
        file: file.into(),
        message: "library class name must be a simple constant or path".into(),
    })?;
    let mut full_path: Vec<String> = scope.to_vec();
    full_path.extend(name_path);
    let owner = ClassId(Symbol::from(full_path.join("::")));

    let parent = class.superclass().and_then(|n| {
        constant_path_of(&n).map(|p| ClassId(Symbol::from(p.join("::"))))
    });
    // A superclass EXPRESSION — `class Image < Struct.new(:asset_path,
    // :width, :height)`. `constant_path_of` has no answer for a call
    // node, so the parent used to come back None and the class emitted
    // as a bare `class Image`: its `super(...)` reached
    // `BasicObject#initialize` and the app died at LOAD time with
    // "wrong number of arguments". See `struct_superclass_members`.
    let struct_members = if parent.is_none() {
        class.superclass().and_then(|n| struct_superclass_members(&n))
    } else {
        None
    };
    let parent = match &struct_members {
        Some(_) => Some(struct_base_id(&owner)),
        None => parent,
    };

    let (mut includes, mut methods, mut constants, mut unknown_calls) =
        walk_decl_body(class.body(), &owner, file, false)?;

    // A `T::Struct` is a class GENERATOR, not an annotation: `const
    // :name, String` IS the constructor and the reader. Lower it into
    // the plain Ruby it stands for, so the emitted tree needs no
    // sorbet-runtime to build one.
    let parent = if parent.as_ref().is_some_and(is_sorbet_struct_parent) {
        let members = sorbet_struct_members(class.body(), file);
        let comparable = includes
            .iter()
            .any(|i| i.0.as_str() == "T::Struct::ActsAsComparable");
        // The declarations become methods, so they must not also be
        // emitted as calls; the sorbet mixins have nothing left to
        // provide.
        unknown_calls.retain(|call| !is_struct_declaration(call));
        includes.retain(|i| !i.0.as_str().starts_with("T::"));
        let mut synthesized = synth_sorbet_struct_methods(&owner, &members, comparable);
        synthesized.append(&mut methods);
        methods = synthesized;
        None
    } else if parent.as_ref().is_some_and(is_sorbet_enum_parent) {
        // The body walk already read the members out of `enums do` as
        // constants of this class, receiver spelled out, so they are
        // read from there rather than from the block a second time.
        let members: Vec<SorbetEnumMember> = constants
            .iter()
            .filter(|(_, value)| is_enum_member(&owner, value))
            .map(|(name, _)| SorbetEnumMember { name: name.clone() })
            .collect();
        // Each member is told the constant it is bound to. sorbet reads
        // that off the constant table when the `enums` block finishes;
        // here it is known at ingest, and `inspect` needs it.
        for (name, value) in constants.iter_mut() {
            if !is_enum_member(&owner, value) {
                continue;
            }
            if let ExprNode::Send { args, .. } = &mut *value.node {
                // `Desktop = new` — no serialized value given. sorbet
                // derives it from the constant name, and derives it by
                // DOWNCASING and nothing else: `PartiallyCompleted` is
                // "partiallycompleted", not "partially_completed"
                // (`const_to_serialized_val`, whose own comment says
                // the lowercase form is historical). Without this the
                // constant name landed in the value's slot and the
                // member was built with one argument where the
                // constructor takes two — an ArgumentError at LOAD
                // time, which takes the whole tree with it.
                if args.is_empty() {
                    args.push(str_lit(&name.as_str().to_lowercase()));
                }
                args.push(str_lit(name.as_str()));
            }
        }
        unknown_calls.retain(|call| !is_enums_declaration(call));
        let mut synthesized = synth_sorbet_enum_methods(&owner, &members);
        synthesized.append(&mut methods);
        methods = synthesized;
        None
    } else {
        parent
    };
    let base = struct_members
        .as_ref()
        .map(|members| struct_base_class(&owner, members));
    Ok((
        LibraryClass {
            name: owner,
            is_module: false,
            parent,
            includes,
            methods,
            nullable_columns: Vec::new(),
            origin: None,
            constants,
            unknown_calls,
        },
        base,
    ))
}

/// `NAME = T.type_alias { … }` — a constant holding a TYPE. Unlike
/// `T::Struct` and `T::Enum` it generates nothing and answers nothing:
/// the only thing that ever reads it is a `sig`, and those are read
/// before they are dropped. So the constant goes with them rather than
/// being carried into an emitted tree that has no sorbet-runtime to
/// build the type object with.
pub(super) fn is_sorbet_type_alias(value: &ruby_prism::Node<'_>) -> bool {
    let Some(call) = value.as_call_node() else { return false };
    match constant_id_str(&call.name()) {
        "type_alias" => call
            .receiver()
            .and_then(|r| r.as_constant_read_node())
            .is_some_and(|c| constant_id_str(&c.name()) == "T"),
        // `EventType = type_member { { upper: Event } }` — a type
        // PARAMETER, declared by a receiverless call `T::Generic`
        // provides. The annotations pass drops that `extend`, which is
        // right, and left this behind, which was not: the emitted
        // class called a method nothing defines and the tree stopped
        // loading there. A type parameter has no more runtime than the
        // alias above it, and the only thing that reads either is a
        // `sig`.
        "type_member" | "type_template" => call.receiver().is_none(),
        _ => false,
    }
}

/// `T::Enum` in superclass position. Like `T::Struct` it is a class
/// GENERATOR, not an annotation: `enums do Fill = new("fill") end`
/// declares members other code names and a serialization surface other
/// code calls, so the class is lowered into the plain Ruby it stands
/// for rather than dropped or carried.
fn is_sorbet_enum_parent(parent: &ClassId) -> bool {
    parent.0.as_str() == "T::Enum"
}

/// The `enums do … end` call in the collected class-body calls. The
/// members were read out of it as constants by the body walk, so
/// replaying the call would re-declare them against a `T::Enum` that
/// is not there.
fn is_enums_declaration(call: &Expr) -> bool {
    matches!(
        &*call.node,
        ExprNode::Send { recv: None, method, .. } if method.as_str() == "enums"
    )
}

/// One member of a lowered enum: the constant it is bound to, which is
/// also the name `inspect` prints.
struct SorbetEnumMember {
    name: Symbol,
}

/// A class-body constant initialized by this class's own `new` — which
/// is what the body walk makes of a member inside `enums do`, receiver
/// spelled out. A `T::Enum` may hold other constants (an alias for a
/// member, a lookup table); those are not members and are left alone.
fn is_enum_member(owner: &ClassId, value: &Expr) -> bool {
    let ExprNode::Send { recv: Some(recv), method, .. } = &*value.node else { return false };
    if method.as_str() != "new" {
        return false;
    }
    let ExprNode::Const { path } = &*recv.node else { return false };
    path.iter().map(Symbol::as_str).collect::<Vec<_>>().join("::") == owner.0.as_str()
}

fn str_lit(value: &str) -> Expr {
    Expr::new(
        Span::synthetic(),
        ExprNode::Lit { value: crate::expr::Literal::Str { value: value.to_string() } },
    )
}

fn call(recv: Option<Expr>, method: &str, args: Vec<Expr>) -> Expr {
    Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv,
            method: Symbol::from(method),
            args,
            block: None,
            parenthesized: true,
        },
    )
}

fn local(name: &str) -> Expr {
    Expr::new(Span::synthetic(), ExprNode::Var { id: VarId(0), name: Symbol::from(name) })
}

/// The plain Ruby a `T::Enum` stands for. The surface is sorbet's own,
/// read off `T::Enum` rather than guessed: `serialize` answers the
/// value a member was built from, `values` lists the members,
/// `try_deserialize` maps a value back to the member that carries it
/// (nil when none does) and `from_serialized` / `deserialize` raise
/// `KeyError` there instead, `has_serialized?` asks without raising,
/// and `to_s` delegates to `inspect`.
///
/// `==` is deliberately absent: sorbet's is identity (`super`) outside
/// migration mode, and members are single instances bound to
/// constants, so the plain class already answers it the same way.
fn synth_sorbet_enum_methods(owner: &ClassId, members: &[SorbetEnumMember]) -> Vec<MethodDef> {
    let method = |name: &str, params: Vec<Param>, receiver: MethodReceiver, body: Expr| MethodDef {
        name_span: Span::synthetic(),
        name: Symbol::from(name),
        receiver,
        params,
        body,
        signature: None,
        effects: EffectSet::default(),
        enclosing_class: Some(owner.0.clone()),
        kind: crate::dialect::AccessorKind::Method,
        is_async: false,
        mutates_self: name == "initialize",
        block_param: None,
    };
    let member_const = |name: &Symbol| {
        Expr::new(
            Span::synthetic(),
            ExprNode::Const {
                path: owner
                    .0
                    .as_str()
                    .split("::")
                    .map(Symbol::from)
                    .chain(std::iter::once(name.clone()))
                    .collect(),
            },
        )
    };
    let ivar = |name: &str| Expr::new(Span::synthetic(), ExprNode::Ivar { name: Symbol::from(name) });
    let assign = |name: &str, value: Expr| {
        Expr::new(
            Span::synthetic(),
            ExprNode::Assign { target: LValue::Ivar { name: Symbol::from(name) }, value },
        )
    };
    // `find { |member| member.serialize == value }` over the members.
    let lookup = || {
        let matches = call(
            Some(call(Some(local("member")), "serialize", vec![])),
            "==",
            vec![local("value")],
        );
        Expr::new(
            Span::synthetic(),
            ExprNode::Send {
                recv: Some(call(Some(self_class()), "values", vec![])),
                method: Symbol::from("find"),
                args: vec![],
                block: Some(block_of("member", matches)),
                parenthesized: false,
            },
        )
    };
    let value_param = || vec![Param::positional(Symbol::from("value"))];

    let mut methods = vec![
        method(
            "initialize",
            vec![
                Param::positional(Symbol::from("serialized")),
                Param::positional(Symbol::from("const_name")),
            ],
            MethodReceiver::Instance,
            Expr::new(
                Span::synthetic(),
                ExprNode::Seq {
                    exprs: vec![
                        assign("serialized_val", local("serialized")),
                        assign("const_name", local("const_name")),
                    ],
                },
            ),
        ),
        method("serialize", vec![], MethodReceiver::Instance, ivar("serialized_val")),
        // sorbet's `inspect` is `#<Enum::Member>`, and its `to_s`
        // delegates to it rather than to the serialized value — a
        // difference that would otherwise show up wherever an enum is
        // interpolated into a string, with nothing at the call site to
        // say it changed.
        method(
            "inspect",
            vec![],
            MethodReceiver::Instance,
            call(
                Some(call(
                    Some(str_lit(&format!("#<{}::", owner.0.as_str()))),
                    "+",
                    vec![ivar("const_name")],
                )),
                "+",
                vec![str_lit(">")],
            ),
        ),
        method(
            "to_s",
            vec![],
            MethodReceiver::Instance,
            call(Some(Expr::new(Span::synthetic(), ExprNode::SelfRef)), "inspect", vec![]),
        ),
        method(
            "values",
            vec![],
            MethodReceiver::Class,
            Expr::new(
                Span::synthetic(),
                ExprNode::Array {
                    elements: members.iter().map(|m| member_const(&m.name)).collect(),
                    style: crate::expr::ArrayStyle::default(),
                },
            ),
        ),
        method("try_deserialize", value_param(), MethodReceiver::Class, lookup()),
        method(
            "has_serialized?",
            value_param(),
            MethodReceiver::Class,
            Expr::new(
                Span::synthetic(),
                ExprNode::Send {
                    recv: Some(call(Some(self_class()), "values", vec![])),
                    method: Symbol::from("any?"),
                    args: vec![],
                    block: Some(block_of(
                        "member",
                        call(
                            Some(call(Some(local("member")), "serialize", vec![])),
                            "==",
                            vec![local("value")],
                        ),
                    )),
                    parenthesized: false,
                },
            ),
        ),
    ];
    // `from_serialized` raises where `try_deserialize` answers nil, and
    // `deserialize` is sorbet's alias for it.
    let found = Expr::new(
        Span::synthetic(),
        ExprNode::BoolOp {
            op: crate::expr::BoolOpKind::Or,
            surface: crate::expr::BoolOpSurface::default(),
            left: call(Some(self_class()), "try_deserialize", vec![local("value")]),
            right: call(
                None,
                "raise",
                vec![call(
                    Some(Expr::new(
                        Span::synthetic(),
                        ExprNode::Const { path: vec![Symbol::from("KeyError")] },
                    )),
                    "new",
                    // The offending value is in sorbet's message, and a
                    // KeyError without it is the one thing you need at
                    // the point it is raised.
                    vec![call(
                        Some(str_lit(&format!("Enum {} key not found: ", owner.0.as_str()))),
                        "+",
                        vec![call(Some(local("value")), "inspect", vec![])],
                    )],
                )],
            ),
        },
    );
    methods.push(method(
        "from_serialized",
        value_param(),
        MethodReceiver::Class,
        found,
    ));
    methods.push(method(
        "deserialize",
        value_param(),
        MethodReceiver::Class,
        call(Some(self_class()), "from_serialized", vec![local("value")]),
    ));
    methods
}

/// `{ |name| body }` attached to a call.
fn block_of(param: &str, body: Expr) -> Expr {
    Expr::new(
        Span::synthetic(),
        ExprNode::Lambda {
            params: vec![Symbol::from(param)],
            rest_param: None,
            block_param: None,
            body,
            block_style: crate::expr::BlockStyle::Brace,
        },
    )
}

fn self_class() -> Expr {
    Expr::new(Span::synthetic(), ExprNode::SelfRef)
}

/// A `const` / `prop` call in the collected class-body calls, which
/// the struct lowering replaces with methods.
fn is_struct_declaration(call: &Expr) -> bool {
    matches!(
        &*call.node,
        ExprNode::Send { recv: None, method, .. }
            if matches!(method.as_str(), "const" | "prop")
    )
}

/// One `const` / `prop` in a `T::Struct` body.
struct SorbetStructMember {
    name: Symbol,
    /// `prop` is writable, `const` is not.
    writable: bool,
    /// The value an omitted keyword takes: `default:` verbatim, and a
    /// `factory: -> { … }`'s body, which Ruby evaluates per call just
    /// as sorbet evaluates the factory per instance.
    default: Option<Expr>,
}

/// `T::Struct` and its variants in superclass position. `T::Struct` is
/// a class GENERATOR, not an annotation: `const :name, String` is the
/// constructor and the reader, so the class is lowered into the plain
/// Ruby it stands for rather than dropped or carried.
fn is_sorbet_struct_parent(parent: &ClassId) -> bool {
    matches!(
        parent.0.as_str(),
        "T::Struct" | "T::ImmutableStruct" | "T::InexactStruct"
    )
}

/// The `const` / `prop` declarations in a class body, in source order —
/// which is the order the keyword constructor takes them in.
fn sorbet_struct_members(body: Option<ruby_prism::Node<'_>>, file: &str) -> Vec<SorbetStructMember> {
    let mut members = Vec::new();
    let Some(body) = body else { return members };
    for stmt in flatten_statements(body) {
        let Some(call) = stmt.as_call_node() else { continue };
        if call.receiver().is_some() {
            continue;
        }
        let method = call.name();
        let writable = match constant_id_str(&method) {
            "const" => false,
            "prop" => true,
            _ => continue,
        };
        let Some(arguments) = call.arguments() else { continue };
        let mut args = arguments.arguments().iter();
        let Some(name) = args.next().and_then(|n| {
            let symbol = n.as_symbol_node()?;
            Some(Symbol::from(
                String::from_utf8_lossy(symbol.value_loc()?.as_slice()).as_ref(),
            ))
        }) else {
            continue;
        };
        // The type is the second argument and belongs to the analyzer,
        // not to the emit: `ingest::sorbet_sig` has already read it.
        let _ty = args.next();
        let mut default = None;
        for arg in args {
            let Some(hash) = arg.as_keyword_hash_node() else { continue };
            for element in hash.elements().iter() {
                let Some(assoc) = element.as_assoc_node() else { continue };
                let Some(key) = symbol_value(&assoc.key()) else { continue };
                match key.as_str() {
                    "default" => default = ingest_expr(&assoc.value(), file).ok(),
                    "factory" => {
                        // `factory: -> { expr }` — the body is the
                        // default, evaluated per call.
                        default = assoc
                            .value()
                            .as_lambda_node()
                            .and_then(|l| l.body())
                            .and_then(|b| b.as_statements_node())
                            .and_then(|b| b.body().iter().next())
                            .and_then(|e| ingest_expr(&e, file).ok());
                    }
                    _ => {}
                }
            }
        }
        members.push(SorbetStructMember { name, writable, default });
    }
    members
}

/// The plain Ruby a `T::Struct` stands for: a reader per member, a
/// writer per `prop`, and the keyword constructor sorbet generates.
///
/// Keyword rather than positional, because that is the constructor
/// sorbet builds and therefore the one every call site in the app
/// already passes.
fn synth_sorbet_struct_methods(
    owner: &ClassId,
    members: &[SorbetStructMember],
    comparable: bool,
) -> Vec<MethodDef> {
    let mut methods = Vec::new();
    for member in members {
        methods.push(synth_attr_reader(owner, &member.name, MethodReceiver::Instance));
        if member.writable {
            methods.push(synth_attr_writer(owner, &member.name, MethodReceiver::Instance));
        }
    }
    let params: Vec<Param> = members
        .iter()
        .map(|m| Param::keyword(m.name.clone(), m.default.clone()))
        .collect();
    let assigns: Vec<Expr> = members
        .iter()
        .map(|m| {
            Expr::new(
                Span::synthetic(),
                ExprNode::Assign {
                    target: LValue::Ivar { name: m.name.clone() },
                    value: Expr::new(
                        Span::synthetic(),
                        ExprNode::Var { id: VarId(0), name: m.name.clone() },
                    ),
                },
            )
        })
        .collect();
    methods.push(MethodDef {
        name_span: Span::synthetic(),
        name: Symbol::from("initialize"),
        receiver: MethodReceiver::Instance,
        params,
        body: Expr::new(Span::synthetic(), ExprNode::Seq { exprs: assigns }),
        signature: None,
        effects: EffectSet::default(),
        enclosing_class: Some(owner.0.clone()),
        kind: crate::dialect::AccessorKind::Method,
        is_async: false,
        mutates_self: true,
        block_param: None,
    });
    if comparable {
        methods.push(synth_struct_equality(owner, members));
    }
    methods
}

/// `include T::Struct::ActsAsComparable` gives a struct value equality.
/// Dropping the include without this would leave object identity in its
/// place — the same expression answering differently, silently.
fn synth_struct_equality(owner: &ClassId, members: &[SorbetStructMember]) -> MethodDef {
    let other = || Expr::new(Span::synthetic(), ExprNode::Var { id: VarId(0), name: Symbol::from("other") });
    let self_class = Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(Expr::new(Span::synthetic(), ExprNode::SelfRef)),
            method: Symbol::from("class"),
            args: vec![],
            block: None,
            parenthesized: false,
        },
    );
    let mut condition = Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(other()),
            method: Symbol::from("is_a?"),
            args: vec![self_class],
            block: None,
            parenthesized: true,
        },
    );
    for member in members {
        let mine = Expr::new(Span::synthetic(), ExprNode::Ivar { name: member.name.clone() });
        let theirs = Expr::new(
            Span::synthetic(),
            ExprNode::Send {
                recv: Some(other()),
                method: member.name.clone(),
                args: vec![],
                block: None,
                parenthesized: false,
            },
        );
        let same = Expr::new(
            Span::synthetic(),
            ExprNode::Send {
                recv: Some(mine),
                method: Symbol::from("=="),
                args: vec![theirs],
                block: None,
                parenthesized: false,
            },
        );
        condition = Expr::new(
            Span::synthetic(),
            ExprNode::BoolOp {
                op: crate::expr::BoolOpKind::And,
                surface: crate::expr::BoolOpSurface::default(),
                left: condition,
                right: same,
            },
        );
    }
    MethodDef {
        name_span: Span::synthetic(),
        name: Symbol::from("=="),
        receiver: MethodReceiver::Instance,
        params: vec![Param::positional(Symbol::from("other"))],
        body: condition,
        signature: None,
        effects: EffectSet::default(),
        enclosing_class: Some(owner.0.clone()),
        kind: crate::dialect::AccessorKind::Method,
        is_async: false,
        mutates_self: false,
        block_param: None,
    }
}

/// `Struct.new(:a, :b, :c)` in SUPERCLASS position → its member names.
/// `None` for anything else, including the keyword-init form
/// (`Struct.new(:a, keyword_init: true)`) and a `Struct.new(...) do …
/// end` carrying a body: both mean something this synthesis does not
/// supply, and answering as though they were the positional form would
/// build a class with the wrong constructor.
fn struct_superclass_members(node: &ruby_prism::Node<'_>) -> Option<Vec<Symbol>> {
    let call = node.as_call_node()?;
    if call.name().as_slice() != b"new" || call.block().is_some() {
        return None;
    }
    let recv = call.receiver()?;
    if constant_path_of(&recv)? != vec!["Struct".to_string()] {
        return None;
    }
    let args = call.arguments()?;
    let mut members = Vec::new();
    for arg in args.arguments().iter() {
        // Symbol literals only — a `keyword_init:` keyword arrives as a
        // KeywordHashNode and lands here as a non-symbol, which is what
        // makes this a rejection rather than a silent drop.
        members.push(Symbol::from(symbol_value(&arg)?));
    }
    if members.is_empty() {
        return None;
    }
    Some(members)
}

/// The name given to the anonymous struct that stood in superclass
/// position: a SIBLING of the class it serves, not a nested constant.
/// `Sound::Image` gets `Sound::ImageStruct` — nesting it would put a
/// constant called `Struct` inside the class and shadow ::Struct for
/// every body in it.
fn struct_base_id(owner: &ClassId) -> ClassId {
    ClassId(Symbol::from(format!("{}Struct", owner.0.as_str())))
}

/// The class an anonymous `Struct.new(:a, :b)` becomes: a reader and a
/// writer per member, and a positional constructor that assigns them in
/// declaration order — which is what makes the subclass's `super(a, b)`
/// resolve.
///
/// WHAT IT IS NOT. `Struct` also gives `to_a`, `==`, `each`, `members`,
/// `[]` and `deconstruct`. None is reached in the corpus, and each is a
/// separate decision (`==` in particular is VALUE equality, which is
/// the whole reason a Ruby author reaches for Struct at all). They are
/// left out rather than approximated, so a call to one is a
/// NoMethodError naming the method instead of a wrong answer.
///
/// Every parameter defaults to nil, matching Struct: `Point.new(1)`
/// leaves `y` nil rather than raising.
fn struct_base_class(owner: &ClassId, members: &[Symbol]) -> LibraryClass {
    let base = struct_base_id(owner);
    let mut methods = Vec::new();
    for m in members {
        methods.push(synth_attr_reader(&base, m, MethodReceiver::Instance));
        methods.push(synth_attr_writer(&base, m, MethodReceiver::Instance));
    }
    let params: Vec<Param> = members
        .iter()
        .map(|m| {
            let mut p = Param::positional(m.clone());
            p.default = Some(Expr::new(Span::synthetic(), ExprNode::Lit { value: Literal::Nil }));
            p
        })
        .collect();
    let assigns: Vec<Expr> = members
        .iter()
        .map(|m| {
            Expr::new(
                Span::synthetic(),
                ExprNode::Assign {
                    target: LValue::Ivar { name: m.clone() },
                    value: Expr::new(
                        Span::synthetic(),
                        ExprNode::Var { id: VarId(0), name: m.clone() },
                    ),
                },
            )
        })
        .collect();
    methods.push(MethodDef {
        name_span: crate::span::Span::synthetic(),
        name: Symbol::from("initialize"),
        receiver: MethodReceiver::Instance,
        params,
        body: Expr::new(Span::synthetic(), ExprNode::Seq { exprs: assigns }),
        signature: None,
        effects: EffectSet::default(),
        enclosing_class: Some(base.0.clone()),
        kind: crate::dialect::AccessorKind::Method,
        is_async: false,
        mutates_self: true,
        block_param: None,
    });
    LibraryClass {
        name: base,
        is_module: false,
        parent: None,
        includes: Vec::new(),
        methods,
        nullable_columns: Vec::new(),
        origin: Some(crate::dialect::LibraryClassOrigin::StructSuperclass {
            owner: owner.0.clone(),
            members: members.to_vec(),
        }),
        constants: Vec::new(),
        unknown_calls: Vec::new(),
    }
}

/// Same as `library_class_from_node` but for module-as-namespace
/// declarations — modules whose body has at least one direct `def`,
/// surfaced via `find_all_modules`. Lowered to a `LibraryClass` with
/// `is_module: true` and `parent: None`. The `is_module` flag is
/// load-bearing: callers using `include` on the result need it to be
/// emitted as `module`, not `class`, or Ruby will raise TypeError.
fn library_class_from_module_node_with_scope(
    module: &ruby_prism::ModuleNode<'_>,
    scope: &[String],
    file: &str,
) -> IngestResult<LibraryClass> {
    let name_path = module_name_path(module).ok_or_else(|| IngestError::Unsupported {
        file: file.into(),
        message: "library module name must be a simple constant or path".into(),
    })?;
    let mut full_path: Vec<String> = scope.to_vec();
    full_path.extend(name_path);
    let owner = ClassId(Symbol::from(full_path.join("::")));

    let (includes, methods, constants, unknown_calls) =
        walk_decl_body(module.body(), &owner, file, false)?;
    Ok(LibraryClass {
        name: owner,
        is_module: true,
        parent: None,
        includes,
        methods,
        nullable_columns: Vec::new(),
        origin: None,
        constants,
        unknown_calls,
    })
}

/// Walk a class or module body, collecting `include` directives and
/// method definitions (with `attr_*` lowered to synthesized methods).
/// Receiverless calls the walk doesn't recognize (`rule(:x) { … }`,
/// `alias_method`, …) are captured into the fourth slot rather than
/// dropped — see `LibraryClass::unknown_calls`. Nested class/module
/// declarations are still dropped; those surface separately via the
/// plural ingest entry points.
///
/// `force_class_receiver` is true when we're recursing into a
/// `class << self` block; it overrides every synthesized method's
/// receiver to `Class`, so e.g. `attr_accessor :adapter` inside
/// `class << self` produces class-level getter/setter pairs.
type DeclBody = (Vec<ClassId>, Vec<MethodDef>, Vec<(Symbol, Expr)>, Vec<Expr>);

/// Receiverless class-body calls that are NOT safe to capture into
/// `unknown_calls`, because their meaning depends on where they sit
/// relative to the method definitions around them — and a
/// `LibraryClass` has no source-ordered body, so a captured call
/// replays ahead of every method. `private` replayed at the top of the
/// class body would make the whole class private rather than its tail.
///
/// `require` / `require_relative` are here for a different reason: the
/// emitted tree builds its own require graph (spinel's AOT stage
/// resolves it statically), so replaying a source require inside a
/// class body would point at a path that doesn't exist in the output.
/// `extend T::Sig` and its siblings — the mixins whose whole surface is
/// the annotations dropped above. `T::Sig` provides `sig`, `T::Helpers`
/// provides `abstract!`/`interface!`, `T::Generic` provides
/// `type_member`; with none of those left in the emit, the mixin is a
/// `NameError` waiting at load time and nothing more.
///
/// `include T::Struct::ActsAsComparable` is deliberately NOT here: it
/// gives a struct its `==`, which is behavior, and it goes when the
/// struct itself is lowered.
fn is_sorbet_annotation_mixin(call: &ruby_prism::CallNode<'_>) -> bool {
    let name = call.name();
    if !matches!(constant_id_str(&name), "extend" | "include") {
        return false;
    }
    let Some(arguments) = call.arguments() else { return false };
    let args: Vec<_> = arguments.arguments().iter().collect();
    let [only] = args.as_slice() else { return false };
    let Some(path) = only.as_constant_path_node() else { return false };
    let written = constant_path_written(&path);
    matches!(written.as_str(), "T::Sig" | "T::Helpers" | "T::Generic")
}

/// `T::Sig` — the constant path as written.
fn constant_path_written(path: &ruby_prism::ConstantPathNode<'_>) -> String {
    let mut out = match path.parent() {
        Some(parent) => match parent.as_constant_read_node() {
            Some(read) => format!("{}::", constant_id_str(&read.name())),
            None => match parent.as_constant_path_node() {
                Some(inner) => format!("{}::", constant_path_written(&inner)),
                None => String::new(),
            },
        },
        None => String::new(),
    };
    if let Some(name) = path.name() {
        out.push_str(constant_id_str(&name));
    }
    out
}

/// sorbet-runtime's pure ANNOTATIONS: they carry types and nothing
/// else, and roundhouse has already read them (`ingest::sorbet_sig`)
/// by the time a class body is walked.
///
/// Dropped from the emit rather than round-tripped, because keeping
/// them would be the one thing this project exists not to do: a `sig`
/// makes sorbet-runtime wrap the method and re-check its types on
/// every call, which is per-request work whose answer cannot differ
/// between requests. The emitted tree carries no sorbet-runtime, so a
/// surviving `sig` is also a `NameError` waiting at load time.
///
/// `T::Struct` and `T::Enum` are NOT here: `const :name, String` is a
/// constructor and a reader, not an annotation, and deleting it would
/// leave a class that cannot be built. Those need lowering, not
/// dropping.
const SORBET_ANNOTATIONS: &[&str] = &[
    "sig",
    "abstract!",
    "interface!",
    "final!",
    "sealed!",
    "type_parameters",
];

const POSITION_SENSITIVE_MARKERS: &[&str] = &[
    "private",
    "public",
    "protected",
    "private_class_method",
    "public_class_method",
    "private_constant",
    "public_constant",
    "require",
    "require_relative",
];

fn walk_decl_body<'pr>(
    body: Option<ruby_prism::Node<'pr>>,
    owner: &ClassId,
    file: &str,
    force_class_receiver: bool,
) -> IngestResult<DeclBody> {
    let mut includes: Vec<ClassId> = Vec::new();
    let mut methods: Vec<MethodDef> = Vec::new();
    let mut constants: Vec<(Symbol, Expr)> = Vec::new();
    let mut unknown_calls: Vec<Expr> = Vec::new();
    // `module_function` (called bare inside a module body) marks every
    // subsequent direct `def` as a module-function — both an instance
    // method AND a class method. For our targets (which call these as
    // `Mod.x(...)`), we only need the class-method form, so flip the
    // receiver to Class. Doesn't affect nested `class`/`module` bodies
    // — they get their own walk_decl_body recursion.
    let mut module_function_active = false;
    // Names from the `module_function :a, :b` form, plus the positions
    // of the direct `def`s in this body they may promote. Tracking
    // positions (rather than searching `methods` by name at the end)
    // keeps a `class << self` block's methods out of reach — those are
    // appended to the same vec but belong to a different scope.
    let mut module_function_named: Vec<String> = Vec::new();
    let mut direct_def_positions: Vec<usize> = Vec::new();

    let Some(b) = body else {
        return Ok((includes, methods, constants, unknown_calls));
    };

    for stmt in flatten_statements(b) {
        // `enums do Fill = new("fill") end` — sorbet-runtime's `T::Enum`
        // declares its members inside a block, so the constants are one
        // level deeper than every other class-body constant. They are
        // constants of this class all the same, and reading them is what
        // lets `Mode::Fill` type as a Mode rather than as a class nobody
        // defined.
        if let Some(call) = stmt.as_call_node() {
            let name = call.name();
            if constant_id_str(&name) == "enums" && call.receiver().is_none() {
                if let Some(block) = call.block().and_then(|b| b.as_block_node()) {
                    if let Some(block_body) = block.body() {
                        for member in flatten_statements(block_body) {
                            let Some(cw) = member.as_constant_write_node() else { continue };
                            let name = Symbol::from(constant_id_str(&cw.name()));
                            let mut value = ingest_expr(&cw.value(), file)?;
                            // `Fill = new("fill")` — inside the class
                            // body, the receiverless `new` IS
                            // `Mode.new`, and spelling it out is what
                            // lets the constant registry type the
                            // member as an instance of its enum rather
                            // than leaving it untyped. (T::Enum makes
                            // the constructor private at run time; this
                            // is the ingest's reading of it, not an
                            // emitted call.)
                            if let ExprNode::Send { recv: recv @ None, method, .. } =
                                &mut *value.node
                            {
                                if method.as_str() == "new" {
                                    *recv = Some(Expr::new(
                                        crate::span::Span::synthetic(),
                                        ExprNode::Const {
                                            path: owner
                                                .0
                                                .as_str()
                                                .split("::")
                                                .map(Symbol::from)
                                                .collect(),
                                        },
                                    ));
                                }
                            }
                            constants.push((name, value));
                        }
                    }
                    continue;
                }
            }
        }
        // Class-level constant `NAME = <expr>` (e.g. `STORIES_PER_PAGE = 25`).
        if let Some(cw) = stmt.as_constant_write_node() {
            if is_sorbet_type_alias(&cw.value()) {
                continue;
            }
            let name = Symbol::from(constant_id_str(&cw.name()));
            let value = ingest_expr(&cw.value(), file)?;
            constants.push((name, value));
            continue;
        }
        // `@@X = nil` class-body initializer. The corpus pairs these with
        // `cattr_accessor` (extras/keybase, github, twitter) — class-var
        // reads in class-method bodies normalize to class-level ivars
        // (see below), and an unset class-level ivar already reads nil,
        // so the nil form drops as semantically exact. A non-nil
        // initializer would be silently lost; refuse it loudly until one
        // exists.
        if let Some(cvw) = stmt.as_class_variable_write_node() {
            let value = ingest_expr(&cvw.value(), file)?;
            if !matches!(&*value.node, ExprNode::Lit { value: crate::expr::Literal::Nil }) {
                return Err(IngestError::Unsupported {
                    file: file.into(),
                    message: "class-variable initializer with non-nil value".into(),
                });
            }
            continue;
        }
        if let Some(def) = stmt.as_def_node() {
            let mut m = ingest_library_method(&def, owner, file)?;
            if force_class_receiver || module_function_active {
                m.receiver = MethodReceiver::Class;
            }
            direct_def_positions.push(methods.len());
            methods.push(m);
            continue;
        }
        // `class << self ... end` — singleton class block. Body
        // defines class-level methods on the enclosing scope.
        if let Some(sc) = stmt.as_singleton_class_node() {
            let (inner_includes, inner_methods, inner_constants, inner_unknown) =
                walk_decl_body(sc.body(), owner, file, true)?;
            includes.extend(inner_includes);
            methods.extend(inner_methods);
            constants.extend(inner_constants);
            unknown_calls.extend(inner_unknown);
            continue;
        }
        // `module ClassMethods … end` — ActiveSupport::Concern's OTHER
        // spelling for the class side, and the one campfire's
        // `User::Bot` uses for `create_bot!`/`authenticate_bot`.
        // `class_methods do` (below) is sugar that Concern turns into
        // exactly this module, so both have to arrive at the same
        // place: Class-receiver methods of the enclosing module, which
        // the registry's concern fold copies onto every includer.
        // `ingest_library_classes` skips the nested module for this
        // reason — otherwise the same defs would emit twice.
        if let Some(m) = stmt.as_module_node() {
            if module_name_path(&m).as_deref() == Some(&["ClassMethods".to_string()]) {
                let (inner_includes, inner_methods, inner_constants, inner_unknown) =
                    walk_decl_body(m.body(), owner, file, true)?;
                includes.extend(inner_includes);
                methods.extend(inner_methods);
                constants.extend(inner_constants);
                unknown_calls.extend(inner_unknown);
                continue;
            }
        }
        if let Some(call) = stmt.as_call_node() {
            if call.receiver().is_none() {
                let kw = constant_id_str(&call.name());
                // `class_methods do … end` — ActiveSupport::Concern's
                // class-side block: its defs become class methods of
                // every includer (`Account.find_local!`). Capture them
                // as Class-receiver methods of the module; the
                // registry's concern fold copies them onto includers.
                if kw == "class_methods" {
                    if let Some(block) = call.block().and_then(|blk| blk.as_block_node()) {
                        let (inner_includes, inner_methods, inner_constants, inner_unknown) =
                            walk_decl_body(block.body(), owner, file, true)?;
                        includes.extend(inner_includes);
                        methods.extend(inner_methods);
                        constants.extend(inner_constants);
                        unknown_calls.extend(inner_unknown);
                        continue;
                    }
                }
                match kw {
                    "include" => {
                        if let Some(args) = call.arguments() {
                            for arg in args.arguments().iter() {
                                if let Some(path) = constant_path_of(&arg) {
                                    // lobsters' `TimeSeries` includes
                                    // `ActionView::Helpers::NumberHelper`
                                    // and calls one member, which emit
                                    // qualifies to `ActionView::
                                    // ViewHelpers.number_with_delimiter`
                                    // anyway. `ActiveModel::*` does NOT
                                    // drop here — see the predicate's
                                    // sibling.
                                    let segs: Vec<&str> =
                                        path.iter().map(|p| p.as_str()).collect();
                                    if crate::ingest::util::is_view_helper_marker_include(&segs) {
                                        continue;
                                    }
                                    includes.push(ClassId(Symbol::from(path.join("::"))));
                                } else if is_rails_url_helpers_chain(&arg) {
                                    // `include Rails.application.routes.
                                    // url_helpers` (lobsters' Routes class,
                                    // inside `class << self`) — the whole
                                    // route-helper surface. Recorded as an
                                    // include of our generated RouteHelpers
                                    // module: the analyzer registers the
                                    // helper names off this marker and the
                                    // ruby emit rewrites `X.<helper>` call
                                    // sites through RouteHelpers.
                                    includes.push(ClassId(Symbol::from("RouteHelpers")));
                                }
                            }
                        }
                    }
                    "attr_reader" | "attr_writer" | "attr_accessor"
                    | "cattr_reader" | "cattr_writer" | "cattr_accessor"
                    | "mattr_reader" | "mattr_writer" | "mattr_accessor" => {
                        // Lower to method definitions at ingest time
                        // (per the YAGNI-on-round-trip decision):
                        //   attr_reader :foo  → def foo; @foo; end
                        //   attr_writer :foo  → def foo=(v); @foo = v; end
                        //   attr_accessor :foo → both
                        // The `cattr_*` / `mattr_*` (ActiveSupport class- and
                        // module-level attribute accessors) generate the same
                        // pair on the *singleton*, so a bare `Keybase.DOMAIN`
                        // resolves; we model the class form (Rails also makes
                        // instance-level copies, not needed by the corpus).
                        let mut names: Vec<Symbol> = Vec::new();
                        if let Some(args) = call.arguments() {
                            for arg in args.arguments().iter() {
                                if let Some(s) = symbol_value(&arg) {
                                    names.push(Symbol::from(s));
                                }
                            }
                        }
                        let is_class_attr =
                            kw.starts_with("cattr_") || kw.starts_with("mattr_");
                        let recv = if is_class_attr || force_class_receiver {
                            MethodReceiver::Class
                        } else {
                            MethodReceiver::Instance
                        };
                        for name in &names {
                            let want_reader = kw.ends_with("_reader") || kw.ends_with("_accessor");
                            let want_writer = kw.ends_with("_writer") || kw.ends_with("_accessor");
                            if want_reader {
                                methods.push(synth_attr_reader(owner, name, recv));
                            }
                            if want_writer {
                                methods.push(synth_attr_writer(owner, name, recv));
                            }
                        }
                    }
                    // `extend self` — the OTHER spelling of the same
                    // idea, and the one campfire's
                    // `RestrictedHTTP::PrivateNetworkGuard` uses. Ruby
                    // makes every instance method a singleton method
                    // too, so `PrivateNetworkGuard.resolve(host)` reaches
                    // the `def resolve` below it. Dropped, the module
                    // emitted its methods as instance-only and every
                    // dotted call was a NoMethodError — which is what
                    // left `Opengraph::Metadata.from_url` fetching
                    // nothing at all.
                    //
                    // Same treatment as bare `module_function`: our
                    // targets call these as `Mod.x(...)`, so only the
                    // class-method form is needed.
                    "extend"
                        if call
                            .arguments()
                            .map(|a| {
                                let args: Vec<_> = a.arguments().iter().collect();
                                args.len() == 1 && args[0].as_self_node().is_some()
                            })
                            .unwrap_or(false) =>
                    {
                        module_function_active = true;
                    }
                    "module_function" => {
                        // Bare `module_function` (no args) — flip the
                        // flag for every subsequent direct `def` in
                        // this body.
                        if call.arguments().is_none() {
                            module_function_active = true;
                        } else if let Some(names) =
                            crate::runtime_src::module_function_arg_names(&stmt)
                        {
                            // `module_function :foo, :bar` names its
                            // methods rather than flipping a mode. Ruby
                            // requires them to be defined already (it
                            // copies the existing definition), so the
                            // promotion is retroactive — recorded here
                            // and applied after the body walk, which
                            // also makes it order-independent.
                            module_function_named.extend(names);
                        }
                    }
                    _ => {
                        // A call this walk doesn't model. Capture it so
                        // the targets that CAN replay a class-body DSL
                        // do (`LibraryClass::unknown_calls`); the
                        // position-sensitive markers stay dropped
                        // because a capture would replay them in the
                        // wrong place.
                        //
                        // An expression we can't even ingest is left
                        // dropped rather than failing the whole class:
                        // the class still has its methods, and the emit
                        // side reports the gap. That keeps a single
                        // exotic call in one library class from taking
                        // the app's ingest down.
                        if !POSITION_SENSITIVE_MARKERS.contains(&kw)
                            && !SORBET_ANNOTATIONS.contains(&kw)
                            && !is_sorbet_annotation_mixin(&call)
                        {
                            if let Ok(e) = ingest_expr(&stmt, file) {
                                unknown_calls.push(e);
                            }
                        }
                    }
                }
            }
        }
        // Nested class/module declarations also fall through here; they
        // surface as separate entries via the plural API.
    }

    // Class-variable reads/writes in CLASS-receiver bodies normalize to
    // class-level ivars — the storage `cattr_accessor`'s synthesized
    // accessors use — so `@@DOMAIN.present?` in `def self.enabled?` and
    // `Keybase.DOMAIN=` agree (verbatim `@@X` in the emitted class method
    // is a NameError when unassigned; class-level `@X` reads nil).
    // Instance-method bodies are left alone: `@X` there would be
    // instance storage, a different variable — a verbatim `@@X` failing
    // loudly at runtime beats silently splitting the storage.
    // `module_function :a, :b` promotions, applied before the classvar
    // normalization below so a named method gets exactly what the bare
    // form's methods get (the flag there is set before the def is even
    // pushed, so it is already Class by this point).
    //
    // Ruby keeps BOTH copies — a module method plus a private instance
    // method — and lobsters uses both spellings of the same name
    // (`EmailBlocklistValidation.email_on_blocklist?` from a mailer and
    // a view; a bare `email_on_blocklist?(email)` from the sibling
    // validation method that runs on an includer instance). We emit one
    // method per name, so promoting alone would just move the breakage
    // from the module spelling to the instance one. Retarget the
    // sibling bare calls to the module spelling instead, which keeps a
    // single definition and leaves both call sites resolving.
    if !module_function_named.is_empty() {
        let mut promoted: Vec<Symbol> = Vec::new();
        for pos in &direct_def_positions {
            if module_function_named
                .iter()
                .any(|n| n == methods[*pos].name.as_str())
            {
                methods[*pos].receiver = MethodReceiver::Class;
                promoted.push(methods[*pos].name.clone());
            }
        }
        if !promoted.is_empty() {
            for m in &mut methods {
                // The promoted method's own body is included: a
                // self-recursive call needs the same retarget.
                retarget_module_function_calls(&mut m.body, owner, &promoted);
            }
        }
    }

    for m in &mut methods {
        if m.receiver == MethodReceiver::Class {
            normalize_classvars_to_ivars(&mut m.body);
        }
    }

    Ok((includes, methods, constants, unknown_calls))
}

/// Rewrite `@@X` (ingested as a sigil-verbatim `Var`) to `Ivar { X }`,
/// both in read position and as an `Assign` target.
/// Match the `Rails.application.routes.url_helpers` receiver chain (a
/// nested CallNode ladder rooted at the `Rails` constant).
fn is_rails_url_helpers_chain(node: &ruby_prism::Node<'_>) -> bool {
    let mut expected = ["url_helpers", "routes", "application"].iter();
    let mut cur = match node.as_call_node() {
        Some(c) => c,
        None => return false,
    };
    loop {
        let Some(want) = expected.next() else { return false };
        if cur.name().as_slice() != want.as_bytes() {
            return false;
        }
        match cur.receiver() {
            Some(r) => {
                if let Some(cr) = r.as_constant_read_node() {
                    return expected.next().is_none()
                        && cr.name().as_slice() == b"Rails";
                }
                match r.as_call_node() {
                    Some(next) => cur = next,
                    None => return false,
                }
            }
            None => return false,
        }
    }
}

fn normalize_classvars_to_ivars(e: &mut Expr) {
    match &mut *e.node {
        ExprNode::Var { name, .. } if name.as_str().starts_with("@@") => {
            let bare = Symbol::from(&name.as_str()[2..]);
            *e.node = ExprNode::Ivar { name: bare };
        }
        ExprNode::Assign { target: LValue::Var { name, .. }, .. }
            if name.as_str().starts_with("@@") =>
        {
            let bare = Symbol::from(&name.as_str()[2..]);
            let ExprNode::Assign { target, value } = &mut *e.node else { unreachable!() };
            *target = LValue::Ivar { name: bare };
            normalize_classvars_to_ivars(value);
        }
        _ => {
            e.node.for_each_child_mut(&mut |c| normalize_classvars_to_ivars(c));
        }
    }
}

/// Synthesize `def <name>; @<name>; end` (instance receiver) or
/// `def self.<name>; @<name>; end` (class receiver).
fn synth_attr_reader(owner: &ClassId, name: &Symbol, receiver: MethodReceiver) -> MethodDef {
    let body = Expr::new(
        Span::synthetic(),
        ExprNode::Ivar { name: name.clone() },
    );
    MethodDef {
        name_span: crate::span::Span::synthetic(),
        name: name.clone(),
        receiver,
        params: Vec::new(),
        body,
        signature: None,
        effects: EffectSet::default(),
        enclosing_class: Some(owner.0.clone()),
        kind: crate::dialect::AccessorKind::AttributeReader,
        is_async: false,
            mutates_self: false,
            block_param: None,
    }
}

/// Rewrite receiver-less calls to a `module_function`-promoted name
/// into `<Owner>.name(...)`.
///
/// Only bare sends match, so an explicit receiver (`other.foo`) is left
/// alone. A local variable shadowing the name would be a `Var` node,
/// not a `Send`, so it can't be caught here either.
fn retarget_module_function_calls(expr: &mut Expr, owner: &ClassId, promoted: &[Symbol]) {
    expr.node
        .for_each_child_mut(&mut |c| retarget_module_function_calls(c, owner, promoted));
    let ExprNode::Send { recv, method, .. } = &mut *expr.node else {
        return;
    };
    if recv.is_some() || !promoted.iter().any(|p| p == method) {
        return;
    }
    *recv = Some(Expr::new(
        expr.span,
        ExprNode::Const {
            path: vec![owner.0.clone()],
        },
    ));
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
        name_span: crate::span::Span::synthetic(),
        name: setter_name,
        receiver,
        params: vec![Param::positional(value_param)],
        body,
        signature: None,
        effects: EffectSet::default(),
        enclosing_class: Some(owner.0.clone()),
        kind: crate::dialect::AccessorKind::AttributeWriter,
        is_async: false,
            mutates_self: false,
            block_param: None,
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
                        params.push(Param::rest(Symbol::from(s)));
                    }
                }
            }
        }
        for post in pn.posts().iter() {
            if let Some(pp) = post.as_required_parameter_node() {
                params.push(Param::positional(Symbol::from(constant_id_str(&pp.name()))));
            }
        }
        // An optional keyword can only take the positional-with-default
        // approximation below when nothing else in the signature forces
        // Ruby's ordering rules. Two shapes do, and campfire has one of
        // each: a REQUIRED keyword beside it (`def initialize(name:,
        // text: nil)` → `(name:, text = nil)`, Sound::Image) and a rest
        // param before it (`def f(*messages, count: 1)` → `(*messages,
        // count = 1)`). Neither parses. Keep the keyword group honest
        // in those defs; elsewhere the approximation stands, because
        // the trailing-kwargs normalize path depends on it.
        //
        // A `**kwrest` BESIDE an optional keyword (`def f(x, style:
        // :time, **attributes)`) parses fine flattened, so it stays on
        // the approximation — but the two adjacent positionals are
        // indistinguishable to a caller forwarding a bundle, which
        // silently binds the keyword instead of the rest. That is why
        // both flattenings are MARKED below: `lower::kwrest_forward`
        // repairs the call, and the marks are the only record that
        // these slots were not positional in the source.
        let keeps_keywords = params.iter().any(|p| p.rest)
            || pn
                .keywords()
                .iter()
                .any(|kw| kw.as_required_keyword_parameter_node().is_some());
        for kw in pn.keywords().iter() {
            if let Some(rkp) = kw.as_required_keyword_parameter_node() {
                if let Ok(s) = std::str::from_utf8(rkp.name().as_slice()) {
                    // Marked keyword so passes that cannot forward
                    // kwargs positionally (mailer/job class-side
                    // wrappers) see the truth and ledger instead of
                    // synthesizing a mis-binding wrapper. (The
                    // optional-keyword branch below deliberately stays
                    // positional-with-default — the trailing-kwargs
                    // normalize path depends on that shape.)
                    params.push(Param::keyword(
                        Symbol::from(s.trim_end_matches(':')),
                        None,
                    ));
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
                    params.push(if keeps_keywords {
                        Param::keyword(Symbol::from(s), Some(default))
                    } else {
                        // Flattened to a positional-with-default, and
                        // MARKED as flattened: the emitted shape says
                        // "an optional positional you may fill", which
                        // the original Ruby did not offer. A caller that
                        // appears to fill it is an erased `**`, and
                        // `lower::kwrest_forward` needs that fact.
                        let mut p = Param::with_default(Symbol::from(s), default);
                        p.from_keyword = true;
                        p
                    });
                }
            }
        }
        if let Some(krest) = pn.keyword_rest() {
            if let Some(krp) = krest.as_keyword_rest_parameter_node() {
                if let Some(loc) = krp.name() {
                    if let Ok(s) = std::str::from_utf8(loc.as_slice()) {
                        // `**options` is OPTIONAL in Ruby — it binds to
                        // `{}` when the caller passes no keywords — and
                        // the trailing positional it becomes here has to
                        // say so, or every bare call is an ArgumentError.
                        // campfire's `avatar_tag(user, **options)` is
                        // called with one argument from the message row,
                        // the user list and the sidebar.
                        let mut p = Param::with_default(
                            Symbol::from(s),
                            Expr::new(
                                Span::synthetic(),
                                ExprNode::Hash { entries: vec![], kwargs: false },
                            ),
                        );
                        p.from_kwrest = true;
                        params.push(p);
                    }
                }
            }
        }
    }

    // `&block` rides in `MethodDef.block_param`, not the flat list —
    // it occupies the call-site `block:` slot, never `args:`. Mirrors
    // the runtime_src split (see runtime_src::method_params).
    let block_param = def.parameters().and_then(|pn| pn.block()).map(|block| {
        let name = block
            .name()
            .and_then(|loc| std::str::from_utf8(loc.as_slice()).ok())
            // Ruby 3.4 anonymous block param (`def f(&)`) — synthesize a
            // name so body-side bare-`&` forwarding (`__blk`) binds.
            .unwrap_or("__blk");
        Param::positional(Symbol::from(name))
    });

    let body = match def.body() {
        Some(b) => ingest_expr(&b, file)?,
        None => Expr::new(Span::synthetic(), ExprNode::Seq { exprs: vec![] }),
    };

    Ok(MethodDef {
        name_span: super::util::def_name_span(def, file),
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
        is_async: false,
        mutates_self: false,
        block_param,
    })
}

/// Quick classifier: does the file's first class extend
/// `ApplicationRecord` or `ActiveRecord::Base`? If yes the file is a
/// model; otherwise it's a library class. Files with no class at all
/// return `None`.
pub fn classify_class_file(source: &[u8]) -> Option<ClassKind> {
    let result = parse(source);
    let root = result.node();
    let Some(class) = find_first_class(&root) else {
        // No class node. A bare top-level module under app/models/
        // (`module InactiveUser; def self.x; …; end`) is a namespace of
        // singleton methods, not a model — classify it as a library
        // class so the module-aware (plural) ingest registers its
        // `def self.x` as dotted-call class methods. (`find_first_class`
        // already descends modules, so a model nested in a namespace
        // module — `module Admin; class User < ApplicationRecord` — is
        // still found above and classified Model.)
        if !find_all_modules_with_scope(&root).is_empty() {
            return Some(ClassKind::LibraryClass);
        }
        return None;
    };
    let parent_path = class
        .superclass()
        .and_then(|n| constant_path_of(&n))
        .map(|p| p.join("::"));

    Some(match parent_path.as_deref() {
        Some("ApplicationRecord") | Some("ActiveRecord::Base") => ClassKind::Model,
        // A superclass-less class that `include`s the ActiveModel
        // validation surface (lobsters' Search) is a tableless model:
        // the model path lowers its `validates` DSL and synthesizes
        // `valid?`/`errors`; the library-class path would drop them.
        None if includes_active_model(&class) => ClassKind::Model,
        _ => ClassKind::LibraryClass,
    })
}

fn includes_active_model(class: &ruby_prism::ClassNode<'_>) -> bool {
    let Some(body) = class.body() else { return false };
    let Some(stmts) = body.as_statements_node() else { return false };
    stmts.body().iter().any(|stmt| {
        let Some(call) = stmt.as_call_node() else { return false };
        if call.receiver().is_some() || call.name().as_slice() != b"include" {
            return false;
        }
        call.arguments().is_some_and(|args| {
            args.arguments().iter().any(|arg| {
                constant_path_of(&arg).is_some_and(|path| {
                    path.join("::") == "ActiveModel::Validations"
                        || path.join("::") == "ActiveModel::Model"
                })
            })
        })
    })
}

/// Is `parent` one of Rails' own subclassable bases that the runtime
/// does NOT port? A subclass of one is app code the analyzer can read
/// — `check` still types a mailbox's `process` — but no emitted tree
/// can load it: there is no `ActionMailbox` to name, so the file
/// raises `NameError` at require time and, through the `app/models.rb`
/// aggregator, takes the whole app down with it. That is what
/// lobsters' `ApplicationMailbox < ActionMailbox::Base` did the day
/// `app/mailboxes/` became a support root: the tree emitted, then
/// would not boot. `lower::unported_rails_subclasses` drops the class
/// after analysis, out loud.
///
/// Spelled out, and Rails' own only. The ported bases — the ones
/// `runtime/ruby/` defines and the ruby emitter's
/// `require_path_for_parent` anchors — are simply not on it. A gem's
/// base is not on it either, even one parked under a Rails namespace
/// (`ActiveModel::Serializer`, Mastodon's 160 serializers): that is
/// the gem-DSL path, where the class body replays for the gem to run
/// on the ruby lane. An unlisted Rails base is the old behavior, a
/// replay that fails at load; add it here when it turns up.
pub fn is_unported_rails_base(parent: &str) -> bool {
    matches!(
        parent,
        "ActionMailbox::Base"
            | "ActiveModel::Validator"
            | "ActiveModel::EachValidator"
            | "ActiveRecord::Migration"
            | "Rails::Generators::Base"
            | "Rails::Generators::NamedBase"
            | "Rails::Railtie"
            | "Rails::Engine"
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClassKind {
    Model,
    LibraryClass,
}

/// Filters declared inside a concern module's `included do` block:
/// `module AccountOwnedConcern … included do before_action :set_account,
/// … end end` → `(AccountOwnedConcern, [Filter(set_account), …])`.
/// Rails evaluates that block in each including class, so these filters
/// belong to every includer — analyze consumes the returned pairs (via
/// `App::concern_filters`) to extend each including controller's filter
/// chain. Modules without an `included do`, and `included do` statements
/// that aren't filter calls, contribute nothing here (the module's
/// method defs are captured separately by [`ingest_library_classes`]).
/// Per module, the names its ActiveSupport::Concern CLASS-SIDE CARRIER
/// declares — `module ClassMethods … end` or `class_methods do … end`.
///
/// `walk_decl_body` flattens both into the parent module as
/// Class-receiver methods, which is right for resolution and loses the
/// one fact an includer needs: whether a given class-side method is
/// inherited. Concern's `append_features` runs `base.extend
/// ClassMethods` and nothing else, so ONLY these cross. A module's own
/// singletons — `module_function :x`, `class << self` — are also
/// Class-receiver methods after the flatten and are NOT inherited.
///
/// Without the distinction, the model concern splice invented
/// `User.email_on_blocklist?` on three lobsters models, from
/// `EmailBlocklistValidation`'s `module_function :email_on_blocklist?`.
///
/// Read with its own parse, like `ingest_concern_filters` and
/// `ingest_concern_model_items` beside it, rather than widening
/// `DeclBody` — that tuple reaches 25 `LibraryClass` construction
/// sites, nearly all of them synthesizing classes that can never have a
/// concern carrier.
/// Every `helper_method :name, …` the file declares.
///
/// Rails' `helper_method` is the app SAYING which controller methods a
/// view may call — campfire's `SetPlatform` exposes `platform`,
/// `Authentication` exposes `signed_in?`, `TrackedRoomVisit` exposes
/// `last_room_visited`, and the room page calls all three. Our views
/// lower to module functions with no controller instance, so a bare
/// `platform` there resolved to nothing and the page died on a
/// NameError.
///
/// A whole-file VISIT rather than a per-module statement scan: the
/// declaration is spelled the same in a controller class body, inside a
/// concern's `included do`, and inside `class_methods do`, and what the
/// call-site rewrite wants is one NAME SET. Rails scopes the exposure to
/// the declaring controller and its descendants; a name is only routed
/// where a view actually calls it, and that rewrite is already shadowed
/// by the module's own methods and its params.
pub fn ingest_helper_method_names(source: &[u8]) -> Vec<Symbol> {
    struct HelperMethodVisitor {
        names: Vec<Symbol>,
    }

    impl<'pr> ruby_prism::Visit<'pr> for HelperMethodVisitor {
        fn visit_call_node(&mut self, node: &ruby_prism::CallNode<'pr>) {
            if node.receiver().is_none() && constant_id_str(&node.name()) == "helper_method" {
                if let Some(args) = node.arguments() {
                    for arg in args.arguments().iter() {
                        if let Some(sym) = arg.as_symbol_node() {
                            self.names.push(Symbol::from(
                                String::from_utf8_lossy(sym.unescaped()).as_ref(),
                            ));
                        }
                    }
                }
            }
            ruby_prism::visit_call_node(self, node);
        }
    }

    let result = parse(source);
    let mut visitor = HelperMethodVisitor { names: Vec::new() };
    ruby_prism::Visit::visit(&mut visitor, &result.node());
    visitor.names
}

pub fn ingest_concern_class_method_names(source: &[u8]) -> Vec<(ClassId, Vec<Symbol>)> {
    fn defs_in(body: Option<ruby_prism::Node<'_>>, out: &mut Vec<Symbol>) {
        let Some(body) = body else { return };
        for stmt in flatten_statements(body) {
            if let Some(def) = stmt.as_def_node() {
                out.push(Symbol::from(constant_id_str(&def.name())));
            }
        }
    }

    let result = parse(source);
    let root = result.node();
    let mut out = Vec::new();
    for (scope, module) in find_all_modules_with_scope(&root) {
        let Some(name_path) = module_name_path(&module) else { continue };
        // A nested `ClassMethods` is reported under its PARENT, which is
        // the module an app actually includes.
        if name_path.as_slice() == ["ClassMethods".to_string()] && !scope.is_empty() {
            continue;
        }
        let mut full_path: Vec<String> = scope.clone();
        full_path.extend(name_path);
        let id = ClassId(Symbol::from(full_path.join("::")));

        let Some(body) = module.body() else { continue };
        let mut names: Vec<Symbol> = Vec::new();
        for stmt in flatten_statements(body) {
            if let Some(m) = stmt.as_module_node() {
                if module_name_path(&m).as_deref() == Some(&["ClassMethods".to_string()]) {
                    defs_in(m.body(), &mut names);
                }
                continue;
            }
            if let Some(call) = stmt.as_call_node() {
                if call.receiver().is_none()
                    && constant_id_str(&call.name()) == "class_methods"
                {
                    if let Some(block) = call.block().and_then(|b| b.as_block_node()) {
                        defs_in(block.body(), &mut names);
                    }
                }
            }
        }
        if !names.is_empty() {
            out.push((id, names));
        }
    }
    out
}

pub fn ingest_concern_filters(
    source: &[u8],
    file: &str,
) -> Vec<(ClassId, Vec<crate::dialect::Filter>)> {
    let result = parse(source);
    let root = result.node();
    let mut out = Vec::new();
    for (scope, module) in find_all_modules_with_scope(&root) {
        let Some(name_path) = module_name_path(&module) else { continue };
        let mut full_path: Vec<String> = scope.clone();
        full_path.extend(name_path);
        let id = ClassId(Symbol::from(full_path.join("::")));

        let Some(body) = module.body() else { continue };
        let mut filters = Vec::new();
        for stmt in flatten_statements(body) {
            let Some(call) = stmt.as_call_node() else { continue };
            if call.receiver().is_some() || constant_id_str(&call.name()) != "included" {
                continue;
            }
            let Some(block) = call.block().and_then(|b| b.as_block_node()) else { continue };
            let Some(block_body) = block.body() else { continue };
            for inner in flatten_statements(block_body) {
                if let Some(fs) = super::controller::parse_filter_call(&inner, file) {
                    filters.extend(fs);
                } else if let Some(f) = block_form_concern_filter(&inner, file) {
                    filters.push(f);
                }
            }
        }
        if !filters.is_empty() {
            out.push((id, filters));
        }
    }
    out
}

/// A block-form filter in a concern's `included do` — campfire's
/// `SetCurrentRequest` is nothing but `before_action do Current.request =
/// request end`. `parse_filter_call` returns `None` for it (no symbol
/// target), so it fell off the chain entirely: the splice never carried
/// it into any controller, the trace never listed it, and the emitted
/// controllers never set `Current.request`. Captured here as a `Filter`
/// whose `block` is the whole call, kept IN ORDER among the named
/// filters; the splice turns it back into the `Unknown` body item a
/// controller's own block-form filter is, so one lowering serves both.
/// The `__block__` target is a placeholder the splice never emits.
fn block_form_concern_filter(stmt: &ruby_prism::Node<'_>, file: &str) -> Option<crate::dialect::Filter> {
    use crate::dialect::{Filter, FilterKind};
    let call = stmt.as_call_node()?;
    if call.receiver().is_some() || call.block().is_none() {
        return None;
    }
    let kind = match constant_id_str(&call.name()) {
        "before_action" => FilterKind::Before,
        "around_action" => FilterKind::Around,
        "after_action" => FilterKind::After,
        _ => return None,
    };
    let expr = ingest_expr(stmt, file).ok()?;
    Some(Filter {
        target_span: crate::span::Span::synthetic(),
        kind,
        target: Symbol::from("__block__"),
        from_concern: None,
        only: Vec::new(),
        except: Vec::new(),
        only_style: crate::expr::ArrayStyle::default(),
        except_style: crate::expr::ArrayStyle::default(),
        if_cond: None,
        unless_cond: None,
        if_cond_expr: None,
        unless_cond_expr: None,
        block: Some(expr),
    })
}

/// Model DSL declared inside a concern module's `included do` block —
/// `Account::Associations` holds `has_many :statuses` etc. — captured
/// as classified [`crate::dialect::ModelBodyItem`]s per module. Rails
/// evaluates the block in each including model's class body, so these
/// items belong to every includer; analyze registers them (via
/// `App::concern_model_items`) exactly like the model's own
/// declarations. Only the DSL shapes (associations, scopes,
/// validations, callbacks) are kept — arbitrary statements stay with
/// the module. `with_options … do` wrappers are descended (their
/// kwargs refine defaults — `dependent:`, `inverse_of:` — that don't
/// affect the association's type); an item the classifier rejects is
/// survey-recorded and skipped so one exotic line doesn't cost the
/// rest of the block.
/// True when an Unknown body item is a receiverless block-form call to
/// a lifecycle hook the callback lowering handles.
fn unknown_is_block_callback(item: &crate::dialect::ModelBodyItem) -> bool {
    use crate::expr::ExprNode;
    let crate::dialect::ModelBodyItem::Unknown { expr, .. } = item else { return false };
    let ExprNode::Send { recv: None, method, args, block: Some(_), .. } = &*expr.node else {
        return false;
    };
    args.is_empty()
        && crate::lower::model_to_library::BLOCK_CALLBACK_HOOKS
            .contains(&method.as_str())
}

/// Model-DSL MACROS that a lowering expands back out of the
/// `ModelBodyItem::Unknown` holding pen rather than from a variant of
/// their own — `lower::attached`, `lower::rich_text`,
/// `lower::secure_token`, `lower::secure_password`, `lower::has_json`,
/// `lower::typed_store`, `lower::broadcasts`.
///
/// Each one is per-includer DSL exactly like a `has_many`, so a
/// concern's `included do` has to carry it. It didn't: campfire's
/// `Message::Attachment` declares `has_one_attached :attachment` there,
/// the item was dropped on the floor, and `Message#attachment` was
/// never synthesized — six tests died on `undefined local variable or
/// method 'attachment'` while the concern's own `attachment?`, which
/// calls it, emitted right beside the hole.
///
/// Named, not inferred: `Unknown` is a holding pen for everything the
/// classifier doesn't claim, and most of what lands there really does
/// belong to the module rather than to its includers.
const CONCERN_MODEL_MACROS: &[&str] = &[
    "has_one_attached",
    "has_rich_text",
    "has_secure_token",
    "has_secure_password",
    "has_json",
    "typed_store",
    "broadcasts_to",
];

/// True when an Unknown body item is one of [`CONCERN_MODEL_MACROS`].
/// The block form counts — `has_one_attached :avatar do |attachable|
/// attachable.variant … end` declares variants we don't model, and the
/// attachment half still has to expand.
fn unknown_is_model_macro(item: &crate::dialect::ModelBodyItem) -> bool {
    use crate::expr::ExprNode;
    let crate::dialect::ModelBodyItem::Unknown { expr, .. } = item else { return false };
    let ExprNode::Send { recv: None, method, .. } = &*expr.node else { return false };
    CONCERN_MODEL_MACROS.contains(&method.as_str())
}

/// Second return value: `enum` columns declared inside an `included
/// do`, keyed by the concern module. They belong to every includer
/// exactly as the DSL items do; the splice folds them into each
/// including model's own `enums` table.
pub type ConcernModelItems = (
    Vec<(ClassId, Vec<crate::dialect::ModelBodyItem>)>,
    Vec<(ClassId, Vec<(Symbol, Vec<(String, crate::expr::Literal)>)>)>,
);

pub fn ingest_concern_model_items(source: &[u8], file: &str) -> ConcernModelItems {
    use crate::dialect::ModelBodyItem;

    fn walk_dsl_stmts<'pr>(body: ruby_prism::Node<'pr>, out: &mut Vec<ruby_prism::Node<'pr>>) {
        for stmt in flatten_statements(body) {
            if let Some(call) = stmt.as_call_node() {
                if call.receiver().is_none()
                    && constant_id_str(&call.name()) == "with_options"
                {
                    if let Some(block) = call.block().and_then(|b| b.as_block_node()) {
                        if let Some(inner) = block.body() {
                            walk_dsl_stmts(inner, out);
                        }
                        continue;
                    }
                }
            }
            out.push(stmt);
        }
    }

    let result = parse(source);
    let root = result.node();
    let mut out = Vec::new();
    let mut enums_out = Vec::new();
    for (scope, module) in find_all_modules_with_scope(&root) {
        let Some(name_path) = module_name_path(&module) else { continue };
        let mut full_path: Vec<String> = scope.clone();
        full_path.extend(name_path);
        let id = ClassId(Symbol::from(full_path.join("::")));

        let Some(body) = module.body() else { continue };
        let mut items: Vec<ModelBodyItem> = Vec::new();
        let mut enums: Vec<(Symbol, Vec<(String, crate::expr::Literal)>)> = Vec::new();
        for stmt in flatten_statements(body) {
            let Some(call) = stmt.as_call_node() else { continue };
            if call.receiver().is_some() || constant_id_str(&call.name()) != "included" {
                continue;
            }
            let Some(block) = call.block().and_then(|b| b.as_block_node()) else { continue };
            let Some(block_body) = block.body() else { continue };
            let mut stmts = Vec::new();
            walk_dsl_stmts(block_body, &mut stmts);
            for inner in stmts {
                // `enum` inside `included do` belongs to every includer
                // exactly like an association does — campfire declares
                // `enum :role, %i[member administrator bot]` in
                // User::Role. Expanded here for the same reason the
                // model walk expands it: one statement, many items.
                if let Some(call) = inner.as_call_node() {
                    match super::model::expand_enum_decl(&call, file, &[]) {
                        Ok(Some(expanded)) => {
                            enums.push((expanded.column, expanded.mapping));
                            items.extend(expanded.items);
                            continue;
                        }
                        Ok(None) => {}
                        Err(err) => {
                            super::survey::record(&err);
                            continue;
                        }
                    }
                }
                // `_items`, plural: a multi-attribute `validates` (or
                // its `validates_presence_of` spelling) declares one
                // per attribute, and a concern splices ALL of them into
                // every includer — keeping only the first would fault
                // one field of several.
                match super::model::ingest_model_body_items(&inner, &id, file, Vec::new()) {
                    Ok(parsed) => {
                        for item in parsed {
                            match item {
                                ModelBodyItem::Association { .. }
                                | ModelBodyItem::Scope { .. }
                                | ModelBodyItem::Validation { .. }
                                | ModelBodyItem::Callback { .. } => items.push(item),
                                // Block-form lifecycle callbacks
                                // (`after_initialize do … end` —
                                // lobsters' Token concern generates its
                                // unique token there) surface as Unknown
                                // items; keep the ones the callback
                                // lowering understands so the concern
                                // splice carries them into each
                                // includer. Other Unknowns stay with the
                                // module.
                                ModelBodyItem::Unknown { .. } => {
                                    if unknown_is_block_callback(&item)
                                        || unknown_is_model_macro(&item)
                                    {
                                        items.push(item);
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    Err(err) => super::survey::record(&err),
                }
            }
        }
        if !enums.is_empty() {
            enums_out.push((id.clone(), enums));
        }
        if !items.is_empty() {
            out.push((id, items));
        }
    }
    (out, enums_out)
}
