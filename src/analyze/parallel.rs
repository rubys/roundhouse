//! Owned-item work within an existing phase; the caller replays effects in order.
use std::sync::Mutex;

pub(super) fn map<T: Send, R: Send>(items: &mut [T], f: impl Fn(&mut T) -> R + Sync) -> Vec<R> {
    if crate::par::workers_for(items.len()) == 1 {
        return items.iter_mut().map(f).collect();
    }
    // Each job hands its exclusive item borrow to exactly one worker.
    let jobs: Vec<_> = items.iter_mut().map(Mutex::new).collect();
    let inflections = crate::naming::app_inflections();
    let plans = crate::lower::i18n_translate::plan_cache_snapshot();
    crate::par::map_init(
        &jobs,
        || {
            crate::naming::install_app_inflections(inflections.clone());
            crate::lower::i18n_translate::install_plan_cache(plans.clone())
        },
        |_, job| f(&mut **job.lock().unwrap()),
    )
}
