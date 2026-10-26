package v2

import (
	"os"
	"path/filepath"
	"sync"
)

// Not a package var: net/http serves each request on its own goroutine, and Go has no goroutine-local storage
var i18nLocaleByGoroutine sync.Map // uint64 → string

var i18nTable struct {
	once sync.Once
	text string
}

func I18nHost_reset_request() {
	i18nLocaleByGoroutine.Delete(goroutineID())
}

func I18nHost_locale_name() string {
	if v, ok := i18nLocaleByGoroutine.Load(goroutineID()); ok {
		return v.(string)
	}
	return ""
}

func I18nHost_set_locale_name(name string) string {
	i18nLocaleByGoroutine.Store(goroutineID(), name)
	return name
}

func I18nHost_env_name() string {
	if v := os.Getenv("RAILS_ENV"); v != "" {
		return v
	}
	if v := os.Getenv("RACK_ENV"); v != "" {
		return v
	}
	return "development"
}

// Not the working directory alone: `go test` runs in the package directory, below the app root
func I18nHost_table_text() string {
	i18nTable.once.Do(func() {
		start, _ := os.Getwd()
		for dir := start; ; dir = filepath.Dir(dir) {
			if b, err := os.ReadFile(filepath.Join(dir, "config", "i18n_translations")); err == nil {
				i18nTable.text = string(b)
				return
			}
			if filepath.Dir(dir) == dir {
				panic("config/i18n_translations is in no directory above " + start)
			}
		}
	})
	return i18nTable.text
}
