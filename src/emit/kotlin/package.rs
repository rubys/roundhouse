//! Ecosystem files for the emitted Kotlin project — the Gradle scaffold.
//! The analog of `src/emit/typescript/package.rs` (which emits
//! `package.json`/`tsconfig.json`). Locked stack: Gradle (Kotlin DSL),
//! Javalin (HTTP), xerial `sqlite-jdbc` (DB) — see
//! `docs/kotlin-migration-plan.md`.

use std::path::PathBuf;

use crate::emit::EmittedFile;

// JVM 17 bytecode (Javalin's floor); no toolchain pin so Gradle uses the
// running JDK. Java + Kotlin tasks must agree on the target, so both are
// pinned to 17. Kept in sync with `kotlin-reference/build.gradle.kts`.
const BUILD_GRADLE_KTS: &str = r#"import org.jetbrains.kotlin.gradle.dsl.JvmTarget

plugins {
    kotlin("jvm") version "2.4.0"
    application
}

repositories {
    mavenCentral()
}

dependencies {
    implementation("io.javalin:javalin:6.4.0")
    implementation("org.xerial:sqlite-jdbc:3.46.1.3")
    implementation("org.slf4j:slf4j-simple:2.0.13")
}

application {
    mainClass.set("roundhouse.MainKt")
}

kotlin {
    compilerOptions {
        jvmTarget.set(JvmTarget.JVM_17)
    }
}

java {
    sourceCompatibility = JavaVersion.VERSION_17
    targetCompatibility = JavaVersion.VERSION_17
}
"#;

const SETTINGS_GRADLE_KTS: &str = "rootProject.name = \"roundhouse-app\"\n";

const GITIGNORE: &str = "/build/\n/.gradle/\n/storage/\n";

/// The Gradle scaffold files. Phase 1 emits only these; Phase 2+ adds the
/// `src/main/kotlin/` sources (models, controllers, views, runtime).
pub fn scaffold() -> Vec<EmittedFile> {
    vec![
        EmittedFile {
            path: PathBuf::from("build.gradle.kts"),
            content: BUILD_GRADLE_KTS.to_string(),
        },
        EmittedFile {
            path: PathBuf::from("settings.gradle.kts"),
            content: SETTINGS_GRADLE_KTS.to_string(),
        },
        EmittedFile {
            path: PathBuf::from(".gitignore"),
            content: GITIGNORE.to_string(),
        },
    ]
}
