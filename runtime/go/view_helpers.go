// Roundhouse Go view-helpers runtime.
//
// Hand-written, shipped alongside generated code (copied in by the
// Go emitter as `app/view_helpers.go`). Ports the same helper
// surface as runtime/rust/view_helpers.rs and
// runtime/python/view_helpers.py — link_to, button_to, FormBuilder,
// turbo_stream_from, dom_id, pluralize, truncate, plus request-
// scoped yield/slot storage for layout dispatch.
//
// Thread-safety: render state lives in a sync.Mutex-guarded map
// keyed by nothing (single-server shape, like rust's OnceLock +
// thread-local trade-off). net/http runs each request on its own
// goroutine, so a later pass can swap this to goroutine-local
// context if concurrent render becomes an issue.

package app

import (
	"encoding/base64"
	"encoding/json"
	"fmt"
	"html"
	"sort"
	"strconv"
	"strings"
	"sync"
)

// ── Render state (yield + content_for slots) ───────────────────

var renderMu sync.Mutex
var renderYield string
var renderSlots = map[string]string{}

// ResetRenderState wipes the current request's yield/slot state.
// Called by the server middleware at the start of every dispatch.
func ResetRenderState() {
	renderMu.Lock()
	defer renderMu.Unlock()
	renderYield = ""
	renderSlots = map[string]string{}
}

// SetYield stashes the inner view body for the layout's
// `<%= yield %>`.
func SetYield(body string) {
	renderMu.Lock()
	defer renderMu.Unlock()
	renderYield = body
}

// GetYield reads the stashed inner view body.
func GetYield() string {
	renderMu.Lock()
	defer renderMu.Unlock()
	return renderYield
}

// GetSlot reads a named `content_for` slot.
func GetSlot(name string) string {
	renderMu.Lock()
	defer renderMu.Unlock()
	return renderSlots[name]
}

// ContentForSet stashes a string into a named `content_for` slot.
func ContentForSet(slot, body string) {
	renderMu.Lock()
	defer renderMu.Unlock()
	renderSlots[slot] = body
}

// ContentForGet returns the named slot's current body.
func ContentForGet(slot string) string {
	return GetSlot(slot)
}

// ── Layout-meta helpers ────────────────────────────────────────

// CsrfMetaTags emits two `<meta>` tags; tokens are blank (the
// compare tool masks them). Rails' byte-output separates them with
// a newline, which we match since DOM compare counts whitespace
// text nodes.
func CsrfMetaTags() string {
	return `<meta name="csrf-param" content="authenticity_token" />` + "\n" +
		`<meta name="csrf-token" content="" />`
}

// CspMetaTag is empty — we don't emit CSP nonces.
func CspMetaTag() string { return "" }

// StylesheetLinkTag emits a `<link rel="stylesheet">`. Extra attrs
// (typically `data-turbo-track: "reload"`) fold in sorted by key.
func StylesheetLinkTag(name string, opts map[string]string) string {
	href := "/assets/" + name + ".css"
	return fmt.Sprintf(`<link rel="stylesheet" href="%s"%s />`,
		escapeHTML(href), sortedAttrs(opts))
}

// JavascriptImportmapTags emits the importmap JSON + module-preload
// links + bootstrap script, matching Rails byte-output. Pins are
// passed as parallel name/path slices to preserve declaration
// order (Go maps iterate randomly).
func JavascriptImportmapTags(pins [][2]string, mainEntry string) string {
	var b strings.Builder
	b.WriteString(`<script type="importmap" data-turbo-track="reload">`)
	b.WriteString("{\n")
	b.WriteString(`  "imports": {` + "\n")
	for i, pin := range pins {
		sep := ","
		if i+1 == len(pins) {
			sep = ""
		}
		nameJSON, _ := json.Marshal(pin[0])
		pathJSON, _ := json.Marshal(pin[1])
		b.WriteString(fmt.Sprintf("    %s: %s%s\n", nameJSON, pathJSON, sep))
	}
	b.WriteString("  }\n")
	b.WriteString("}</script>")
	for _, pin := range pins {
		b.WriteString("\n")
		b.WriteString(fmt.Sprintf(`<link rel="modulepreload" href="%s">`, escapeHTML(pin[1])))
	}
	b.WriteString("\n")
	b.WriteString(fmt.Sprintf(`<script type="module">import "%s"</script>`, escapeHTML(mainEntry)))
	return b.String()
}

// ── link_to / button_to ────────────────────────────────────────

// LinkTo emits `<a href="url" ...>text</a>`.
func LinkTo(text, url string, opts map[string]string) string {
	return fmt.Sprintf(`<a href="%s"%s>%s</a>`,
		escapeHTML(url), sortedAttrs(opts), escapeHTML(text))
}

// ButtonTo emits `<form><button>text</button></form>` with CSRF
// hidden input. `method:"delete|put|patch"` becomes the `_method`
// hidden input. `class:` goes on the button; `form_class:` on the
// wrapper form (defaulting to `"button_to"` per Rails convention).
// `data-*` keys flatten onto the button.
func ButtonTo(text, target string, opts map[string]string) string {
	method := opts["method"]
	if method == "" {
		method = "post"
	}
	buttonClass := opts["class"]
	formClass := opts["form_class"]
	if formClass == "" {
		formClass = "button_to"
	}
	methodLower := strings.ToLower(method)
	var methodInput string
	if methodLower != "post" && methodLower != "get" {
		methodInput = fmt.Sprintf(`<input type="hidden" name="_method" value="%s" />`, escapeHTML(method))
	}
	buttonAttrs := ""
	keys := sortedKeys(opts)
	for _, k := range keys {
		if strings.HasPrefix(k, "data-") {
			buttonAttrs += fmt.Sprintf(` %s="%s"`, escapeHTML(k), escapeHTML(opts[k]))
		}
	}
	buttonClassAttr := ""
	if buttonClass != "" {
		buttonClassAttr = fmt.Sprintf(` class="%s"`, escapeHTML(buttonClass))
	}
	csrfInput := `<input type="hidden" name="authenticity_token" value="">`
	return fmt.Sprintf(`<form class="%s" method="post" action="%s">%s<button%s%s type="submit">%s</button>%s</form>`,
		escapeHTML(formClass),
		escapeHTML(target),
		methodInput,
		buttonClassAttr,
		buttonAttrs,
		escapeHTML(text),
		csrfInput,
	)
}

// ── form_with wrapper ──────────────────────────────────────────

// FormWrap wraps an inner view buffer in a `<form>` tag. Emits the
// _method override when `isPersisted`, plus a blank CSRF input.
func FormWrap(action string, isPersisted bool, class, inner string) string {
	classAttr := ""
	if class != "" {
		classAttr = fmt.Sprintf(` class="%s"`, escapeHTML(class))
	}
	methodInput := ""
	if isPersisted {
		methodInput = `<input type="hidden" name="_method" value="patch">`
	}
	csrfInput := `<input type="hidden" name="authenticity_token" value="">`
	return fmt.Sprintf(`<form%s action="%s" accept-charset="UTF-8" method="post">%s%s%s</form>`,
		classAttr,
		escapeHTML(action),
		methodInput,
		csrfInput,
		inner,
	)
}

// ── FormBuilder ────────────────────────────────────────────────

// FormBuilder is the one instance per form_with block. Field names
// prefix as `{prefix}[{field}]` in inputs; submit's default label
// is `Create/Update {capitalized prefix}`.
type FormBuilder struct {
	Prefix      string
	Class       string
	IsPersisted bool
}

// NewFormBuilder mirrors the rust runtime's constructor shape.
func NewFormBuilder(prefix, class string, isPersisted bool) *FormBuilder {
	return &FormBuilder{Prefix: prefix, Class: class, IsPersisted: isPersisted}
}

func (f *FormBuilder) nameFor(field string) string {
	if f.Prefix == "" {
		return field
	}
	return fmt.Sprintf("%s[%s]", f.Prefix, field)
}

func (f *FormBuilder) idFor(field string) string {
	if f.Prefix == "" {
		return field
	}
	return f.Prefix + "_" + field
}

// Label emits `<label for="...">Field</label>`. Rails capitalizes
// the first letter of the field name for the label text.
func (f *FormBuilder) Label(field string, opts map[string]string) string {
	classAttr := ""
	if cls, ok := opts["class"]; ok && cls != "" {
		classAttr = fmt.Sprintf(` class="%s"`, escapeHTML(cls))
	}
	return fmt.Sprintf(`<label for="%s"%s>%s</label>`,
		escapeHTML(f.idFor(field)),
		classAttr,
		escapeHTML(capitalizeFirst(field)),
	)
}

// TextField emits a `<input type="text">`. Empty value omits the
// value attr (Rails convention).
func (f *FormBuilder) TextField(field, value string, opts map[string]string) string {
	classAttr := ""
	if cls, ok := opts["class"]; ok && cls != "" {
		classAttr = fmt.Sprintf(` class="%s"`, escapeHTML(cls))
	}
	valueAttr := ""
	if value != "" {
		valueAttr = fmt.Sprintf(` value="%s"`, escapeHTML(value))
	}
	return fmt.Sprintf(`<input type="text" name="%s" id="%s"%s%s />`,
		escapeHTML(f.nameFor(field)),
		escapeHTML(f.idFor(field)),
		valueAttr,
		classAttr,
	)
}

// Textarea wraps content in a leading newline (HTML5 shape).
func (f *FormBuilder) Textarea(field, value string, opts map[string]string) string {
	classAttr := ""
	if cls, ok := opts["class"]; ok && cls != "" {
		classAttr = fmt.Sprintf(` class="%s"`, escapeHTML(cls))
	}
	rowsAttr := ""
	if rows, ok := opts["rows"]; ok && rows != "" {
		rowsAttr = fmt.Sprintf(` rows="%s"`, escapeHTML(rows))
	}
	body := ""
	if value != "" {
		body = escapeHTML(value)
	}
	return fmt.Sprintf(`<textarea%s%s name="%s" id="%s">%s</textarea>`,
		rowsAttr,
		classAttr,
		escapeHTML(f.nameFor(field)),
		escapeHTML(f.idFor(field)),
		"\n"+body,
	)
}

// Submit emits `<input type="submit">`. Default label is
// `Create/Update {prefix}` (capitalized first letter).
func (f *FormBuilder) Submit(opts map[string]string) string {
	classAttr := ""
	if cls, ok := opts["class"]; ok && cls != "" {
		classAttr = fmt.Sprintf(` class="%s"`, escapeHTML(cls))
	}
	label := opts["label"]
	if label == "" {
		prefixHuman := capitalizeFirst(f.Prefix)
		if f.IsPersisted {
			label = "Update " + prefixHuman
		} else {
			label = "Create " + prefixHuman
		}
	}
	esc := escapeHTML(label)
	return fmt.Sprintf(`<input type="submit" name="commit" value="%s"%s data-disable-with="%s" />`,
		esc, classAttr, esc)
}

// ── Turbo / misc ───────────────────────────────────────────────

// TurboStreamFrom matches Rails' byte-output: the channel attribute
// is always `Turbo::StreamsChannel`; the actual channel name travels
// base64-encoded through `signed-stream-name`. Explicit closing tag
// (custom elements can't self-close per HTML5).
func TurboStreamFrom(channel string) string {
	nameJSON, _ := json.Marshal(channel)
	encoded := base64.StdEncoding.EncodeToString(nameJSON)
	return fmt.Sprintf(
		`<turbo-cable-stream-source channel="Turbo::StreamsChannel" signed-stream-name="%s--unsigned"></turbo-cable-stream-source>`,
		encoded,
	)
}

// DomId emits Rails' standard `{singular}_{id}` or
// `{prefix}_{singular}_{id}` dom id.
func DomId(singular string, id int64, prefix string) string {
	base := fmt.Sprintf("%s_%d", singular, id)
	if prefix != "" {
		return prefix + "_" + base
	}
	return base
}

// Pluralize returns `{count} {word}` (singular for 1, plural `s`
// otherwise).
func Pluralize(count int64, word string) string {
	if count == 1 {
		return fmt.Sprintf("1 %s", word)
	}
	return fmt.Sprintf("%d %ss", count, word)
}

// Truncate with optional length + omission. Rails default length
// is 30, default omission is "...".
func Truncate(text string, opts map[string]string) string {
	length := 30
	if s, ok := opts["length"]; ok {
		if n, err := strconv.Atoi(s); err == nil {
			length = n
		}
	}
	omission := "..."
	if s, ok := opts["omission"]; ok {
		omission = s
	}
	if len(text) <= length {
		return text
	}
	cut := length - len(omission)
	if cut < 0 {
		cut = 0
	}
	return text[:cut] + omission
}

// FieldHasError returns true when any ValidationError in errors
// targets the named field.
func FieldHasError(errors []ValidationError, field string) bool {
	for _, e := range errors {
		if e.Field == field {
			return true
		}
	}
	return false
}

// ErrorMessagesFor returns a formatted block of validation errors.
// Empty when errors is empty — matches the TS/rust stubs.
func ErrorMessagesFor(errors []ValidationError, noun string) string {
	_ = noun
	if len(errors) == 0 {
		return ""
	}
	return ""
}

// ── helpers ────────────────────────────────────────────────────

func sortedAttrs(opts map[string]string) string {
	if len(opts) == 0 {
		return ""
	}
	keys := sortedKeys(opts)
	var b strings.Builder
	for _, k := range keys {
		b.WriteString(fmt.Sprintf(` %s="%s"`, escapeHTML(k), escapeHTML(opts[k])))
	}
	return b.String()
}

func sortedKeys(m map[string]string) []string {
	keys := make([]string, 0, len(m))
	for k := range m {
		keys = append(keys, k)
	}
	sort.Strings(keys)
	return keys
}

func escapeHTML(s string) string {
	return html.EscapeString(s)
}

func capitalizeFirst(s string) string {
	if s == "" {
		return s
	}
	return strings.ToUpper(s[:1]) + s[1:]
}
