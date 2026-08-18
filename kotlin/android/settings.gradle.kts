pluginManagement {
    repositories {
        google()
        mavenCentral()
        gradlePluginPortal()
    }
}

dependencyResolutionManagement {
    repositoriesMode.set(RepositoriesMode.FAIL_ON_PROJECT_REPOS)
    repositories {
        google()
        mavenCentral()
    }
}

rootProject.name = "anydoc-kotlin"
include(":anydoc")
// :jvm-test is a standalone Gradle project (its own settings.gradle.kts).
// Including it here would configure its test task on every AAR build.
