import org.jetbrains.kotlin.gradle.dsl.JvmTarget

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
    // Javalin logs through SLF4J; provide a simple binding so startup is quiet.
    implementation("org.slf4j:slf4j-simple:2.0.13")
}

application {
    mainClass.set("roundhouse.MainKt")
}

// Emit JVM 17 bytecode (Javalin's floor); compiled and run on the active
// JDK (26 locally). No toolchain pin so Gradle uses the running JDK rather
// than provisioning a specific one — Java and Kotlin tasks must agree on
// the target, so pin both to 17.
kotlin {
    compilerOptions {
        jvmTarget.set(JvmTarget.JVM_17)
    }
}

java {
    sourceCompatibility = JavaVersion.VERSION_17
    targetCompatibility = JavaVersion.VERSION_17
}
