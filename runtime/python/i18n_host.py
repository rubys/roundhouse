import contextvars
import os

# Not a module global: aiohttp serves each request in its own task, interleaved across awaits
_locale: contextvars.ContextVar[str] = contextvars.ContextVar("roundhouse_i18n_locale", default="")
_table: list[str] = []


class I18nHost:
    @staticmethod
    def reset_request() -> None:
        _locale.set("")

    @staticmethod
    def locale_name() -> str:
        return _locale.get()

    @staticmethod
    def set_locale_name(name: str) -> str:
        _locale.set(name)
        return name

    @staticmethod
    def env_name() -> str:
        return os.environ.get("RAILS_ENV") or os.environ.get("RACK_ENV") or "development"

    # Not the working directory alone: a test runner may start below the app root
    @staticmethod
    def table_text() -> str:
        if not _table:
            here = os.path.abspath(os.getcwd())
            while True:
                path = os.path.join(here, "config", "i18n_translations")
                if os.path.exists(path):
                    with open(path, encoding="utf-8") as f:
                        _table.append(f.read())
                    break
                parent = os.path.dirname(here)
                if parent == here:
                    raise FileNotFoundError("config/i18n_translations is in no directory above " + os.getcwd())
                here = parent
        return _table[0]
