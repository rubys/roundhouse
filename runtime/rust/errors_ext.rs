//! Per-target error/raise primitives the transpiled framework runtime
//! reaches via bare-token emit.
//!
//! `raise(KIND, payload)` is the Ruby-shape `raise Klass, "..."`
//! emitted by the transpile pipeline. Rust has no `raise` keyword and
//! the error classes do not transpile cleanly yet — Display + Error
//! synthesis for `class < StandardError` is a separate emit feature.
//!
//! Phase 3 stub: a single `FrameworkError` enum, module-level
//! consts the transpile's bare tokens map to (via the `imports` field
//! in `RUST_RUNTIME`), and a `raise` function generic over the payload
//! type. The function returns `!` so call sites in non-Unit-returning
//! methods compile (`!` coerces to any type).
//!
//! Payload is intentionally `T` (no Debug/Display bound) because the
//! emitted code passes structs (`self`, `Base` instances) that don't
//! derive Debug yet. Lost message content is acceptable for the
//! contract-marker raise calls (table_name, instantiate, etc.) — the
//! panic still surfaces the kind, which is enough to diagnose a
//! missing subclass override.

#[derive(Debug, Clone, Copy)]
pub enum FrameworkError {
    NotImplemented,
    RecordNotFound,
    RecordInvalid,
    Argument,
    Runtime,
    Index,
}

#[allow(non_upper_case_globals)]
pub const NotImplementedError: FrameworkError = FrameworkError::NotImplemented;
#[allow(non_upper_case_globals)]
pub const RecordNotFound: FrameworkError = FrameworkError::RecordNotFound;
#[allow(non_upper_case_globals)]
pub const RecordInvalid: FrameworkError = FrameworkError::RecordInvalid;
#[allow(non_upper_case_globals)]
pub const ArgumentError: FrameworkError = FrameworkError::Argument;
#[allow(non_upper_case_globals)]
pub const RuntimeError: FrameworkError = FrameworkError::Runtime;
#[allow(non_upper_case_globals)]
pub const IndexError: FrameworkError = FrameworkError::Index;

/// Ruby-shape `raise Klass, payload`. Panics with the framework
/// error kind; payload is accepted but discarded.
///
/// Returns `!` so the caller can use it as the body of any-typed
/// method (`fn table_name() -> String { raise(...) }` compiles).
pub fn raise<T>(kind: FrameworkError, _payload: T) -> ! {
    panic!("FrameworkError::{:?}", kind);
}

impl FrameworkError {
    fn ruby_name(self) -> &'static str {
        match self {
            FrameworkError::NotImplemented => "NotImplementedError",
            FrameworkError::RecordNotFound => "ActiveRecord::RecordNotFound",
            FrameworkError::RecordInvalid => "ActiveRecord::RecordInvalid",
            FrameworkError::Argument => "ArgumentError",
            FrameworkError::Runtime => "RuntimeError",
            FrameworkError::Index => "IndexError",
        }
    }

    fn named(name: &str) -> Option<FrameworkError> {
        [
            FrameworkError::NotImplemented,
            FrameworkError::RecordNotFound,
            FrameworkError::RecordInvalid,
            FrameworkError::Argument,
            FrameworkError::Runtime,
            FrameworkError::Index,
        ]
        .into_iter()
        .find(|k| format!("FrameworkError::{k:?}") == name)
    }
}

/// What a `rescue` clause binds: the class and message of the panic it caught.
#[derive(Debug, Clone)]
pub struct Rescued {
    pub class: &'static str,
    pub message: String,
}

impl Rescued {
    /// The exception a caught panic carries: a `Raised`, a framework kind's
    /// `FrameworkError::Kind` text, or any other text as Ruby's RuntimeError.
    pub fn from_panic(payload: &(dyn std::any::Any + Send)) -> Rescued {
        if let Some(r) = payload.downcast_ref::<Raised>() {
            return Rescued { class: r.class, message: r.message.clone() };
        }
        let text = payload
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| payload.downcast_ref::<&str>().map(|s| s.to_string()))
            .unwrap_or_default();
        match FrameworkError::named(&text) {
            Some(kind) => Rescued { class: kind.ruby_name(), message: kind.ruby_name().to_string() },
            None => Rescued { class: "RuntimeError", message: text },
        }
    }

    /// Ruby's `is_a?` over the classes the runtime raises; an app's own
    /// class is known only by name, and answers to StandardError as an
    /// application error does.
    pub fn is_a(&self, class: &str) -> bool {
        let mut cur = self.class;
        loop {
            if cur == class {
                return true;
            }
            cur = match cur {
                "ActiveRecord::RecordNotFound" | "ActiveRecord::RecordInvalid" => "ActiveRecord::ActiveRecordError",
                "KeyError" => "IndexError",
                "NoMethodError" => "NameError",
                "NotImplementedError" => "ScriptError",
                "ScriptError" | "StandardError" => "Exception",
                "Exception" => return false,
                _ => "StandardError",
            };
        }
    }

    /// Rust raises carry no cause.
    pub fn cause(&self) -> Option<Rescued> {
        None
    }

    /// `raise e` of a rescued exception.
    pub fn raise(&self) -> ! {
        raise_class(self.class, self.message.clone())
    }
}

/// A raised Ruby exception: its class and message. The panic payload of
/// `raise_msg` / `raise_class`, so a caller can `catch_unwind` and read
/// what Ruby's `rescue => e` would (`e.class.name`, `e.message`).
#[derive(Debug, Clone)]
pub struct Raised {
    pub class: &'static str,
    pub message: String,
}

impl std::fmt::Display for Raised {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({})", self.message, self.class)
    }
}

/// `raise Klass, "message"` for a framework error kind.
pub fn raise_msg(kind: FrameworkError, message: String) -> ! {
    raise_class(kind.ruby_name(), message)
}

/// `raise Klass, "message"` for any other Ruby exception class, named as Ruby names it.
pub fn raise_class(class: &'static str, message: String) -> ! {
    std::panic::panic_any(Raised { class, message })
}

/// Placeholder for Ruby's `self.name` (class name) inside emitted
/// class methods. The transpile lowers bare `name` calls to a free
/// function reference; until the emit-side rewrites these to
/// per-class string literals or `std::any::type_name::<Self>()`,
/// this stub gives the references something to resolve to.
pub fn name() -> &'static str {
    "Base"
}

// Thread-local validation-error buffer the AR shim's `save` path
// reads through. Every emitted model's `validate` body pushes
// user-visible messages here — rust2 emit rewrites the lowered
// `self.errors() << msg` shape to `validation_errors_push(msg)` so
// the messages survive the `errors() returns owned Vec<String>` shim
// that would otherwise drop them. `save` clears the buffer before
// running validate and reads `validation_errors_is_empty()` to
// decide whether to persist.
//
// Per-thread keeps tests independent (cargo test runs tests on
// separate threads); a global Mutex would cross-contaminate.
thread_local! {
    static VALIDATION_ERRORS: std::cell::RefCell<Vec<String>> =
        std::cell::RefCell::new(Vec::new());
    // The (attribute, kind, message, full message, symbolic, optionless) each message above records, index for index.
    // Tuples, not `ErrorDetail`: this file also compiles alone, without the transpiled base.
    static VALIDATION_ERROR_DETAILS: std::cell::RefCell<Vec<(String, String, String, String, bool, bool)>> =
        std::cell::RefCell::new(Vec::new());
}

pub fn validation_errors_clear() {
    VALIDATION_ERRORS.with(|c| c.borrow_mut().clear());
    VALIDATION_ERROR_DETAILS.with(|c| c.borrow_mut().clear());
}

pub fn validation_error_details_push(attribute: String, kind: String, message: String, full_message: String, symbolic: bool, optionless: bool) {
    VALIDATION_ERROR_DETAILS.with(|c| c.borrow_mut().push((attribute, kind, message, full_message, symbolic, optionless)));
}

pub fn validation_error_details_snapshot() -> Vec<(String, String, String, String, bool, bool)> {
    VALIDATION_ERROR_DETAILS.with(|c| c.borrow().clone())
}

pub fn validation_errors_push(msg: String) {
    VALIDATION_ERRORS.with(|c| c.borrow_mut().push(msg));
}

pub fn validation_errors_is_empty() -> bool {
    VALIDATION_ERRORS.with(|c| c.borrow().is_empty())
}

pub fn validation_errors_snapshot() -> Vec<String> {
    VALIDATION_ERRORS.with(|c| c.borrow().clone())
}
