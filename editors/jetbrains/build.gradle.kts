import org.jetbrains.intellij.platform.gradle.TestFrameworkType
import org.jetbrains.intellij.platform.gradle.IntelliJPlatformType
import org.jetbrains.intellij.platform.gradle.tasks.BuildPluginVariantsTask
import org.jetbrains.kotlin.gradle.dsl.JvmTarget
import org.jetbrains.kotlin.gradle.dsl.JvmDefaultMode
import org.jetbrains.kotlin.gradle.tasks.KotlinCompile

plugins {
    id("org.jetbrains.kotlin.jvm") version "2.3.20"
    id("org.jetbrains.intellij.platform") version "2.20.1"
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
    compilerOptions.jvmDefault = JvmDefaultMode.NO_COMPATIBILITY
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

tasks.named<JavaExec>("runIde") {
    dependsOn(buildDevelopmentServer)
    jvmArgs(
        "-Ddev.localebreeze.development=true",
        "-Ddev.localebreeze.server=${developmentServer.asFile.absolutePath}",
    )
}

intellijPlatform {
    pluginConfiguration {
        ideaVersion {
            sinceBuild = "262.9437.145"
            untilBuild = "263.*"
        }
    }
    pluginVerification {
        ides {
            create(IntelliJPlatformType.WebStorm, "2026.2.1")
            create(IntelliJPlatformType.WebStorm, "2026.3")
        }
    }
}

intellijPlatform {
    nativeVariants {
        enabled = true
        windows.x86_64.from(repositoryRoot.dir("dist/jetbrains")) { include("win32-x64/**"); into("bin") }
        windows.arm64.from(repositoryRoot.dir("dist/jetbrains")) { include("win32-arm64/**"); into("bin") }
        mac.x86_64.from(repositoryRoot.dir("dist/jetbrains")) { include("darwin-x64/**"); into("bin") }
        mac.arm64.from(repositoryRoot.dir("dist/jetbrains")) { include("darwin-arm64/**"); into("bin") }
        linux.x86_64.from(repositoryRoot.dir("dist/jetbrains")) { include("linux-x64/**"); into("bin") }
        linux.arm64.from(repositoryRoot.dir("dist/jetbrains")) { include("linux-arm64/**"); into("bin") }
    }
}

val verifyBundledBinaries by tasks.registering {
    val variants = tasks.named<BuildPluginVariantsTask>("buildPluginVariants")
    dependsOn(variants)
    inputs.files(variants.map { it.archiveFiles })
    doLast {
        val targets = mapOf("windows-x86_64" to "win32-x64", "windows-arm64" to "win32-arm64", "mac-x86_64" to "darwin-x64", "mac-arm64" to "darwin-arm64", "linux-x86_64" to "linux-x64", "linux-arm64" to "linux-arm64")
        val archives = variants.get().archiveFiles.files.associateBy { it.name }
        check(archives.keys == targets.keys.map { "${project.name}-${project.version}-$it.zip" }.toSet())
        for ((target, directory) in targets) {
            val archive = archives.getValue("${project.name}-${project.version}-$target.zip")
            val binary = if (directory.startsWith("win32-")) "locale-breeze.exe" else "locale-breeze"
            java.util.zip.ZipFile(archive).use { zip ->
                val binaries = zip.entries().asSequence().filter { !it.isDirectory && "/bin/" in it.name }.toList()
                check(binaries.size == 1 && binaries.single().name.endsWith("/bin/$directory/$binary") && binaries.single().size > 0) {
                    "${archive.name} must contain only the $directory language server"
                }
            }
        }
    }
}
