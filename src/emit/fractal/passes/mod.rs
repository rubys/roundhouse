//! The registered passes, one module each. [`super::pass::registered`]
//! lists them in the order a build runs them.

pub mod delegate_definitions;
pub mod duration_definitions;
pub mod inline_delegate;
pub mod lower_duration;

pub use inline_delegate::InlineDelegate;
pub use lower_duration::LowerDuration;
