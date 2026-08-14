//! ActiveRecord literal base class, the `CollectionProxy` runtime helper,
//! the `ActiveRecord::AdapterInterface` contract, and the `Arel` node
//! family. Extracted verbatim from `Analyzer::with_adapter`.

use std::collections::HashMap;

use crate::analyze::ClassInfo;
use crate::ident::{ClassId, Symbol};
use crate::ty::Ty;

pub(in crate::analyze) fn register(classes: &mut HashMap<ClassId, ClassInfo>) {
    // `ActiveRecord::Base` itself — the literal base class, called
    // directly as `ActiveRecord::Base.transaction { ... }` and
    // `ActiveRecord::Base.connection.exec_query(...)`. It sits at the
    // end of every model's parent chain but was never registered as a
    // class, so dispatch on the non-model receiver `Class
    // { ActiveRecord::Base }` found nothing and errored. `transaction`
    // runs the block in a DB transaction (return = the block value,
    // not statically tracked) and `connection` hands back a raw
    // connection adapter — both gradual (`Untyped`), exactly mirroring
    // the per-model class-side framework block above. `or_insert` so a
    // real `active_record/base.rb` library file (none in practice)
    // would still win.
    {
        let mut base = ClassInfo::default();
        for m in [
            "transaction",
            "connection",
            "connection_pool",
            "establish_connection",
        ] {
            base.class_methods.insert(Symbol::from(m), Ty::Untyped);
        }
        classes
            .entry(ClassId(Symbol::from("ActiveRecord::Base")))
            .or_insert(base);
    }

    // CollectionProxy — the runtime helper transpiled models use
    // for has_many associations. `new(...)` returns an instance;
    // iteration/build/create/count/size live on the instance.
    // Registered under the bare last-segment name because the
    // body-typer instantiates `Const { path }` using `path.last()`
    // — see ExprNode::Const branch in analyze/body/mod.rs.
    let cp_class = ClassId(Symbol::from("CollectionProxy"));
    let mut cp_cls = ClassInfo::default();
    cp_cls.class_methods.insert(
        Symbol::from("new"),
        Ty::Class { id: cp_class.clone(), args: vec![] },
    );
    cp_cls.instance_methods.insert(Symbol::from("size"), Ty::Int);
    cp_cls.instance_methods.insert(Symbol::from("length"), Ty::Int);
    cp_cls.instance_methods.insert(Symbol::from("count"), Ty::Int);
    cp_cls.instance_methods.insert(Symbol::from("empty?"), Ty::Bool);
    // `each`, `build`, `create` — return types depend on the target
    // class which isn't known from the proxy type alone. Leave as
    // unknown() placeholders; real resolution requires threading
    // association metadata through the ivar type, which is future
    // work.
    classes.insert(cp_class, cp_cls);

    // `ActiveRecord::AdapterInterface` — the 9-method contract that
    // `runtime/ruby/active_record/base.rb` calls into via
    // `ActiveRecord.adapter.X`. Each per-target runtime ships its
    // own concrete impl (Rust trait + impls in `runtime/rust/`,
    // Crystal abstract class + SqliteAdapter, TS interface +
    // SqliteActiveRecordAdapter). On the
    // Ruby side there's no class declaration — the RBS for
    // `ActiveRecord.adapter` previously returned `untyped`, which
    // let TS get away with `any` but left rust2 emit producing
    // method calls on `serde_json::Value` (E0599 on
    // `.find/.where/.all/.insert/.update/.delete/.count/.exists/.truncate`).
    // Registering it here gives the body-typer a concrete class to
    // dispatch against; the RBS sidecar then references it as
    // `() -> AdapterInterface`.
    let hash_str_untyped = Ty::Hash {
        key: Box::new(Ty::Str),
        value: Box::new(Ty::Untyped),
    };
    let row_ty = hash_str_untyped.clone();
    let nilable_row = Ty::Union {
        variants: vec![row_ty.clone(), Ty::Nil],
    };
    let array_of_rows = Ty::Array { elem: Box::new(row_ty.clone()) };
    let mut adapter_iface = ClassInfo::default();
    adapter_iface
        .instance_methods
        .insert(Symbol::from("all"), array_of_rows.clone());
    adapter_iface
        .instance_methods
        .insert(Symbol::from("find"), nilable_row.clone());
    adapter_iface
        .instance_methods
        .insert(Symbol::from("where"), array_of_rows.clone());
    adapter_iface
        .instance_methods
        .insert(Symbol::from("count"), Ty::Int);
    adapter_iface
        .instance_methods
        .insert(Symbol::from("exists?"), Ty::Bool);
    adapter_iface
        .instance_methods
        .insert(Symbol::from("insert"), Ty::Int);
    adapter_iface
        .instance_methods
        .insert(Symbol::from("update"), Ty::Nil);
    adapter_iface
        .instance_methods
        .insert(Symbol::from("delete"), Ty::Nil);
    adapter_iface
        .instance_methods
        .insert(Symbol::from("truncate"), Ty::Nil);
    classes.insert(
        ClassId(Symbol::from("ActiveRecord::AdapterInterface")),
        adapter_iface,
    );

    // Arel — the low-level SQL AST that advanced scopes reach for
    // (`Model.arel_table[:col].not_in(subquery)`, `relation.arel.exists`,
    // `Arel.sql(...)`). A small class family whose methods all return
    // Arel nodes (never `Untyped`), so a chain that drops into Arel
    // stays typed instead of collapsing to a gradual escape at the
    // first `arel_table`/`arel`/`Arel.sql` hop. Precision is coarse —
    // every predicate/combinator returns the same `Arel::Node`; the
    // win is that the chain resolves rather than which node it is.
    let arel_node = Ty::Class { id: ClassId(Symbol::from("Arel::Node")), args: vec![] };
    let arel_attribute_ty =
        Ty::Class { id: ClassId(Symbol::from("Arel::Attribute")), args: vec![] };
    let arel_select_mgr =
        Ty::Class { id: ClassId(Symbol::from("Arel::SelectManager")), args: vec![] };

    // `Arel.sql(...)` / `Arel.star` — module-level node constructors.
    let mut arel_mod = ClassInfo::default();
    arel_mod.class_methods.insert(Symbol::from("sql"), arel_node.clone());
    arel_mod.class_methods.insert(Symbol::from("star"), arel_node.clone());
    classes.insert(ClassId(Symbol::from("Arel")), arel_mod);

    // `Model.arel_table` → table; `table[:col]` → attribute. A table
    // also delegates query-builder calls to a select manager
    // (`table.project(Arel.star)`, `table.where(...)`).
    let mut arel_table = ClassInfo::default();
    arel_table.instance_methods.insert(Symbol::from("[]"), arel_attribute_ty.clone());
    for m in [
        "project", "where", "order", "group", "having", "join", "on",
        "take", "skip", "from", "distinct",
    ] {
        arel_table.instance_methods.insert(Symbol::from(m), arel_select_mgr.clone());
    }
    classes.insert(ClassId(Symbol::from("Arel::Table")), arel_table);

    // `Arel::Attribute` predicates → node.
    let mut arel_attribute = ClassInfo::default();
    for pred in [
        "eq", "not_eq", "in", "not_in", "gt", "gteq", "lt", "lteq",
        "matches", "does_not_match", "between", "eq_any", "in_any",
        "asc", "desc", "count", "sum", "minimum", "maximum", "average",
    ] {
        arel_attribute.instance_methods.insert(Symbol::from(pred), arel_node.clone());
    }
    classes.insert(ClassId(Symbol::from("Arel::Attribute")), arel_attribute);

    // `Arel::Node` boolean combinators chain into nodes; `where(node)`
    // already accepts any argument type.
    let mut arel_node_cls = ClassInfo::default();
    for m in ["and", "or", "not"] {
        arel_node_cls.instance_methods.insert(Symbol::from(m), arel_node.clone());
    }
    classes.insert(ClassId(Symbol::from("Arel::Node")), arel_node_cls);

    // `relation.arel` / `Model.arel` → select manager; `.exists` →
    // node; further builder calls stay on the manager.
    let mut arel_select = ClassInfo::default();
    arel_select.instance_methods.insert(Symbol::from("exists"), arel_node.clone());
    for m in ["where", "project", "join", "on", "group", "order", "take", "skip"] {
        arel_select.instance_methods.insert(Symbol::from(m), arel_select_mgr.clone());
    }
    classes.insert(ClassId(Symbol::from("Arel::SelectManager")), arel_select);
}

/// Action Text's VALUE surface — `ActionText::Content` (the coder a
/// `has_rich_text` attribute reads back through) and
/// `ActionText::Attachment` (one parsed `<action-text-attachment>`
/// node). Both are implemented in `runtime/ruby/action_text.rb`; this
/// registration is what lets an app call `message.body.to_plain_text`
/// and have it type.
///
/// `ActionText::RichText` is deliberately absent: it has a table, so
/// `lower::rich_text` synthesizes it as an ordinary Model and it
/// registers through the model loop like every other. Registering a
/// stand-in here would shadow the real one.
///
/// Unconditional, like every other entry in this file — an app with no
/// `has_rich_text` simply never dispatches against these.
pub(in crate::analyze) fn register_action_text(classes: &mut HashMap<ClassId, ClassInfo>) {
    let attachment_id = ClassId(Symbol::from("ActionText::Attachment"));
    let attachment_ty = Ty::Class { id: attachment_id.clone(), args: vec![] };

    let mut attachment = ClassInfo::default();
    for m in ["sgid", "content_type", "caption", "filename", "url", "to_plain_text", "[]"] {
        attachment.instance_methods.insert(Symbol::from(m), Ty::Str);
    }
    attachment.instance_methods.insert(
        Symbol::from("attributes"),
        Ty::Hash { key: Box::new(Ty::Str), value: Box::new(Ty::Str) },
    );
    // Read as a constant by content filters (`Attachment.tag_name`),
    // which is why it is a class method and not a bare literal.
    attachment.class_methods.insert(Symbol::from("tag_name"), Ty::Str);
    classes.insert(attachment_id, attachment);

    let mut content = ClassInfo::default();
    for m in ["to_html", "to_s", "as_json", "to_plain_text"] {
        content.instance_methods.insert(Symbol::from(m), Ty::Str);
    }
    for m in ["blank?", "empty?", "present?"] {
        content.instance_methods.insert(Symbol::from(m), Ty::Bool);
    }
    content
        .instance_methods
        .insert(Symbol::from("links"), Ty::Array { elem: Box::new(Ty::Str) });
    content
        .instance_methods
        .insert(Symbol::from("attachments"), Ty::Array { elem: Box::new(attachment_ty) });
    // DIVERGENCE, and typed as such: Rails resolves each attachment's
    // signed GlobalID back to the record it points at, so this is
    // `Array[ActionText::Attachable]` there. Nothing dereferences an
    // sgid here yet, so the runtime returns an empty list and the
    // element type is gradual rather than a lie about which model
    // comes back.
    content
        .instance_methods
        .insert(Symbol::from("attachables"), Ty::Array { elem: Box::new(Ty::Untyped) });
    classes.insert(ClassId(Symbol::from("ActionText::Content")), content);
}
