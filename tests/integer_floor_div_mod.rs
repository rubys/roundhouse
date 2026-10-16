//! Ruby's `Integer#/` floors (`-7 / 2 == -4`) and `Integer#%` takes the
//! sign of the divisor (`-7 % 3 == 2`). TypeScript, Python and Elixir `/`
//! yield a float, Elixir's `rem` takes the dividend's sign, and Rust's
//! `/`/`%` and JS's `%` truncate toward zero, so each emitter renders
//! an Int/Int pair in its floored form. Rust also had no operator arm
//! for `%` and emitted `x.%(y)`, which does not parse.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::project::BuildTarget;

fn emitted(target: BuildTarget, path: &str) -> String {
    let (dir, errors) = emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n",
            "class Article < ApplicationRecord\n  \
             def self.floor_div\n    a = -7\n    a / 2\n  end\n\n  \
             def self.floor_mod\n    a = -7\n    a % 3\n  end\n\n  \
             def self.floor_mod_neg_div\n    a = 7\n    a % -3\n  end\n\n  \
             def self.float_div\n    7.0 / 2.0\n  end\n\n",
        )
        .emit(target);
    assert!(errors.is_empty(), "errors:\n{}", errors.join("\n"));
    std::fs::read_to_string(dir.join(path)).unwrap_or_else(|e| panic!("read {path}: {e}"))
}

#[test]
fn rust_floors_int_div_and_mod() {
    let out = emitted(BuildTarget::Rust, "src/models/article.rs");
    assert!(!out.contains(".%("), "`%` emitted as a method call:\n{out}");
    assert!(
        out.contains("if b == -1 { a.wrapping_neg() } else { let q = a / b; if a % b != 0 && ((a < 0) != (b < 0)) { q - 1 } else { q } }"),
        "Int / Int not floored:\n{out}"
    );
    assert!(
        out.contains("let m = if b == -1 { 0 } else { a % b }; if m != 0 && ((m < 0) != (b < 0)) { m + b } else { m }"),
        "Int % Int not floored:\n{out}"
    );
    // Negative divisor (`7 % -3 == -2`) must still hit the floored
    // helper; a "fix negative rem only" form would still pass the
    // `-7 % 3` site alone.
    assert!(out.contains("-3_i64"), "neg-divisor mod missing:\n{out}");
    assert!(out.contains("7.0 / 2.0"), "Float / Float should stay native:\n{out}");
}

#[test]
fn typescript_floors_int_div_and_mod() {
    let out = emitted(BuildTarget::Typescript, "app/models/article.ts");
    assert!(out.contains("Math.floor(a / 2)"), "Int / Int not floored:\n{out}");
    assert!(out.contains("const __m = __a % __b"), "Int % Int not floored:\n{out}");
    // Operands are IIFE call args so `await` stays outside the sync arrow.
    assert!(
        out.contains("((__a, __b) =>") || out.contains("(__a, __b) =>"),
        "Int % Int must take operands as IIFE params:\n{out}"
    );
    // Add the divisor only on a sign mismatch: a second `% __b` over
    // `__m + __b` would round sums past 2**53. Rem-vs-divisor (not
    // "negative rem only") so `7 % -3` stays `-2`.
    assert!(out.contains("__m + __b :"), "Int % Int not floored:\n{out}");
    // Printer omits parens: `__m < 0 !== __b < 0` (rem vs divisor).
    assert!(
        out.contains("__m < 0 !== __b < 0"),
        "Int % Int must compare rem and divisor signs:\n{out}"
    );
    assert!(!out.contains("+ __b) % __b"), "Int % Int sums before flooring:\n{out}");
    assert!(out.contains(", -3)") || out.contains(",-3)"), "neg-divisor mod missing:\n{out}");
    assert!(!out.contains("Math.floor(7.0 / 2.0)") && !out.contains("Math.floor(7 / 2)"),
        "Float / Float must not floor:\n{out}");
}

#[test]
fn python_int_div_is_floor_div() {
    let out = emitted(BuildTarget::Python, "app/v2/models.py");
    assert!(out.contains("a // 2"), "Int / Int not `//`:\n{out}");
    // Python's `%` already follows the divisor's sign.
    assert!(out.contains("a % 3"), "{out}");
    assert!(out.contains("a % -3"), "neg-divisor mod missing:\n{out}");
    assert!(out.contains("7.0 / 2.0"), "Float / Float should stay `/`:\n{out}");
}

#[test]
fn elixir_floors_int_div_and_mod() {
    let out = emitted(BuildTarget::Elixir, "lib/article.ex");
    assert!(out.contains("Integer.floor_div(a, 2)"), "Int / Int not floored:\n{out}");
    assert!(out.contains("Integer.mod(a, 3)"), "Int % Int not floored:\n{out}");
    assert!(out.contains("Integer.mod(a, -3)"), "neg-divisor mod missing:\n{out}");
    assert!(out.contains("7.0 / 2.0"), "Float / Float should stay `/`:\n{out}");
}
