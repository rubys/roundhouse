//! Observe generated ownership once per App, using the same model
//! synthesis and Ruby-family IR producers as emission. Source methods
//! remain inputs to derived producers, but their existing ivars are not
//! framework storage. No rendering or file generation runs here.
//!
//! A candidate-bearing App costs one isolated clone/analyze/post-lower,
//! masked synthesis of candidates/ancestors, and two Ruby adaptations.
//! Observe before ingest returns: source-shaped check/editor consumers
//! deliberately do not run the real post-lowering pipeline themselves.

use std::collections::{HashMap, HashSet};

use crate::App;
use crate::dialect::{LibraryClass, MethodDef, MethodReceiver, Model};
use crate::expr::{Expr, ExprNode, LValue};
use crate::ident::{ClassId, Symbol};
use crate::span::Span;

fn ivars(expr: &Expr, out: &mut HashSet<(Span, Symbol)>) {
    match &*expr.node {
        ExprNode::Ivar { name }
        | ExprNode::Assign {
            target: LValue::Ivar { name },
            ..
        }
        | ExprNode::OpAssign {
            target: LValue::Ivar { name },
            ..
        } => {
            out.insert((expr.span, name.clone()));
        }
        _ => {}
    }
    expr.node.for_each_child(&mut |child| ivars(child, out));
}

fn generated_ivars(expr: &Expr, source: &HashSet<(Span, Symbol)>, out: &mut HashSet<Symbol>) {
    let mut nodes = HashSet::new();
    ivars(expr, &mut nodes);
    // Backfilled generated spans are real too. Compare actual original
    // ivar occurrences, not merely FileId or an ivar-name blacklist.
    out.extend(
        nodes
            .into_iter()
            .filter(|node| !source.contains(node))
            .map(|(_, name)| name),
    );
}

fn is_source_method(model: &Model, method: &MethodDef) -> bool {
    model.methods().any(|source| {
        !source.name_span.is_synthetic()
            && source.name_span == method.name_span
            && source.receiver == method.receiver
            && source.name == method.name
    })
}

fn collect_surfaces(
    classes: &[LibraryClass],
    models: &HashMap<ClassId, &Model>,
    source_ivars: &HashSet<(Span, Symbol)>,
    surfaces: &mut HashMap<ClassId, HashSet<Symbol>>,
) {
    for class in classes {
        // Row siblings traverse the real producer sequence too, but
        // are not model ownership and must not leak into admission.
        let Some(model) = models.get(&class.name) else {
            continue;
        };
        let occupied = surfaces.entry(class.name.clone()).or_default();
        for method in &class.methods {
            if !is_source_method(model, method) {
                occupied.insert(method.name.clone());
            }
            generated_ivars(&method.body, source_ivars, occupied);
        }
    }
}

/// Requested models get an entry; `None` denotes an abstract, non-schema
/// or ambiguously defined model, which cannot admit this accessor contract.
pub(crate) fn occupied_surfaces(
    app: &App,
    requested: &HashSet<ClassId>,
) -> HashMap<ClassId, Option<HashSet<Symbol>>> {
    let models: HashMap<_, _> = app.models.iter().map(|m| (m.name.clone(), m)).collect();
    let mut selected = HashSet::new();
    let mut pending: Vec<_> = requested.iter().cloned().collect();
    while let Some(id) = pending.pop() {
        if !selected.insert(id.clone()) {
            continue;
        }
        if let Some(parent) = models.get(&id).and_then(|m| m.parent.as_ref()) {
            if models.contains_key(parent) {
                pending.push(parent.clone());
            }
        }
    }
    observe(app, requested, &selected)
}

/// Selection restricts materialization, never typing or demand inputs.
/// Selection equivalence is tested separately from an ordinary
/// production-materialization oracle; neither compares rendered text.
fn observe(
    app: &App,
    requested: &HashSet<ClassId>,
    selected: &HashSet<ClassId>,
) -> HashMap<ClassId, Option<HashSet<Symbol>>> {
    // Isolate both mutable analysis data and speculative diagnostics,
    // including permit collection. Preserve original whole-app demand.
    let (surfaces, _) = crate::emit::diagnostics::scope(|| {
        let mut probe = app.clone();
        let mut source_ivars = HashSet::new();
        crate::lower::for_each_hook_body(&mut probe, &mut |body| ivars(body, &mut source_ivars));
        // Faithful lookup is essential: removing a local accessor here
        // could select an inherited reader of a different type and hide
        // a real post-analysis producer (e.g. the JSON writer).
        crate::session::analyze_and_lower(&mut probe);
        let models: HashMap<_, _> = probe.models.iter().map(|m| (m.name.clone(), m)).collect();
        let (mut classes, _) = crate::emit::ruby::materialize_models(
            &probe,
            super::Materialization::AccessorProbe(selected),
        );
        let mut surfaces = HashMap::new();
        collect_surfaces(&classes, &models, &source_ivars, &mut surfaces);
        // Late producers can also yield to user overrides. Observe a
        // generated-only vector and an independent source-bearing one:
        // the latter retains inputs needed for derived helper clones.
        {
            let mut generated = classes.clone();
            for class in &mut generated {
                if let Some(model) = models.get(&class.name) {
                    class
                        .methods
                        .retain(|method| !is_source_method(model, method));
                }
            }
            crate::emit::ruby::apply_model_lowering(&mut generated, &probe);
            collect_surfaces(&generated, &models, &source_ivars, &mut surfaces);
        }
        crate::emit::ruby::apply_model_lowering(&mut classes, &probe);
        collect_surfaces(&classes, &models, &source_ivars, &mut surfaces);
        surfaces
    });
    inherit_surfaces(app, requested, surfaces)
}

fn inherit_surfaces(
    app: &App,
    requested: &HashSet<ClassId>,
    mut surfaces: HashMap<ClassId, HashSet<Symbol>>,
) -> HashMap<ClassId, Option<HashSet<Symbol>>> {
    static BASE_SURFACE: std::sync::OnceLock<HashSet<Symbol>> = std::sync::OnceLock::new();
    let base = BASE_SURFACE.get_or_init(|| {
        let methods = crate::runtime_src::parse_methods(include_str!(
            "../../../runtime/ruby/active_record/base.rb"
        ))
        .expect("framework Base method bodies must ingest");
        let mut base = HashSet::new();
        for method in methods {
            if method.receiver == MethodReceiver::Instance {
                base.insert(method.name.clone());
                generated_ivars(&method.body, &HashSet::new(), &mut base);
            }
        }
        base
    });
    // Every includer also inherits its ancestors' generated ownership.
    // Read the unmerged sets so traversal order cannot affect admission.
    let own = surfaces.clone();
    let mut models = HashMap::new();
    let mut ambiguous = HashSet::new();
    for model in &app.models {
        if models.insert(model.name.clone(), model).is_some() {
            ambiguous.insert(model.name.clone());
        }
    }
    for model in models.values().filter(|m| requested.contains(&m.name)) {
        let occupied = surfaces.get_mut(&model.name).unwrap();
        occupied.extend(base.iter().cloned());
        let mut seen = HashSet::new();
        let mut parent = model.parent.as_ref();
        while let Some(id) = parent {
            if !seen.insert(id.clone()) {
                break;
            }
            if let Some(names) = own.get(id) {
                occupied.extend(names.iter().cloned());
            }
            parent = models.get(id).and_then(|m| m.parent.as_ref());
        }
    }
    // Eligibility changes the result, never the synthesis inputs or
    // ancestor/demand inventory: abstract bases still own storage.
    // Emission does not merge model reopenings: a later definition can
    // overwrite the file carrying an earlier one's admitted accessors.
    models
        .values()
        .filter(|model| requested.contains(&model.name))
        .map(|model| {
            let names = surfaces.remove(&model.name).unwrap();
            let eligible = !ambiguous.contains(&model.name)
                && !super::markers::is_abstract_class(model)
                && app.schema.tables.contains_key(&model.table.0);
            (model.name.clone(), eligible.then_some(names))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(files: &[(&str, &str)]) -> App {
        crate::ingest::ingest_app_from_tree(
            files
                .iter()
                .map(|(path, text)| (std::path::PathBuf::from(path), text.as_bytes().to_vec()))
                .collect(),
        )
        .unwrap()
    }

    fn projection(app: &App, names: &[&str]) -> HashMap<ClassId, Option<HashSet<Symbol>>> {
        let requested = names
            .iter()
            .map(|name| ClassId(Symbol::from(*name)))
            .collect();
        let all = app.models.iter().map(|model| model.name.clone()).collect();
        let selected = occupied_surfaces(app, &requested);
        assert_eq!(
            selected,
            observe(app, &requested, &all),
            "selecting models must preserve occupancy, not necessarily rendered bodies"
        );
        assert_eq!(
            selected.len(),
            names.len(),
            "unrelated models must not escape the query"
        );
        selected
    }

    // Ordinary production entry, not the observation branch. Retain
    // all models and Rows; collect demand independently of the new seam.
    fn production_classes(app: &App) -> Vec<LibraryClass> {
        let mut params =
            crate::lower::controller_to_library::params::collect_specs(&app.controllers);
        params.mark_file_fields(&app.models);
        let unfolded = crate::lower::scope_chain::assoc_query_method_names(
            &crate::lower::scope_chain::survey_assoc_class_methods(
                app,
                &crate::lower::scope_chain::build_assoc_registry(&app.models),
                &crate::lower::scope_chain::build_scope_registry(&app.models),
            )
            .0,
        );
        super::super::lower_models_to_library_classes_unfolding(
            &app.models,
            &app.schema,
            Vec::new(),
            &params,
            &unfolded,
        )
    }

    #[test]
    fn occupied_names_match_independent_production_on_unmasked_controls() {
        let mut input = app(&[
            (
                "db/schema.rb",
                "ActiveRecord::Schema.define do\n  create_table :messages do |t|\n    t.string :body\n    t.datetime :created_at\n  end\n  create_table :comments do |t|\n    t.integer :message_id\n  end\nend\n",
            ),
            (
                "app/models/message_base.rb",
                "class MessageBase < ApplicationRecord\n  self.abstract_class = true\n  self.table_name = 'messages'\n  has_many :comments\n  def decorate(value)\n    @scratch = value\n    value\n  end\nend\n",
            ),
            (
                "app/models/message.rb",
                "class Message < MessageBase\nend\n",
            ),
            (
                "app/models/comment.rb",
                "class Comment < ApplicationRecord\n  belongs_to :message\nend\n",
            ),
            (
                "app/helpers/application_helper.rb",
                "module ApplicationHelper\n  def decorate(value)\n    value\n  end\nend\n",
            ),
            (
                "app/views/comments/index.html.erb",
                "<%= decorate(raw('hello')) %>",
            ),
            (
                "app/controllers/comments_controller.rb",
                "class CommentsController < ApplicationController\n  def message_params\n    params.require(:message).permit(:body)\n  end\nend\n",
            ),
        ]);
        let requested = [ClassId(Symbol::from("Message"))].into_iter().collect();
        let actual = occupied_surfaces(&input, &requested);
        let mut source_ivars = HashSet::new();
        crate::lower::for_each_hook_body(&mut input, &mut |body| ivars(body, &mut source_ivars));
        crate::session::analyze_and_lower(&mut input);
        let models = input.models.iter().map(|m| (m.name.clone(), m)).collect();
        let mut emitted = production_classes(&input);
        let mut expected = HashMap::new();
        collect_surfaces(&emitted, &models, &source_ivars, &mut expected);
        crate::emit::ruby::apply_model_lowering(&mut emitted, &input);
        collect_surfaces(&emitted, &models, &source_ivars, &mut expected);
        assert_eq!(actual, inherit_surfaces(&input, &requested, expected));
        let names = actual[&ClassId(Symbol::from("Message"))].as_ref().unwrap();
        for name in ["comments", "decorate_raw", "__t_created_at", "from_params"] {
            assert!(
                names.contains(&Symbol::from(name)),
                "production must demand {name}"
            );
        }
        assert!(!names.contains(&Symbol::from("scratch")));
        assert!(!names.contains(&Symbol::from("message_id=")));
    }

    #[test]
    fn selected_materialization_keeps_production_arel_and_unfolding() {
        let mut input = app(&[
            (
                "db/schema.rb",
                "ActiveRecord::Schema.define do\n  create_table :messages do |t|\n    t.string :body\n  end\n  create_table :comments do |t|\n    t.integer :message_id\n  end\nend\n",
            ),
            (
                "app/models/message.rb",
                "class Message < ApplicationRecord\n  has_many :comments\n  def total_comments\n    Comment.count\n  end\n  def counted_comments\n    comments.counted\n  end\nend\n",
            ),
            (
                "app/models/comment.rb",
                "class Comment < ApplicationRecord\n  belongs_to :message\n  def self.counted\n    count\n  end\nend\n",
            ),
        ]);
        crate::session::analyze_and_lower(&mut input);
        let selected: HashSet<_> = ["Message", "Comment"]
            .into_iter()
            .map(|name| ClassId(Symbol::from(name)))
            .collect();
        let (probe, _) = crate::emit::ruby::materialize_models(
            &input,
            super::super::Materialization::AccessorProbe(&selected),
        );
        let ordinary = production_classes(&input);
        for (owner, name) in [("Message", "total_comments"), ("Comment", "counted")] {
            let find = |classes: &[LibraryClass]| {
                classes
                    .iter()
                    .find(|c| c.name.0.as_str() == owner)
                    .unwrap()
                    .methods
                    .iter()
                    .find(|m| m.name.as_str() == name)
                    .unwrap()
                    .clone()
            };
            let expected = find(&ordinary);
            assert_eq!(find(&probe), expected, "{owner}#{name}");
            if name == "total_comments" {
                assert!(
                    serde_json::to_string(&expected.body)
                        .unwrap()
                        .contains("SELECT COUNT(*) FROM comments"),
                    "cross-model count must be arel-lowered"
                );
            } else {
                assert!(
                    matches!(&*expected.body.node, ExprNode::Send { recv: Some(recv), method, .. }
                    if method.as_str() == "count" && matches!(&*recv.node, ExprNode::Const { path } if path == &[Symbol::from("Comment")])),
                    "association count must stay unfolded"
                );
            }
        }
        let one = [ClassId(Symbol::from("Message"))].into_iter().collect();
        let (bounded, _) = crate::emit::ruby::materialize_models(
            &input,
            super::super::Materialization::AccessorProbe(&one),
        );
        assert_eq!(
            bounded
                .iter()
                .map(|c| c.name.0.as_str())
                .collect::<Vec<_>>(),
            ["Message", "MessageRow"]
        );
    }

    #[test]
    fn selection_preserves_ancestor_storage_raw_helpers_and_unrelated_demand() {
        let input = app(&[
            (
                "db/schema.rb",
                "ActiveRecord::Schema.define do\n  create_table :messages do |t|\n    t.string :body\n    t.integer :parent_id\n    t.datetime :created_at\n  end\n  create_table :comments do |t|\n    t.integer :message_id\n  end\nend\n",
            ),
            (
                "app/models/message_base.rb",
                "class MessageBase < ApplicationRecord\n  self.abstract_class = true\n  self.table_name = 'messages'\n  has_many :comments\n  def decorate(value)\n    @scratch = value\n    value\n  end\nend\n",
            ),
            (
                "app/models/message.rb",
                "class Message < MessageBase\n  belongs_to :parent, class_name: 'Message', optional: true\n  scope :ordered, -> { order(:id) }\n  def _preload_parent(rec)\n    nil\n  end\nend\n",
            ),
            (
                "app/models/comment.rb",
                "class Comment < ApplicationRecord\n  belongs_to :message\nend\n",
            ),
            (
                "app/helpers/application_helper.rb",
                "module ApplicationHelper\n  def decorate(value)\n    value\n  end\nend\n",
            ),
            (
                "app/views/comments/index.html.erb",
                "<%= decorate(raw('hello')) %>",
            ),
            (
                "app/controllers/comments_controller.rb",
                "class CommentsController < ApplicationController\n  def index\n    @messages = Message.ordered.includes(:parent).to_a\n  end\n  def message_params\n    params.require(:message).permit(:body)\n  end\nend\n",
            ),
        ]);
        let projected = projection(&input, &["Message", "Comment", "MessageBase"]);
        let names = projected[&ClassId(Symbol::from("Message"))]
            .as_ref()
            .unwrap();
        for name in [
            "decorate_raw",
            "comments",
            "__t_created_at",
            "_preload_parent",
            "from_params",
        ] {
            assert!(
                names.contains(&Symbol::from(name)),
                "missing independently demanded {name}"
            );
        }
        assert!(
            !names.contains(&Symbol::from("scratch")),
            "copied source ivars are not generated storage"
        );
        assert!(projected[&ClassId(Symbol::from("MessageBase"))].is_none());
        // A single request really excludes the unrelated model's materialization.
        projection(&input, &["Message"]);
    }

    #[test]
    fn slice_wide_route_ambiguity_helper_shadowing_and_constant_collisions_keep_ownership() {
        let input = app(&[
            (
                "db/schema.rb",
                "ActiveRecord::Schema.define do\n  create_table :messages do |t|\n    t.string :body\n  end\n  create_table :comments do |t|\n    t.integer :message_id\n  end\nend\n",
            ),
            (
                "app/models/message.rb",
                "class Message < ApplicationRecord\n  HOISTED_IMAGE_TAG_STAMP_SVG = 'reserved'\n  def to_param\n    body\n  end\n  def selected_record\n    Message.first\n  end\n  def label\n    image_tag('stamp.svg', alt: 'message')\n  end\n  def link\n    message_path(selected_record) + label\n  end\nend\n",
            ),
            (
                "app/models/comment.rb",
                "class Comment < ApplicationRecord\n  def selected_record\n    Comment.first\n  end\n  def label\n    image_tag('stamp.svg', alt: 'comment')\n  end\nend\n",
            ),
            (
                "app/helpers/application_helper.rb",
                "module ApplicationHelper\n  def label\n    'helper'\n  end\nend\n",
            ),
            (
                "config/routes.rb",
                "Rails.application.routes.draw { resources :messages; resources :comments }",
            ),
        ]);
        let projected = projection(&input, &["Message"]);
        let names = projected[&ClassId(Symbol::from("Message"))]
            .as_ref()
            .unwrap();
        assert!(names.contains(&Symbol::from("body=")));
        assert!(!names.contains(&Symbol::from("selected_record")));
        assert!(!names.contains(&Symbol::from("label")));
        assert!(
            !names.contains(&Symbol::from("message_id=")),
            "unrelated schema must not leak"
        );
        projection(&input, &["Comment", "Message"]);
    }

    #[test]
    fn literal_abstract_eligibility_obeys_the_last_declaration() {
        for (declaration, abstract_class) in [
            ("self.abstract_class = true", true),
            ("self.abstract_class = false", false),
            (
                "primary_abstract_class\n  self.abstract_class = false",
                false,
            ),
            (
                "self.abstract_class = false\n  primary_abstract_class",
                true,
            ),
        ] {
            let source = format!("class Message < ApplicationRecord\n  {declaration}\nend\n");
            let input = app(&[
                (
                    "db/schema.rb",
                    "ActiveRecord::Schema.define do\n  create_table :messages do |t|\n    t.string :body\n  end\nend\n",
                ),
                ("app/models/message.rb", &source),
            ]);
            let projected = projection(&input, &["Message"]);
            assert_eq!(
                projected[&ClassId(Symbol::from("Message"))].is_none(),
                abstract_class,
                "{declaration}"
            );
        }
    }

    #[test]
    fn selected_blog_ownership_matches_complete_materialization() {
        let input = crate::ingest::ingest_app(crate::fixtures::real_blog()).unwrap();
        projection(&input, &["Article"]);
        projection(&input, &["Comment"]);
    }
}
