//! Crystal library-shape emitter — under reconstruction.

use crate::dialect::{LibraryClass, MethodDef};

pub fn emit_module(_methods: &[MethodDef]) -> Result<String, String> {
    Ok(String::new())
}

pub fn emit_library_class(_class: &LibraryClass) -> Result<String, String> {
    Ok(String::new())
}
