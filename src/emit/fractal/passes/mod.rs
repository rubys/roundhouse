//! The registered passes, one module each. [`super::pass::registered`]
//! lists them in the order a build runs them.

pub mod delegate_definitions;
pub mod inline_delegate;

pub use inline_delegate::InlineDelegate;
