// Roundhouse go2 framework error sentinels.
//
// Typed `panic(value)` payloads the raise-peephole emits for the
// three known framework exception classes. The HTTP router_glue's
// defer-recover type-switches against these to map runtime errors
// to HTTP responses (RecordNotFound → 404, RecordInvalid → 422,
// NotImplementedError → propagates as 500 since it's a bug).
//
// Each sentinel implements the `error` interface so callers can
// also test via `errors.As` if they ever need to.

package v2

// `Message` is `any` rather than `string` so the payload can carry
// either a Ruby-shape error string (`raise RecordNotFound, "Couldn't
// find ..."`) or a record value (Rails-style `raise RecordInvalid,
// instance` where the failing model travels through the error). Each
// sentinel's `Error()` stringifies via `fmt.Sprintf("%v", ...)` so
// the error-interface contract still works regardless of payload
// shape.

import "fmt"

type RecordNotFoundError struct {
	Message any
}

func (e *RecordNotFoundError) Error() string {
	return fmt.Sprintf("%v", e.Message)
}

type RecordInvalidError struct {
	Message any
}

func (e *RecordInvalidError) Error() string {
	return fmt.Sprintf("%v", e.Message)
}

type NotImplementedErrorValue struct {
	Message any
}

func (e *NotImplementedErrorValue) Error() string {
	return fmt.Sprintf("%v", e.Message)
}
