//! String kernel shared by analysis and the emitters that already
//! mirror it.
//!
//! One row is `(String, name, arity) → return kind + per-target
//! spelling`. `str_method` reads the return kind by name.
//! Rust and TypeScript print the spelling for `arity` instead of
//! matching the Ruby name again. Crystal reads the spelling for the
//! same names at every arity; its `length` / `include?` /
//! `start_with?` / `end_with?` renames were not receiver-specific
//! and had no arity guard, so `crystal_name` stays name-keyed.
//!
//! Analyze types the name at every arity. The ladder this table
//! replaces did not consult arity, and a wrong-arity call staying
//! typed is what keeps it off the error ledger. `arity` is only the
//! Rust and TypeScript bridge.

use crate::ty::Ty;

/// Return kind of one kernel method. Only the three shapes this
/// subset actually returns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KernelReturn {
    Int,
    Bool,
    Str,
}

impl KernelReturn {
    pub fn ty(self) -> Ty {
        match self {
            KernelReturn::Int => Ty::Int,
            KernelReturn::Bool => Ty::Bool,
            KernelReturn::Str => Ty::Str,
        }
    }
}

/// Rust bridge. The `&str` is the Rust method; the variant is the
/// shape around it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RustSpell {
    /// `{recv}.{method}()`
    Call0(&'static str),
    /// `{recv}.{method}(&*({arg}))`. Pattern methods take `&str`, not
    /// `String`, so a computed argument (`ap.start_with?(pp[0, n])`)
    /// needs the reborrow. On a literal `&str`, `&*` is a no-op.
    Call1Borrow(&'static str),
    /// `{recv}.chars().count() as i64`. The cast is non-primary; the
    /// decide pass stamps `NEEDS_PARENS` when this send is a chained
    /// receiver.
    CharCount,
    /// `{recv}.parse::<i64>().unwrap_or(0)`. Ruby `to_i` parses a
    /// leading integer and answers 0 when the rest is not numeric.
    /// `parse::<i64>` requires the whole string, so the failure path
    /// is `unwrap_or(0)`. `&str` and `String` both implement it, which
    /// is why the Symbol arm can share this row.
    ParseI64,
    /// `{recv}.trim().to_string()`. `trim` borrows; Ruby `strip`
    /// returns a new String.
    TrimOwned,
}

/// TypeScript bridge for a receiver the typer has settled as `String`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TsSpell {
    /// `{recv}.length === 0`
    LengthEqZero,
    /// `{recv}.{member}`
    Member(&'static str),
    /// `{recv}.{method}()`
    Method0(&'static str),
    /// `{recv}.{method}({arg})`
    Method1(&'static str),
    /// `Number({recv})`
    Number,
}

/// One kernel method.
pub struct StringKernel {
    pub name: &'static str,
    /// Arity the Rust and TypeScript bridges accept.
    pub arity: usize,
    pub returns: KernelReturn,
    pub rust: RustSpell,
    pub typescript: TsSpell,
    /// Crystal method name. Equal to `name` when Crystal spells it
    /// the same way. Applied by name, not by `arity`.
    pub crystal: &'static str,
}

/// The kernel Rust and TypeScript already mirrored.
pub const STRING_KERNEL: &[StringKernel] = &[
    StringKernel {
        name: "empty?",
        arity: 0,
        returns: KernelReturn::Bool,
        rust: RustSpell::Call0("is_empty"),
        typescript: TsSpell::LengthEqZero,
        crystal: "empty?",
    },
    StringKernel {
        name: "length",
        arity: 0,
        returns: KernelReturn::Int,
        rust: RustSpell::CharCount,
        typescript: TsSpell::Member("length"),
        crystal: "size",
    },
    StringKernel {
        name: "size",
        arity: 0,
        returns: KernelReturn::Int,
        rust: RustSpell::CharCount,
        typescript: TsSpell::Member("length"),
        crystal: "size",
    },
    StringKernel {
        name: "upcase",
        arity: 0,
        returns: KernelReturn::Str,
        rust: RustSpell::Call0("to_uppercase"),
        typescript: TsSpell::Method0("toUpperCase"),
        crystal: "upcase",
    },
    StringKernel {
        name: "downcase",
        arity: 0,
        returns: KernelReturn::Str,
        rust: RustSpell::Call0("to_lowercase"),
        typescript: TsSpell::Method0("toLowerCase"),
        crystal: "downcase",
    },
    StringKernel {
        name: "strip",
        arity: 0,
        returns: KernelReturn::Str,
        rust: RustSpell::TrimOwned,
        typescript: TsSpell::Method0("trim"),
        crystal: "strip",
    },
    StringKernel {
        name: "start_with?",
        arity: 1,
        returns: KernelReturn::Bool,
        rust: RustSpell::Call1Borrow("starts_with"),
        typescript: TsSpell::Method1("startsWith"),
        crystal: "starts_with?",
    },
    StringKernel {
        name: "end_with?",
        arity: 1,
        returns: KernelReturn::Bool,
        rust: RustSpell::Call1Borrow("ends_with"),
        typescript: TsSpell::Method1("endsWith"),
        crystal: "ends_with?",
    },
    StringKernel {
        name: "include?",
        arity: 1,
        returns: KernelReturn::Bool,
        rust: RustSpell::Call1Borrow("contains"),
        typescript: TsSpell::Method1("includes"),
        crystal: "includes?",
    },
    StringKernel {
        name: "to_i",
        arity: 0,
        returns: KernelReturn::Int,
        rust: RustSpell::ParseI64,
        typescript: TsSpell::Number,
        crystal: "to_i",
    },
];

/// The row for `name` at any arity. Analysis uses this.
pub fn by_name(name: &str) -> Option<&'static StringKernel> {
    STRING_KERNEL.iter().find(|entry| entry.name == name)
}

/// The row whose Rust and TypeScript bridge accepts `arity`.
pub fn lookup(name: &str, arity: usize) -> Option<&'static StringKernel> {
    STRING_KERNEL
        .iter()
        .find(|entry| entry.name == name && entry.arity == arity)
}

/// Return type for a kernel name, ignoring arity.
pub fn return_ty(name: &str) -> Option<Ty> {
    by_name(name).map(|entry| entry.returns.ty())
}

/// Crystal spelling of a kernel name, at every arity and for every
/// receiver. `None` when `name` is not in the kernel. Name-keyed on
/// purpose: the old Crystal rename did not check arity.
pub fn crystal_name(name: &str) -> Option<&'static str> {
    by_name(name).map(|entry| entry.crystal)
}

/// TypeScript method spelling for an untyped receiver. Only `strip`
/// and `include?` were rewritten before the receiver type was known
/// (`trim`, `includes`). Any other spelling of those two rows is a
/// broken table.
pub fn untyped_typescript_method(name: &str, arity: usize) -> Option<&'static str> {
    let entry = lookup(name, arity)?;
    match (entry.name, entry.typescript) {
        ("strip" | "include?", TsSpell::Method0(spelled) | TsSpell::Method1(spelled)) => {
            Some(spelled)
        }
        ("strip" | "include?", _) => {
            panic!("untyped TypeScript fallback requires a method spelling");
        }
        _ => None,
    }
}

/// Rust source for one bridged call. `args` is the already-emitted
/// argument list; [`RustSpell::Call1Borrow`] reads the first.
pub fn render_rust(entry: &StringKernel, recv: &str, args: &[String]) -> String {
    match entry.rust {
        RustSpell::Call0(method) => format!("{recv}.{method}()"),
        RustSpell::Call1Borrow(method) => format!("{recv}.{method}(&*({}))", args[0]),
        RustSpell::CharCount => format!("{recv}.chars().count() as i64"),
        RustSpell::ParseI64 => format!("{recv}.parse::<i64>().unwrap_or(0)"),
        RustSpell::TrimOwned => format!("{recv}.trim().to_string()"),
    }
}
