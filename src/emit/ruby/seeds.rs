//! `db/seeds.rb` emission. Uses the general-purpose Ruby expression
//! emitter — seeds.rb is just a top-level Ruby program, so `emit_expr`
//! produces the right shape. Round-trip-identity test asserts this
//! byte-equivalence by ingesting the emitted file and comparing IR.

use std::path::PathBuf;

use super::super::EmittedFile;
use super::expr::emit_expr;
use crate::expr::Expr;

pub(super) fn emit_seeds(expr: &Expr) -> EmittedFile {
    let mut content = emit_expr(expr);
    if !content.ends_with('\n') {
        content.push('\n');
    }
    EmittedFile {
        path: PathBuf::from("db/seeds.rb"),
        content,
    }
}
