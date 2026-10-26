# Narrowing accessors over the recursive request-params tree.
#
# `Roundhouse::ParamValue` was a per-target TYPE WITH NO OPERATIONS, so
# every consumer narrowed at its own call site with `is_a?`, and each
# emitter recognized that as an IDIOM rather than modeling the test.
# The operations live beside the type, hand-written per target the way
# `Db` is: these bodies inspect this target's representation, which is
# what makes them a primitive rather than framework logic.
#
# Semantics fixed by a Rails 8.1 oracle: `permit` keeps a blank "" and
# `Parameters#compact` drops only an explicit nil. So blank counts as
# provided; absent, JSON null, and non-scalar do not.

from typing import Any


def sub(params: dict[str, Any], key: str) -> dict[str, Any]:
    value = params.get(key)
    return value if isinstance(value, dict) else {}


def str_(sub: dict[str, Any], key: str, fallback: str) -> str:
    value = sub.get(key)
    return value if isinstance(value, str) else fallback


def provided(sub: dict[str, Any], key: str) -> bool:
    return isinstance(sub.get(key), str)


# The scalar readers the params lowering emits, and its record of
# multiparameter keys. Those keys (`title(1)`) are modeled on the Ruby
# family only (runtime/ruby/params.rb); this target ignores them, as it
# always has, so these pass through.
def scalar_provided(sub: dict[str, Any], key: str) -> bool:
    return provided(sub, key)


def scalar(sub: dict[str, Any], key: str) -> str:
    return str_(sub, key, "")


def no_fields() -> list[str]:
    return []


def multiparameter_fields(sub: dict[str, Any], keys: list[str]) -> list[str]:
    return []


def multiparameter_unsupported(sub: dict[str, Any], keys: list[str]) -> bool:
    return False


def any_of(names_list: list[str], names: list[str]) -> bool:
    return any(n in names_list for n in names)
