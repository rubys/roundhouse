//! `mix.exs` emission.

use std::path::PathBuf;

use super::super::EmittedFile;

/// Minimal mix.exs. `elixirc_paths` uses a wildcard filter that
/// excludes controllers and the router — their bodies reference
/// runtime that doesn't exist yet (redirect_to, Post.all, etc.), so
/// including them blocks `mix compile`. When Phase 3 wires the
/// runtime, the filter relaxes.
pub(super) fn emit_mix_exs() -> EmittedFile {
    let content = "\
defmodule App.MixProject do
  use Mix.Project

  def project do
    [
      app: :app,
      version: \"0.1.0\",
      elixir: \"~> 1.18\",
      elixirc_paths: elixirc_paths(Mix.env()),
      start_permanent: Mix.env() == :prod,
      deps: deps()
    ]
  end

  def application do
    [extra_applications: [:logger]]
  end

  defp deps do
    [
      {:exqlite, \"~> 0.30\"},
      {:plug_cowboy, \"~> 2.7\"},
      {:jason, \"~> 1.4\"},
      {:websock_adapter, \"~> 0.5\"}
    ]
  end

  # Phase 4c: controllers now lower through Roundhouse.Http stubs and
  # compile. Test env additionally includes test/support/ so fixtures
  # are compiled alongside the app.
  defp elixirc_paths(:test) do
    Path.wildcard(\"lib/**/*.ex\") ++ Path.wildcard(\"test/support/**/*.ex\")
  end

  defp elixirc_paths(_) do
    Path.wildcard(\"lib/**/*.ex\")
  end
end
";
    EmittedFile {
        path: PathBuf::from("mix.exs"),
        content: content.to_string(),
    }
}
