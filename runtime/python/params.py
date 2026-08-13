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
