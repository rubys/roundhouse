package roundhouse

import java.io.File

object I18nHost {
    // Not one property: Javalin serves requests on several Jetty worker threads at once
    private val locale: ThreadLocal<String> = ThreadLocal.withInitial { "" }
    @Volatile private var table: String? = null

    fun resetRequest() {
        locale.set("")
    }

    fun localeName(): String = locale.get()

    fun setLocaleName(name: String): String {
        locale.set(name)
        return name
    }

    fun envName(): String = System.getenv("RAILS_ENV") ?: System.getenv("RACK_ENV") ?: "development"

    // Not the working directory alone: a test runner may start below the app root
    fun tableText(): String {
        table?.let { return it }
        var dir: File? = File(System.getProperty("user.dir")).absoluteFile
        while (dir != null) {
            val file = File(dir, "config/i18n_translations")
            if (file.exists()) {
                val text = file.readText()
                table = text
                return text
            }
            dir = dir.parentFile
        }
        throw IllegalStateException("config/i18n_translations is in no directory above " + System.getProperty("user.dir"))
    }
}

