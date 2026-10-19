//! Classes that define methods at run time under names computed when
//! they run: `define_singleton_method(clean_name) { … }` with the name
//! from a loop over a YAML file (discourse's `SiteSetting`, through
//! `extend SiteSettingExtension`), `define_method(name)` with `name` an
//! argument. Which names such a class answers is not in the source, so a
//! send the registry does not know there is not a dispatch miss: on the
//! Ruby family the app's own code defines the method when it runs, and
//! the send answers untyped, the gradual escape. Only where that code
//! runs in the emitted class: the class `extend`s the defining module
//! and every class-body call that reaches the definition is one the
//! Ruby family replays (`ingest::app_extension_calls`). The class is
//! reported once, as a warning. A literal-named definition is not
//! this: the class macro expansion turns it into a `def`.

use std::collections::{HashMap, HashSet};

use crate::App;
use crate::dialect::{MethodDef, MethodReceiver, ModelBodyItem};
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::{ClassId, Symbol};
use crate::span::Span;

/// Which side of each class answers computed names, with where one is
/// defined.
#[derive(Debug, Default)]
pub struct OpenClasses {
    class_side: HashMap<ClassId, Span>,
    instance_side: HashMap<ClassId, Span>,
    parents: HashMap<ClassId, ClassId>,
}

impl OpenClasses {
    pub fn of(app: &App) -> Self {
        let mut out = OpenClasses::default();
        for m in &app.models {
            if let Some(p) = &m.parent {
                out.parents.insert(m.name.clone(), p.clone());
            }
        }
        // A module's instance methods that (through its other instance methods) define a computed name on `self`.
        let mut reaching: HashMap<&str, (HashSet<Symbol>, Span)> = HashMap::new();
        let mut reopenings: std::collections::BTreeMap<&str, HashMap<&Symbol, &MethodDef>> = Default::default();
        for lc in app.library_classes.iter().filter(|lc| lc.is_module && lc.origin.is_none()) {
            reopenings.entry(lc.name.0.as_str()).or_default().extend(lc.methods.iter().filter(|m| m.receiver == MethodReceiver::Instance).map(|m| (&m.name, m)));
        }
        for (module, methods) in reopenings {
            let mut found: HashMap<Symbol, Span> = HashMap::new();
            for (name, def) in &methods {
                if let Some(span) = computed_definitions(&def.body).into_iter().find(|(singleton, _)| *singleton).map(|(_, s)| s) {
                    found.insert((*name).clone(), span);
                }
            }
            if found.is_empty() {
                continue;
            }
            let span = *found.values().min_by_key(|s| (s.file.0, s.start)).expect("one");
            loop {
                let before = found.len();
                for (name, def) in &methods {
                    if !found.contains_key(*name) && calls_any(&def.body, &found) {
                        found.insert((*name).clone(), span);
                    }
                }
                if found.len() == before {
                    break;
                }
            }
            reaching.insert(module, (found.into_keys().collect(), span));
        }
        if reaching.is_empty() {
            return out;
        }
        // Open only where every class-body call that reaches the definition is one the Ruby family replays.
        for model in &app.models {
            let mut extended: Vec<&str> = Vec::new();
            let mut covered = true;
            for item in &model.body {
                let ModelBodyItem::Unknown { expr, .. } = item else { continue };
                let replayed = expr.decisions & crate::expr::REPLAYED_CLASS_BODY != 0;
                if let ExprNode::Send { recv: None, method, args, .. } = &*expr.node
                    && method.as_str() == "extend"
                    && replayed
                {
                    for a in args {
                        if let ExprNode::Const { path } = &*a.node {
                            let written = path.iter().map(|s| s.as_str()).filter(|s| !s.is_empty()).collect::<Vec<_>>().join("::");
                            let nested = format!("{}::{written}", model.name.0.as_str());
                            if let Some((name, _)) = reaching.get_key_value(nested.as_str()).or_else(|| reaching.get_key_value(written.as_str())) {
                                extended.push(name);
                            }
                        }
                    }
                    continue;
                }
                let reach: Vec<&HashSet<Symbol>> = extended.iter().map(|m| &reaching[m].0).collect();
                let mut root = true;
                visit(expr, &mut |e| {
                    if let ExprNode::Send { recv, method, .. } = &*e.node
                        && recv.as_ref().is_none_or(|r| matches!(&*r.node, ExprNode::SelfRef))
                        && reach.iter().any(|set| set.contains(method))
                        && !(root && replayed)
                    {
                        covered = false;
                    }
                    root = false;
                });
            }
            if covered && let Some(m) = extended.first() {
                out.class_side.insert(model.name.clone(), reaching[m].1);
            }
        }
        out
    }

    /// Where `class` (or an ancestor) defines computed names on that side.
    pub fn open(&self, class: &ClassId, class_side: bool) -> Option<Span> {
        let table = if class_side { &self.class_side } else { &self.instance_side };
        let mut cur = Some(class);
        for _ in 0..32 {
            let c = cur?;
            if let Some(span) = table.get(c) {
                return Some(*span);
            }
            cur = self.parents.get(c);
        }
        None
    }

    pub fn is_empty(&self) -> bool {
        self.class_side.is_empty() && self.instance_side.is_empty()
    }

    /// Every class with an open side, for the one warning each gets.
    pub fn classes(&self) -> impl Iterator<Item = (&ClassId, bool, &Span)> {
        self.class_side.iter().map(|(c, s)| (c, true, s)).chain(self.instance_side.iter().map(|(c, s)| (c, false, s)))
    }
}

fn calls_any(body: &Expr, names: &HashMap<Symbol, Span>) -> bool {
    let mut found = false;
    visit(body, &mut |e| {
        if let ExprNode::Send { recv, method, .. } = &*e.node
            && recv.as_ref().is_none_or(|r| matches!(&*r.node, ExprNode::SelfRef))
            && names.contains_key(method)
        {
            found = true;
        }
    });
    found
}

/// `define_singleton_method` (`true`) / `define_method` (`false`) on
/// `self` with a name that is not a literal.
fn computed_definitions(body: &Expr) -> Vec<(bool, Span)> {
    let mut out = Vec::new();
    visit(body, &mut |e| {
        let ExprNode::Send { recv, method, args, .. } = &*e.node else { return };
        if recv.as_ref().is_some_and(|r| !matches!(&*r.node, ExprNode::SelfRef)) {
            return;
        }
        let singleton = match method.as_str() {
            "define_singleton_method" => true,
            "define_method" => false,
            _ => return,
        };
        let literal = args.first().is_some_and(|a| matches!(&*a.node, ExprNode::Lit { value: Literal::Sym { .. } | Literal::Str { .. } }));
        if !literal {
            out.push((singleton, e.span));
        }
    });
    out
}

fn visit(e: &Expr, f: &mut dyn FnMut(&Expr)) {
    f(e);
    e.node.for_each_child(&mut |c| visit(c, f));
}
