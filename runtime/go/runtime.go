// Roundhouse Go runtime.
//
// Hand-written Go shipped alongside each generated app. The Go emitter
// copies this file verbatim into the generated project as
// `app/runtime.go`, so generated code can reference the types and
// helpers defined here. Mirrors runtime/rust/runtime.rs and
// runtime/crystal/runtime.cr — same per-target posture: minimal
// surface, each new lowering adds exactly what it needs.

package app

import "strings"

// ValidationError is a single validation failure produced by a model's
// generated Validate method. Carries the attribute name and a human-
// readable message; FullMessage composes them into a Rails-compatible
// display string ("Title can't be blank").
type ValidationError struct {
	Field   string
	Message string
}

// NewValidationError is the constructor the emitted model code calls.
// Accepts plain string arguments so emit can pass literals directly.
func NewValidationError(field, message string) ValidationError {
	return ValidationError{Field: field, Message: message}
}

// FullMessage returns the Rails display form: capitalize the field
// name, replace underscores with spaces, prepend to the message.
// ValidationError{Field: "post_id", Message: "can't be blank"} →
// "Post id can't be blank".
func (v ValidationError) FullMessage() string {
	label := strings.ReplaceAll(v.Field, "_", " ")
	if len(label) > 0 {
		label = strings.ToUpper(label[:1]) + label[1:]
	}
	return label + " " + v.Message
}
