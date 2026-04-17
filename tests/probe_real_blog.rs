//! Diagnostic probe: try to ingest the real-blog fixture and report the first
//! gap. This test is marked #[ignore] so it doesn't fail CI; run with
//! `cargo test --test probe_real_blog -- --ignored --nocapture`.

use std::path::Path;

use roundhouse::ingest::ingest_app;

#[test]
#[ignore]
fn probe_real_blog_ingest() {
    match ingest_app(Path::new("fixtures/real-blog")) {
        Ok(app) => {
            println!(
                "ingest ok: {} models, {} controllers, {} views, {} routes",
                app.models.len(),
                app.controllers.len(),
                app.views.len(),
                app.routes.routes.len()
            );
        }
        Err(e) => {
            panic!("ingest failed: {e}");
        }
    }
}
