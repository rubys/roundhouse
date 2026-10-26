use std::cell::RefCell;
use std::sync::OnceLock;

thread_local! {
    // Not one static: axum runs requests on several tokio workers at once, and a request's dispatch stays on one
    static LOCALE: RefCell<String> = const { RefCell::new(String::new()) };
}

static TABLE: OnceLock<String> = OnceLock::new();

pub struct I18nHost;

impl I18nHost {
    pub fn reset_request() {
        LOCALE.with(|l| l.borrow_mut().clear());
    }

    pub fn locale_name() -> String {
        LOCALE.with(|l| l.borrow().clone())
    }

    pub fn set_locale_name(name: String) -> String {
        LOCALE.with(|l| *l.borrow_mut() = name.clone());
        name
    }

    pub fn env_name() -> String {
        std::env::var("RAILS_ENV").or_else(|_| std::env::var("RACK_ENV")).unwrap_or_else(|_| "development".to_string())
    }

    // Not the working directory alone: a test runner may start below the app root
    pub fn table_text() -> String {
        TABLE
            .get_or_init(|| {
                let mut dir = std::env::current_dir().expect("working directory");
                loop {
                    if let Ok(text) = std::fs::read_to_string(dir.join("config").join("i18n_translations")) {
                        return text;
                    }
                    if !dir.pop() {
                        panic!("config/i18n_translations is in no directory above the working directory");
                    }
                }
            })
            .clone()
    }
}
