use std::{fs, path::PathBuf, process::Command};

const HTTP_RUNTIME: &str = include_str!("../runtime/rust/http.rs");
const SERVER_RUNTIME: &str = include_str!("../runtime/rust/server.rs");

#[test]
#[ignore = "compiles the production Rust server stack in an isolated Cargo crate"]
fn production_server_cookie_lifecycle() {
    let root: PathBuf = std::env::temp_dir().join("roundhouse-rust-server-cookie-lifecycle");
    if root.exists() {
        fs::remove_dir_all(&root).expect("remove prior scratch crate");
    }
    fs::create_dir_all(root.join("src")).expect("create scratch source directory");
    fs::write(
        root.join("Cargo.toml"),
        r#"[package]
name = "roundhouse-rust-server-cookie-lifecycle"
version = "0.1.0"
edition = "2024"

[dependencies]
axum = "0.8"
tower-http = { version = "0.6", features = ["fs"] }
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
tower = { version = "0.5", features = ["util"] }
serde_json = "1"
axum-test = "18"
"#,
    )
    .expect("write scratch manifest");

    let lib = format!(
        r#"
pub mod cable {{
    pub async fn cable_handler() -> &'static str {{ "cable" }}
}}
pub mod db {{ pub fn open_production_db(_: &str, _: &str) {{}} }}
pub mod flash {{
    use std::collections::HashMap;
    pub struct Flash;
    impl Flash {{ pub fn from_persisted(_: Option<&HashMap<String, String>>) -> Self {{ Self }} }}
}}
pub mod view_helpers {{
    pub fn reset_render_state() {{}}
    pub fn set_yield(_: &str) {{}}
}}
pub mod http;
pub mod server;

#[cfg(test)]
mod tests {{
    use axum::{{body::Body, extract::Request, http::{{header, StatusCode}}, middleware, response::Response, routing::get, Router}};
    use tower::ServiceExt;
    use std::sync::atomic::{{AtomicUsize, Ordering}};
    use crate::http::current_request_context;

    static LAYOUT_CALLS: AtomicUsize = AtomicUsize::new(0);

    async fn handler() -> Response {{
        let context = current_request_context();
        let path = context.uri.path().to_owned();
        let inbound = context.request_cookie("sentinel").unwrap_or_default();
        context.queue_set_cookie(&format!("handler={{path}}; Path=/")).unwrap();
        context.queue_set_cookie(&format!("handler-inbound={{inbound}}; Path=/")).unwrap();
        if path == "/redirect" {{
            Response::builder().status(StatusCode::SEE_OTHER).header(header::LOCATION, "/next").body(Body::empty()).unwrap()
        }} else if path == "/json" {{
            Response::builder().header(header::CONTENT_TYPE, "application/json").body(Body::from("{{}}")).unwrap()
        }} else {{
            Response::builder().header(header::CONTENT_TYPE, "text/html").header(header::SET_COOKIE, "existing=1; Path=/").body(Body::from("view")).unwrap()
        }}
    }}

    fn layout() -> String {{
        LAYOUT_CALLS.fetch_add(1, Ordering::SeqCst);
        let context = current_request_context();
        let path = context.uri.path().to_owned();
        let inbound = context.request_cookie("sentinel").unwrap_or_default();
        context.queue_set_cookie(&format!("layout={{path}}; Path=/")).unwrap();
        context.queue_set_cookie(&format!("layout-inbound={{inbound}}; Path=/")).unwrap();
        format!("layout:{{path}}:{{inbound}}")
    }}

    async fn request(app: Router, path: &'static str, inbound: &'static str) -> Response {{
        app.oneshot(Request::builder().uri(path).header(header::COOKIE, format!("sentinel={{inbound}}")).body(Body::empty()).unwrap()).await.unwrap()
    }}

    fn cookies(response: &Response) -> Vec<String> {{
        response.headers().get_all(header::SET_COOKIE).iter().map(|v| v.to_str().unwrap().to_owned()).collect()
    }}

    #[tokio::test]
    async fn production_router_drains_cookies_after_layout_and_redirect() {{
        LAYOUT_CALLS.store(0, Ordering::SeqCst);
        let routes = Router::new()
            .route("/html", get(handler))
            .route("/redirect", get(handler))
            .route("/json", get(handler))
            .layer(middleware::from_fn(crate::http::request_context_middleware));
        let app = crate::server::production_router(routes, Some(layout));

        let html = request(app.clone(), "/html", "html-client").await;
        assert_eq!(html.status(), StatusCode::OK);
        let html_cookies = cookies(&html);
        assert!(html_cookies.iter().any(|v| v == "existing=1; Path=/"), "existing cookie missing: {{html_cookies:?}}");
        assert!(html_cookies.iter().any(|v| v == "handler=/html; Path=/"), "handler cookie missing: {{html_cookies:?}}");
        assert!(html_cookies.iter().any(|v| v == "layout=/html; Path=/"), "layout cookie missing: {{html_cookies:?}}");
        assert!(html_cookies.iter().any(|v| v == "handler-inbound=html-client; Path=/"), "handler did not read incoming cookie: {{html_cookies:?}}");
        assert!(html_cookies.iter().any(|v| v == "layout-inbound=html-client; Path=/"), "layout did not read the same incoming cookie: {{html_cookies:?}}");
        assert_eq!(axum::body::to_bytes(html.into_body(), usize::MAX).await.unwrap(), "layout:/html:html-client");

        let before_redirect = LAYOUT_CALLS.load(Ordering::SeqCst);
        let redirect = request(app.clone(), "/redirect", "redirect-client").await;
        assert_eq!(redirect.status(), StatusCode::SEE_OTHER);
        assert_eq!(LAYOUT_CALLS.load(Ordering::SeqCst), before_redirect, "redirect called layout");
        assert!(cookies(&redirect).iter().any(|v| v == "handler=/redirect; Path=/"), "redirect handler cookie was not drained");
        assert!(cookies(&redirect).iter().any(|v| v == "handler-inbound=redirect-client; Path=/"), "redirect handler did not read incoming cookie");
        assert!(!cookies(&redirect).iter().any(|v| v.starts_with("layout=")), "redirect received layout cookie");

        let json = request(app.clone(), "/json", "json-client").await;
        assert_eq!(json.status(), StatusCode::OK);
        assert_eq!(LAYOUT_CALLS.load(Ordering::SeqCst), before_redirect, "non-HTML response called layout");
        assert!(cookies(&json).iter().any(|v| v == "handler=/json; Path=/"), "non-HTML handler cookie was not drained");
        assert!(cookies(&json).iter().any(|v| v == "handler-inbound=json-client; Path=/"), "non-HTML handler did not read incoming cookie");

        let routes = Router::new().route("/a", get(handler)).route("/b", get(handler))
            .layer(middleware::from_fn(crate::http::request_context_middleware));
        let app = crate::server::production_router(routes, None);
        let (a, b) = tokio::join!(request(app.clone(), "/a", "client-a"), request(app, "/b", "client-b"));
        assert!(cookies(&a).iter().any(|v| v == "handler=/a; Path=/"), "request A cookies: {{:?}}", cookies(&a));
        assert!(cookies(&a).iter().any(|v| v == "layout=/a; Path=/"), "request A cookies: {{:?}}", cookies(&a));
        assert!(cookies(&a).iter().any(|v| v == "handler-inbound=client-a; Path=/"), "request A incoming cookie: {{:?}}", cookies(&a));
        assert!(cookies(&a).iter().any(|v| v == "layout-inbound=client-a; Path=/"), "request A layout context: {{:?}}", cookies(&a));
        assert!(cookies(&b).iter().any(|v| v == "handler=/b; Path=/"), "request B cookies: {{:?}}", cookies(&b));
        assert!(cookies(&b).iter().any(|v| v == "layout=/b; Path=/"), "request B cookies: {{:?}}", cookies(&b));
        assert!(cookies(&b).iter().any(|v| v == "handler-inbound=client-b; Path=/"), "request B incoming cookie: {{:?}}", cookies(&b));
        assert!(cookies(&b).iter().any(|v| v == "layout-inbound=client-b; Path=/"), "request B layout context: {{:?}}", cookies(&b));
        assert!(!cookies(&a).iter().any(|v| v.contains("/b")), "request A leaked B cookie: {{:?}}", cookies(&a));
        assert!(!cookies(&b).iter().any(|v| v.contains("/a")), "request B leaked A cookie: {{:?}}", cookies(&b));
    }}
}}
"#
    );
    fs::write(root.join("src/lib.rs"), lib).expect("write scratch crate root");
    fs::write(root.join("src/http.rs"), HTTP_RUNTIME).expect("copy emitted HTTP runtime");
    fs::write(root.join("src/server.rs"), SERVER_RUNTIME).expect("copy production server runtime");

    let output = Command::new("cargo")
        .args(["test", "--lib", "production_router", "--", "--nocapture"])
        .current_dir(&root)
        .env(
            "CARGO_TARGET_DIR",
            std::env::temp_dir().join("roundhouse-rust-server-cookie-target"),
        )
        .env("CARGO_PROFILE_DEV_DEBUG", "0")
        .env("CARGO_PROFILE_TEST_DEBUG", "0")
        .output()
        .expect("run isolated production server tests");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "isolated production server tests failed:\n{stdout}\n{stderr}"
    );
    assert!(
        stdout.contains("1 passed; 0 failed"),
        "production lifecycle test did not pass:\n{stdout}"
    );
}
