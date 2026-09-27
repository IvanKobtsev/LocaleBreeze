import org.jetbrains.intellij.platform.gradle.TestFrameworkType
import org.jetbrains.intellij.platform.gradle.IntelliJPlatformType
import org.jetbrains.intellij.platform.gradle.tasks.PrepareSandboxTask
import org.jetbrains.intellij.platform.gradle.tasks.RunIdeTask
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
        androidStudio("2026.1.4.8")
        bundledPlugin("com.intellij.modules.json")
        plugin("com.redhat.devtools.lsp4ij", "0.20.1")
        testFramework(TestFrameworkType.Platform)
    }
    testImplementation(kotlin("test"))
}

tasks.withType<KotlinCompile>().configureEach {
    compilerOptions.jvmTarget = JvmTarget.JVM_21
}

val repositoryRoot = rootProject.layout.projectDirectory.dir("../..")
val developmentExecutableName = if (System.getProperty("os.name").startsWith("Windows")) {
    "locale-breeze.exe"
} else {
    "locale-breeze"
}
val developmentServer = repositoryRoot.file("target/debug/$developmentExecutableName")

val buildDevelopmentServer by tasks.registering(Exec::class) {
    workingDir(repositoryRoot)
    commandLine("cargo", "build", "-p", "locale-breeze")
}

tasks.named<RunIdeTask>("runIde") {
    dependsOn(buildDevelopmentServer)
    jvmArgs(
        "-Ddev.localebreeze.development=true",
        "-Ddev.localebreeze.server=${developmentServer.asFile.absolutePath}",
    )
    doFirst {
        // Android Studio's What's New assistant resolves this directory with
        // toRealPath() during first startup instead of creating it itself.
        sandboxSystemDirectory.get().dir("whatsnew").asFile.mkdirs()
    }
}

intellijPlatform {
    pluginConfiguration {
        ideaVersion {
            sinceBuild = "261.26222.65"
            untilBuild = "261.*"
        }
    }
    pluginVerification {
        ides {
            create(IntelliJPlatformType.AndroidStudio, "2026.1.4.8")
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

val verifyNoNativeLsp by tasks.registering {
    dependsOn(tasks.named("jar"))
    doLast {
        val forbidden = "com/intellij/platform/lsp"
        val sources = fileTree("src") { include("**/*.kt", "**/*.java", "**/*.xml") }
        check(sources.none { it.readText().contains(forbidden.replace('/', '.')) }) {
            "Android Studio plugin must not reference JetBrains native LSP APIs"
        }

        val pluginJar = tasks.named<Jar>("jar").get().archiveFile.get().asFile
        val compiledReference = zipTree(pluginJar)
            .matching { include("**/*.class") }
            .files
            .firstOrNull { classFile ->
                String(classFile.readBytes(), Charsets.ISO_8859_1).contains(forbidden)
            }
        check(compiledReference == null) {
            "Android Studio plugin bytecode references JetBrains native LSP APIs: ${compiledReference?.name}"
        }
    }
}

tasks.named("check") {
    dependsOn(verifyNoNativeLsp)
}
