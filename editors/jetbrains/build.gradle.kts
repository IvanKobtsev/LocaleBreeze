import org.jetbrains.intellij.platform.gradle.TestFrameworkType
import org.jetbrains.intellij.platform.gradle.tasks.PrepareSandboxTask
import org.jetbrains.kotlin.gradle.dsl.JvmTarget
import org.jetbrains.kotlin.gradle.tasks.KotlinCompile

plugins {
    id("org.jetbrains.kotlin.jvm") version "2.3.20"
    id("org.jetbrains.intellij.platform") version "2.18.1"
}

repositories {
    mavenCentral()
    intellijPlatform { defaultRepositories() }
}

dependencies {
    intellijPlatform {
        webstorm("2026.2.1")
        bundledPlugin("JavaScript")
        testFramework(TestFrameworkType.Platform)
    }
    testImplementation(kotlin("test"))
}

tasks.withType<KotlinCompile>().configureEach {
    compilerOptions.jvmTarget = JvmTarget.JVM_25
}

intellijPlatform {
    pluginConfiguration {
        ideaVersion {
            sinceBuild = "262.9437.145"
            untilBuild = "262.*"
        }
    }
}

val nativeBinaries = layout.buildDirectory.dir("generated-native-binaries")

val prepareNativeBinaries by tasks.registering(Copy::class) {
    from(rootProject.layout.projectDirectory.dir("../../dist/jetbrains"))
    into(nativeBinaries)
}

tasks.withType<PrepareSandboxTask>().configureEach {
    dependsOn(prepareNativeBinaries)
    from(nativeBinaries) {
        into(pluginName.map { "$it/bin" })
    }
}

val verifyBundledBinaries by tasks.registering {
    dependsOn(tasks.named("buildPlugin"))
    doLast {
        val archive = tasks.named<Zip>("buildPlugin").get().archiveFile.get().asFile
        val entries = mutableSetOf<String>()
        zipTree(archive).visit {
            if (!isDirectory) entries += relativePath.pathString.replace('\\', '/')
        }
        val expected = mapOf(
            "win32-x64" to "locale-breeze.exe",
            "win32-arm64" to "locale-breeze.exe",
            "darwin-x64" to "locale-breeze",
            "darwin-arm64" to "locale-breeze",
            "linux-x64" to "locale-breeze",
            "linux-arm64" to "locale-breeze",
        )
        for ((target, binary) in expected) {
            check(entries.any { it.endsWith("/bin/$target/$binary") }) {
                "Plugin archive is missing executable: bin/$target/$binary"
            }
        }
    }
}
