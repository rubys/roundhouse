//! End-to-end proof that template helper calls reach the shared Ruby
//! NumberHelper implementation in emitted CRuby and native Spinel apps.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;
#[path = "support/native_http.rs"]
mod native_http;

use roundhouse::project::BuildTarget;

const INDEX_VIEW: &str = "app/views/articles/index.html.erb";
const INDEX_HEADING: &str = "<h1 class=\"font-bold text-4xl\">Articles</h1>";
const CONTROLLER_TEST: &str = "test/controllers/articles_controller_test.rb";
const INDEX_ASSERTION: &str = "assert_select \"h1\", \"Articles\"\n";
const NUMBER_HELPER_TEMPLATE: &str = r#"
<p id="nh-currency"><%= number_to_currency(1234.5) %></p>
<p id="nh-human"><%= number_to_human(1234567) %></p>
<p id="nh-size"><%= number_to_human_size(1234567) %></p>
<p id="nh-size-large"><%= number_to_human_size("123456789012345678901234567890") %></p>
<p id="nh-percentage"><%= number_to_percentage(99.999, precision: 0) %></p>
<p id="nh-phone"><%= number_to_phone("1235551234", country_code: 1) %></p>
<p id="nh-delimited"><%= number_with_delimiter("1234567.89", delimiter: ".", separator: ",") %></p>
<p id="nh-precision"><%= number_with_precision("-12.345", precision: 2, round_mode: :down) %></p>
<p id="nh-html"><%= number_to_currency(1, unit: "<b>") %></p>
<p id="nh-invalid-html"><%= number_to_currency("<script>") %></p>
<p id="nh-invalid-currency"><%= number_to_currency("-abc") %></p>
"#;

const EXPECTED_HTML: &[&str] = &[
    "id=\"nh-currency\">$1,234.50</p>",
    "id=\"nh-human\">1.23 Million</p>",
    "id=\"nh-size\">1.18 MB</p>",
    "id=\"nh-size-large\">105000000 ZB</p>",
    "id=\"nh-percentage\">100%</p>",
    "id=\"nh-phone\">+1-123-555-1234</p>",
    "id=\"nh-delimited\">1.234.567,89</p>",
    "id=\"nh-precision\">-12.34</p>",
    "id=\"nh-html\">&lt;b&gt;1.00</p>",
    "id=\"nh-invalid-html\">$&lt;script&gt;</p>",
    "id=\"nh-invalid-currency\">-$abc</p>",
];

fn overlay() -> emit_and_run::Overlay {
    emit_and_run::real_blog().edit(
        INDEX_VIEW,
        INDEX_HEADING,
        &format!("{INDEX_HEADING}\n{NUMBER_HELPER_TEMPLATE}"),
    )
}

fn assert_number_helper_html(body: &str) {
    for expected in EXPECTED_HTML {
        assert!(
            body.contains(expected),
            "missing {expected:?} in rendered HTML:\n{body}"
        );
    }
}

#[test]
fn all_action_view_number_wrappers_render_through_emitted_cruby() {
    overlay()
        .edit(
            CONTROLLER_TEST,
            INDEX_ASSERTION,
            &format!(
                "{INDEX_ASSERTION}{}",
                EXPECTED_HTML
                    .iter()
                    .map(|expected| format!("    assert_includes response.body, {expected:?}\n"))
                    .collect::<String>()
            ),
        )
        .run_test(CONTROLLER_TEST)
        .assert_passes();
}

#[test]
#[ignore = "requires the native Spinel toolchain"]
fn all_action_view_number_wrappers_render_through_native_spinel() {
    let (emitted, errors) = overlay().emit(BuildTarget::Spinel);
    assert!(
        errors.is_empty(),
        "analysis or emission reported errors:\n{}",
        errors.join("\n")
    );
    native_http::build(&emitted);
    let server = native_http::Server::start(&emitted);
    let response = server.get("/");
    assert_eq!(response.status, 200, "GET / failed:\n{}", server.log());
    assert_number_helper_html(&response.body);
}
