defmodule I18nHost do
  # Not a module attribute: Plug serves each request in a process of its own, and the locale is that request's
  def reset_request() do
    Process.delete(:roundhouse_i18n_locale)
    nil
  end

  def locale_name() do
    Process.get(:roundhouse_i18n_locale, "")
  end

  def set_locale_name(name) do
    Process.put(:roundhouse_i18n_locale, name)
    name
  end

  def env_name() do
    Enum.find([System.get_env("RAILS_ENV"), System.get_env("RACK_ENV")], "development", &(&1 not in [nil, ""]))
  end

  # Not the working directory alone: `mix test` may start below the app root
  def table_text() do
    case :persistent_term.get(:roundhouse_i18n_table, nil) do
      nil ->
        text = File.read!(find_table(File.cwd!()))
        :persistent_term.put(:roundhouse_i18n_table, text)
        text

      text ->
        text
    end
  end

  defp find_table(dir) do
    path = Path.join([dir, "config", "i18n_translations"])

    cond do
      File.exists?(path) -> path
      Path.dirname(dir) == dir -> raise "config/i18n_translations is in no directory above " <> File.cwd!()
      true -> find_table(Path.dirname(dir))
    end
  end
end
