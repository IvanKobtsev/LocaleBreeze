package dev.localebreeze.jetbrains

import com.intellij.execution.configurations.GeneralCommandLine
import com.intellij.notification.NotificationGroupManager
import com.intellij.notification.NotificationType
import com.intellij.openapi.application.PathManager
import com.intellij.openapi.diagnostic.Logger
import com.intellij.openapi.project.Project
import com.intellij.openapi.util.SystemInfoRt
import com.intellij.openapi.vfs.VirtualFile
import com.intellij.platform.lsp.api.LspIntegrationProvider
import com.intellij.platform.lsp.api.ProjectWideLspClientDescriptor
import com.intellij.platform.lsp.api.customization.LspCustomization
import com.intellij.platform.lsp.api.customization.LspGoToDefinitionDisabled
import java.nio.file.Files
import java.nio.file.Path
import java.util.concurrent.ConcurrentHashMap

class LocaleBreezeLspIntegrationProvider : LspIntegrationProvider {
    override fun fileOpened(
        project: Project,
        file: VirtualFile,
        clientStarter: LspIntegrationProvider.LspClientStarter,
    ) {
        if (!isSupported(file)) return
        val executable = LocaleBreezeExecutable.resolve(project)
        if (executable == null) {
            notifyMissingExecutable(project)
            return
        }
        clientStarter.ensureClientStarted(LocaleBreezeLspClientDescriptor(project, executable))
    }

    private fun notifyMissingExecutable(project: Project) {
        if (!notifiedProjects.add(project.locationHash)) return
        NotificationGroupManager.getInstance()
            .getNotificationGroup("LocaleBreeze")
            .createNotification(
                "LocaleBreeze server not found",
                "Configure the server executable under Settings | Tools | LocaleBreeze, or install a plugin package containing native binaries.",
                NotificationType.ERROR,
            )
            .notify(project)
    }

    companion object {
        private val notifiedProjects = ConcurrentHashMap.newKeySet<String>()
        private val supportedExtensions = setOf("js", "jsx", "ts", "tsx", "json")

        internal fun isSupported(file: VirtualFile): Boolean =
            file.extension?.lowercase() in supportedExtensions
    }
}

private class LocaleBreezeLspClientDescriptor(
    project: Project,
    private val executable: Path,
) : ProjectWideLspClientDescriptor(project, "LocaleBreeze") {
    override fun isSupportedFile(file: VirtualFile): Boolean =
        LocaleBreezeLspIntegrationProvider.isSupported(file)

    override fun createCommandLine(): GeneralCommandLine {
        val command = GeneralCommandLine(executable.toString(), "lsp", "--stdio")
        project.basePath?.let(command::withWorkDirectory)
        LocaleBreezeExecutable.resolveConfig(project)?.let {
            command.addParameters("--config", it.toString())
        }
        return command
    }

    override val lspCustomization: LspCustomization = LocaleBreezeLspCustomization
}

private object LocaleBreezeLspCustomization : LspCustomization() {
    override val goToDefinitionCustomizer = LspGoToDefinitionDisabled
}

private object LocaleBreezeExecutable {
    private val log = Logger.getInstance(LocaleBreezeExecutable::class.java)

    fun resolve(project: Project): Path? {
        val configured = LocaleBreezeSettings.getInstance(project).state.serverPath
        if (configured.isNotBlank()) {
            val path = resolveProjectPath(project, configured)
            return path.takeIf(Files::isRegularFile)
        }
        val pluginRoot = pluginRoot() ?: return null
        val executable = pluginRoot.resolve("bin").resolve(platformDirectory()).resolve(executableName())
        if (!Files.isRegularFile(executable)) return null
        if (!SystemInfoRt.isWindows && !executable.toFile().setExecutable(true)) {
            log.warn("Could not mark LocaleBreeze executable as executable: $executable")
        }
        return executable
    }

    fun resolveConfig(project: Project): Path? {
        val configured = LocaleBreezeSettings.getInstance(project).state.configPath
        if (configured.isBlank()) return null
        return resolveProjectPath(project, configured)
    }

    private fun resolveProjectPath(project: Project, value: String): Path {
        val path = Path.of(value)
        if (path.isAbsolute) return path.normalize()
        val root = project.basePath?.let(Path::of) ?: Path.of(PathManager.getSystemPath())
        return root.resolve(path).normalize()
    }

    private fun pluginRoot(): Path? {
        val location = runCatching {
            Path.of(LocaleBreezeExecutable::class.java.protectionDomain.codeSource.location.toURI())
        }.getOrNull() ?: return null
        var candidate = if (Files.isRegularFile(location)) location.parent else location
        repeat(4) {
            if (Files.isDirectory(candidate.resolve("bin"))) return candidate
            candidate = candidate.parent ?: return null
        }
        return null
    }

    private fun platformDirectory(): String {
        val os = when {
            SystemInfoRt.isWindows -> "win32"
            SystemInfoRt.isMac -> "darwin"
            else -> "linux"
        }
        val architecture = if (System.getProperty("os.arch").lowercase() in setOf("aarch64", "arm64")) {
            "arm64"
        } else {
            "x64"
        }
        return "$os-$architecture"
    }

    private fun executableName(): String = if (SystemInfoRt.isWindows) "locale-breeze.exe" else "locale-breeze"
}
