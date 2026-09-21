package dev.localebreeze.jetbrains

import com.intellij.execution.configurations.GeneralCommandLine
import com.intellij.openapi.components.service
import com.intellij.openapi.application.PathManager
import com.intellij.openapi.diagnostic.Logger
import com.intellij.openapi.project.Project
import com.intellij.openapi.util.Key
import com.intellij.openapi.util.SystemInfoRt
import com.intellij.openapi.vfs.VirtualFile
import com.intellij.platform.lsp.api.LspClientManager
import com.intellij.platform.lsp.api.Lsp4jClient
import com.intellij.platform.lsp.api.LspServerNotificationsHandler
import com.intellij.platform.lsp.api.LspIntegrationProvider
import com.intellij.platform.lsp.api.ProjectWideLspClientDescriptor
import com.intellij.platform.lsp.api.customization.LspCustomization
import com.intellij.platform.lsp.api.customization.LspGoToDefinitionDisabled
import java.nio.file.Files
import java.nio.file.Path
import org.eclipse.lsp4j.jsonrpc.services.JsonNotification

class LocaleBreezeLspIntegrationProvider : LspIntegrationProvider {
    private val log = Logger.getInstance(LocaleBreezeLspIntegrationProvider::class.java)

    override fun fileOpened(
        project: Project,
        file: VirtualFile,
        clientStarter: LspIntegrationProvider.LspClientStarter,
    ) {
        if (!LocaleBreezeSettings.getInstance(project).state.enabled) return
        if (!isSupported(file)) return
        if (LocaleBreezeExecutable.resolve(project) == null) {
            project.service<LocaleBreezeWarningCoordinator>().show(
                LocaleBreezeWorkspaceIssue(
                    code = "server_missing",
                    summary = "LocaleBreeze language server is missing",
                    remediation = "Reinstall the LocaleBreeze plugin to restore its bundled language-server executable.",
                ),
            )
            return
        }
        val descriptor = descriptor(project)
        val clientsBefore = LspClientManager.getInstance(project)
            .getClients(LocaleBreezeLspIntegrationProvider::class.java)
            .size
        log.info(
            "LocaleBreeze client request: project=${project.locationHash}, " +
                "file=${file.path}, descriptor=${System.identityHashCode(descriptor)}, " +
                "clientsBefore=$clientsBefore",
        )
        clientStarter.ensureClientStarted(descriptor)
    }

    companion object {
        private val descriptorKey =
            Key.create<LocaleBreezeLspClientDescriptor>("dev.localebreeze.jetbrains.lspDescriptor")
        private val supportedExtensions = setOf("js", "jsx", "ts", "tsx", "json")

        internal fun isSupported(file: VirtualFile): Boolean =
            file.extension?.lowercase() in supportedExtensions

        private fun descriptor(project: Project): LocaleBreezeLspClientDescriptor =
            project.getUserData(descriptorKey) ?: synchronized(project) {
                project.getUserData(descriptorKey) ?: LocaleBreezeLspClientDescriptor(project).also {
                    project.putUserData(descriptorKey, it)
                }
            }
    }
}

private class LocaleBreezeLspClientDescriptor(
    project: Project,
) : ProjectWideLspClientDescriptor(project, "LocaleBreeze") {
    private val log = Logger.getInstance(LocaleBreezeLspClientDescriptor::class.java)

    override fun isSupportedFile(file: VirtualFile): Boolean =
        LocaleBreezeSettings.getInstance(project).state.enabled &&
            LocaleBreezeLspIntegrationProvider.isSupported(file)

    override fun createLsp4jClient(handler: LspServerNotificationsHandler): Lsp4jClient =
        LocaleBreezeLsp4jClient(handler, project)

    override fun createCommandLine(): GeneralCommandLine {
        val executable = checkNotNull(LocaleBreezeExecutable.resolve(project)) {
            "LocaleBreeze executable disappeared before the language server started"
        }
        val command = GeneralCommandLine(executable.toString(), "lsp", "--stdio")
        project.basePath?.let(command::withWorkDirectory)
        LocaleBreezeExecutable.resolveConfig(project)?.let {
            command.addParameters("--config", it.toString())
        }
        LocaleBreezeSettings.getInstance(project).state.takeIf { it.overrideConfig }?.let {
            command.addParameters("--unused-keys", it.showUnusedKeys.toString())
        }
        log.info(
            "Starting LocaleBreeze language server: project=${project.locationHash}, " +
                "descriptor=${System.identityHashCode(this)}, executable=$executable",
        )
        return command
    }

    override val lspCustomization: LspCustomization = LocaleBreezeLspCustomization
}

private class LocaleBreezeLsp4jClient(
    handler: LspServerNotificationsHandler,
    private val project: Project,
) : Lsp4jClient(handler) {
    @JsonNotification("localeBreeze/workspaceIssue")
    fun workspaceIssue(issue: LocaleBreezeWorkspaceIssue) {
        if (issue.active) project.service<LocaleBreezeWarningCoordinator>().show(issue)
        else project.service<LocaleBreezeWarningCoordinator>().clear(
            issue.identity.takeUnless { issue.code == "clear" },
        )
    }
}

private object LocaleBreezeLspCustomization : LspCustomization() {
    override val goToDefinitionCustomizer = LspGoToDefinitionDisabled
}

private object LocaleBreezeExecutable {
    private val log = Logger.getInstance(LocaleBreezeExecutable::class.java)

    fun resolve(project: Project): Path? {
        val executable = bundledExecutable()
        if (executable == null) {
            log.warn("Could not locate the bundled LocaleBreeze executable")
            return null
        }
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

    private fun bundledExecutable(): Path? {
        val relative = Path.of("bin", platformDirectory(), executableName())
        val roots = buildList {
            pluginRoot()?.let(::add)
            add(Path.of(PathManager.getPluginsPath()).resolve("locale-breeze-jetbrains"))
        }
        return roots
            .asSequence()
            .map { it.resolve(relative).normalize() }
            .distinct()
            .firstOrNull(Files::isRegularFile)
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
