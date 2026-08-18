import org.jetbrains.kotlin.gradle.dsl.JvmTarget

plugins {
    id("com.android.library")
    id("org.jetbrains.kotlin.android")
    id("maven-publish")
}

val groupIdValue = providers.gradleProperty("GROUP_ID").get()
val artifactIdValue = providers.gradleProperty("ARTIFACT_ID").get()
val versionName = providers.gradleProperty("VERSION_NAME").get()

android {
    namespace = "dev.firecrawl.anydoc"
    compileSdk = 35

    defaultConfig {
        minSdk = 24
        consumerProguardFiles("consumer-rules.pro")
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    buildTypes {
        release {
            isMinifyEnabled = false
        }
    }

    packaging {
        jniLibs {
            useLegacyPackaging = false
        }
    }

    publishing {
        singleVariant("release") {
            withSourcesJar()
        }
    }

    sourceSets {
        getByName("main") {
            kotlin.srcDir("../generated")
            kotlin.srcDir("../common")
        }
    }
}

kotlin {
    compilerOptions {
        jvmTarget.set(JvmTarget.JVM_17)
    }
}

dependencies {
    api("net.java.dev.jna:jna:5.19.1@aar")
    api("org.jetbrains.kotlinx:kotlinx-coroutines-core:1.9.0")
}

afterEvaluate {
    publishing {
        publications {
            register<MavenPublication>("release") {
                groupId = groupIdValue
                artifactId = artifactIdValue
                version = versionName
                from(components["release"])
                pom {
                    name.set("anydoc")
                    description.set(
                        "Convert Word, PowerPoint, Excel, OpenDocument, RTF, EPUB, CSV, and PDF files into GitHub-Flavored Markdown.",
                    )
                    url.set("https://github.com/firecrawl/anydoc")
                    licenses {
                        license {
                            name.set("MIT License")
                            url.set("https://opensource.org/licenses/MIT")
                        }
                    }
                    scm {
                        url.set("https://github.com/firecrawl/anydoc")
                        connection.set("scm:git:https://github.com/firecrawl/anydoc.git")
                    }
                }
            }
        }
        repositories {
            maven {
                name = "LocalBuild"
                url = uri(rootProject.layout.buildDirectory.dir("maven"))
            }
            maven {
                name = "GitHubPackages"
                url = uri(
                    "https://maven.pkg.github.com/${System.getenv("GITHUB_REPOSITORY") ?: "firecrawl/anydoc"}",
                )
                credentials {
                    username = System.getenv("GITHUB_ACTOR")
                    password = System.getenv("GITHUB_TOKEN")
                }
            }
        }
    }
}
