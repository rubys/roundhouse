use super::emit_and_run;
use roundhouse::ty::Ty;

const MODEL: &str = include_str!("branch_local_join_model.rb");

/// A local assigned on some paths through `if`/`case`/`while`/`&&`/
/// `rescue` is bound after the join on every path, nil where it was not
/// assigned. Each method's tail read takes the union of the paths.
#[test]
fn a_local_assigned_on_some_paths_reads_as_the_join_of_them() {
    let (_emitted, app, errors) = emit_and_run::empty_app()
        .write("app/services/branch_local_join.rb", MODEL)
        .emit_with_app(roundhouse::project::BuildTarget::Ruby);
    assert!(errors.is_empty(), "{errors:?}");
    let class = app
        .library_classes
        .iter()
        .find(|c| c.name.0.as_str() == "BranchLocalJoin")
        .expect("BranchLocalJoin");
    let union = |variants: Vec<Ty>| Ty::Union { variants: variants.into() };
    let expected = [
        ("if_only", union(vec![Ty::Int, Ty::Nil])),
        ("if_else", union(vec![Ty::Int, Ty::Str])),
        // `w = 0` alone used to stand: Int, which the "s" path contradicts.
        ("prior_widens", union(vec![Ty::Int, Ty::Str])),
        // The `return` path joins nothing.
        ("else_returns", Ty::Int),
        ("case_without_else", union(vec![Ty::Int, Ty::Nil])),
        ("modifier_unless", union(vec![Ty::Int, Ty::Nil])),
        ("while_body", union(vec![Ty::Int, Ty::Nil])),
        ("and_right", union(vec![Ty::Int, Ty::Nil])),
    ];
    for (name, want) in expected {
        let method = class
            .methods
            .iter()
            .find(|m| m.name.as_str() == name)
            .unwrap_or_else(|| panic!("{name}"));
        let roundhouse::expr::ExprNode::Seq { exprs } = &*method.body.node else {
            panic!("{name}: body is not a Seq");
        };
        let tail = exprs.last().unwrap();
        assert_eq!(tail.ty.as_ref(), Some(&want), "{name}: tail read");
    }
}

/// The emitted program answers what Ruby answers on every path.
#[test]
fn branch_assigned_locals_run_as_ruby_runs_them() {
    let expected = emit_and_run::ruby()
        .arg("-e")
        .arg(format!("{MODEL}\np BranchLocalJoin.values"))
        .output()
        .expect("ruby oracle");
    assert!(expected.status.success(), "{}", String::from_utf8_lossy(&expected.stderr));
    let expected = String::from_utf8(expected.stdout).unwrap();
    emit_and_run::empty_app()
        .write("app/services/branch_local_join.rb", MODEL)
        .run_ruby(&format!(
            "expected = {}\nactual = BranchLocalJoin.values\nraise actual.inspect unless actual == expected\nputs \"branch-assigned locals match Ruby\"\n",
            expected.trim()
        ))
        .assert_passes();
}
