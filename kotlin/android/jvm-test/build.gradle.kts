import org.jetbrains.kotlin.gradle.dsl.JvmTarget

plugins {
    id("org.jetbrains.kotlin.jvm") version "2.0.21"
}

val nativeDir = providers.gradleProperty("anydoc.nativeDir")
    .orElse(providers.environmentVariable("ANYDOC_NATIVE_DIR"))

kotlin {
    compilerOptions {
        jvmTarget.set(JvmTarget.JVM_17)
    }
}

sourceSets {
    main {
        kotlin.srcDir("../generated")
        kotlin.srcDir("../common")
    }
}

dependencies {
    implementation("net.java.dev.jna:jna:5.19.1")
    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-core:1.9.0")
    testImplementation(kotlin("test"))
}

tasks.test {
    useJUnitPlatform()
    // Resolve at execution, not configuration: this project is sometimes
    // evaluated by a parent build that never runs these tests.
    doFirst {
        val dir = nativeDir.orNull
            ?: error("Pass -Panydoc.nativeDir=... or set ANYDOC_NATIVE_DIR to the directory containing the anydoc_kotlin native library")
        environment("ANYDOC_NATIVE_DIR", dir)
        systemProperty("jna.library.path", dir)
        systemProperty("uniffi.component.anydoc.libraryOverride", "anydoc_kotlin")
    }
}

java {
    sourceCompatibility = JavaVersion.VERSION_17
    targetCompatibility = JavaVersion.VERSION_17
}
