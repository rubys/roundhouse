//! ActionDispatch::Flash — per-app flash store with typed
//! `notice`/`alert` fields plus HWIA-shape shim methods.
//!
//! Hand-written for rust2 Phase 3 (sibling of
//! `runtime/ruby/action_dispatch/flash.rb` and
//! `runtime/typescript/`'s transpiled `flash.ts`). The transpile
//! pipeline produces broken Rust for this file's HWIA shim methods
//! (Index trait, self-indexing); hand-writing avoids fighting those
//! emit bugs. The struct surface matches the typed-targets contract
//! (`is_flash_name` in `view_to_library/extra_params.rs` declares the
//! closed `notice`/`alert` field set).

use std::collections::HashMap;

#[derive(Debug, Default, Clone)]
pub struct Flash {
    pub notice: Option<String>,
    pub alert: Option<String>,
}

impl Flash {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_persisted(other: Option<&HashMap<String, String>>) -> Self {
        let mut flash = Self::default();
        if let Some(map) = other {
            if let Some(v) = map.get("notice") {
                flash.notice = Some(v.clone());
            }
            if let Some(v) = map.get("alert") {
                flash.alert = Some(v.clone());
            }
        }
        flash
    }

    pub fn get(&self, key: &str) -> Option<String> {
        match key {
            "notice" => self.notice.clone(),
            "alert" => self.alert.clone(),
            _ => None,
        }
    }

    pub fn set(&mut self, key: &str, value: Option<String>) {
        match key {
            "notice" => self.notice = value,
            "alert" => self.alert = value,
            _ => {}
        }
    }

    pub fn fetch(&self, key: &str, default: Option<String>) -> Option<String> {
        self.get(key).or(default)
    }

    pub fn key(&self, key: &str) -> bool {
        self.get(key).is_some()
    }

    pub fn has_key(&self, key: &str) -> bool {
        self.key(key)
    }

    pub fn include(&self, key: &str) -> bool {
        self.key(key)
    }

    pub fn delete(&mut self, key: &str) -> Option<String> {
        match key {
            "notice" => self.notice.take(),
            "alert" => self.alert.take(),
            _ => None,
        }
    }

    pub fn len(&self) -> usize {
        let mut n = 0;
        if self.notice.is_some() {
            n += 1;
        }
        if self.alert.is_some() {
            n += 1;
        }
        n
    }

    pub fn is_empty(&self) -> bool {
        self.notice.is_none() && self.alert.is_none()
    }

    pub fn keys(&self) -> Vec<&'static str> {
        let mut out = Vec::new();
        if self.notice.is_some() {
            out.push("notice");
        }
        if self.alert.is_some() {
            out.push("alert");
        }
        out
    }

    pub fn to_h(&self) -> HashMap<String, String> {
        let mut out = HashMap::new();
        if let Some(v) = &self.notice {
            out.insert("notice".to_string(), v.clone());
        }
        if let Some(v) = &self.alert {
            out.insert("alert".to_string(), v.clone());
        }
        out
    }
}
