// Narrowing accessors over the recursive request-params tree.
//
// `Roundhouse::ParamValue` was a per-target TYPE WITH NO OPERATIONS, so
// every consumer narrowed at its own call site with `is_a?`, and each
// emitter recognized that as an IDIOM rather than modeling the test.
// The operations live beside the type, hand-written per target the way
// `Db` is: these bodies inspect this target's representation, which is
// what makes them a primitive rather than framework logic.
//
// Semantics fixed by a Rails 8.1 oracle: `permit` keeps a blank "" and
// `Parameters#compact` drops only an explicit nil. So blank counts as
// provided; absent, JSON null, and non-scalar do not.

package v2

func Params_sub(params map[string]RoundhouseParamValue, key string) map[string]RoundhouseParamValue {
	if value, ok := params[key]; ok {
		if nested, ok := value.(map[string]RoundhouseParamValue); ok {
			return nested
		}
	}
	return map[string]RoundhouseParamValue{}
}

func Params_str(sub map[string]RoundhouseParamValue, key string, fallback string) string {
	if value, ok := sub[key]; ok {
		if s, ok := value.(string); ok {
			return s
		}
	}
	return fallback
}

func Params_provided(sub map[string]RoundhouseParamValue, key string) bool {
	value, ok := sub[key]
	if !ok {
		return false
	}
	_, isString := value.(string)
	return isString
}

// The scalar readers the params lowering emits, and its record of
// multiparameter keys. Those keys (`title(1)`) are modeled on the Ruby
// family only (runtime/ruby/params.rb); this target ignores them, as it
// always has, so these pass through.
func Params_scalar_provided(sub map[string]RoundhouseParamValue, key string) bool {
	return Params_provided(sub, key)
}

func Params_scalar(sub map[string]RoundhouseParamValue, key string) string {
	return Params_str(sub, key, "")
}

func Params_no_fields() []string {
	return []string{}
}

func Params_multiparameter_fields(sub map[string]RoundhouseParamValue, keys []string) []string {
	return []string{}
}

func Params_multiparameter_unsupported(sub map[string]RoundhouseParamValue, keys []string) bool {
	return false
}

func Params_any_of(list []string, names []string) bool {
	for _, n := range names {
		for _, l := range list {
			if l == n {
				return true
			}
		}
	}
	return false
}
