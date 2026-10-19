// Libtest omits the crate root from test names. A combined suite adds the
// source file's module, while a standalone suite has no such prefix.
macro_rules! name {
    ($test:literal) => {
        concat!(module_path!(), "::", $test).split_once("::").unwrap().1
    };
}

pub(crate) use name;
