//! A file assembled from runtime sources keeps only its leading magic
//! comment: one past the first token is ignored by Ruby with a warning,
//! and a large app's a warnings-are-errors policy turns that warning into a boot failure.
use roundhouse::project::drop_late_magic_comments;

#[test]
fn a_magic_comment_after_code_is_dropped_and_a_leading_one_kept() {
    let src = "# frozen_string_literal: true\n# helper\nrequire_relative \"a\"\n\n# frozen_string_literal: true\n# next part\nmodule M; end\n";
    let out = drop_late_magic_comments(src);
    assert_eq!(out, "# frozen_string_literal: true\n# helper\nrequire_relative \"a\"\n\n# next part\nmodule M; end\n");
}

#[test]
fn a_file_without_late_magic_comments_is_untouched() {
    let src = "# frozen_string_literal: true\nx = \"a\"\n  # frozen_string_literal: true is only text here\n";
    assert!(matches!(drop_late_magic_comments(src), std::borrow::Cow::Borrowed(_)));
}

#[test]
fn the_emitted_files_ruby_parses_warn_free() {
    let src = "require_relative \"active_support_i18n\"\n\n# frozen_string_literal: true\nX = \"x\"\n";
    let out = drop_late_magic_comments(src);
    let dir = std::env::temp_dir().join(format!("rh-late-magic-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("f.rb");
    std::fs::write(&file, out.as_bytes()).unwrap();
    let res = std::process::Command::new("ruby").args(["-W2", "-c"]).arg(&file).output().expect("ruby");
    let stderr = String::from_utf8_lossy(&res.stderr);
    assert!(!stderr.contains("is ignored after any tokens"), "{stderr}");
    std::fs::write(&file, src).unwrap();
    let res = std::process::Command::new("ruby").args(["-W2", "-c"]).arg(&file).output().expect("ruby");
    assert!(String::from_utf8_lossy(&res.stderr).contains("is ignored after any tokens"), "the unfixed source should warn");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_late_shareable_constant_value_is_kept() {
    // Ruby honors this one anywhere in a file, for the constants after it.
    let src = "require \"a\"\n# shareable_constant_value: literal\nX = [\"x\"]\n";
    assert!(matches!(drop_late_magic_comments(src), std::borrow::Cow::Borrowed(_)));
}

#[test]
fn the_same_text_inside_a_heredoc_is_string_data() {
    let src = "X = <<~RUBY\n# frozen_string_literal: true\nputs 1\nRUBY\n# frozen_string_literal: true\nY = 1\n";
    let out = drop_late_magic_comments(src);
    assert_eq!(out, "X = <<~RUBY\n# frozen_string_literal: true\nputs 1\nRUBY\nY = 1\n");
}
