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
        project.service<LocaleBreezeWarningCoordinator>().startingIfNeeded()
        if (LocaleBreezeExecutable.resolve(project) == null) {
            val developmentServer = LocaleBreezeExecutable.configuredDevelopmentServer(project)
            project.service<LocaleBreezeWarningCoordinator>().unavailable(
                LocaleBreezeWorkspaceIssue(
                    code = "server_missing",
                    summary = "LocaleBreeze language server is missing",
                    remediation = if (developmentServer == null) {
                        "Reinstall the LocaleBreeze plugin to restore its bundled language-server executable."
                    } else {
                        "Choose an existing development language-server executable in LocaleBreeze settings."
                    },
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
        command.addParameters(
            "--unused-keys",
            LocaleBreezePreferences.getInstance().state.showUnusedKeys.toString(),
        )
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

    @JsonNotification("localeBreeze/workspaceStatus")
    fun workspaceStatus(status: LocaleBreezeWorkspaceStatus) {
        val settings = LocaleBreezeSettings.getInstance(project)
        if (settings.configLocation() == LocaleBreezeSettings.ConfigLocation.WORKSPACE_ROOT && status.configPath.isNotBlank()) {
            settings.state.configPath = status.configPath
        }
        project.service<LocaleBreezeContentRoots>().reconcile(status.dictionaryRootPath)
        project.service<LocaleBreezeWarningCoordinator>().status(status)
    }
}

private object LocaleBreezeLspCustomization : LspCustomization() {
    override val goToDefinitionCustomizer = LspGoToDefinitionDisabled
}

private object LocaleBreezeExecutable {
    private val log = Logger.getInstance(LocaleBreezeExecutable::class.java)

    fun resolve(project: Project): Path? {
        val configured = configuredDevelopmentServer(project)
        val executable = configured ?: bundledExecutable()
        if (executable == null) {
            log.warn("Could not locate the bundled LocaleBreeze executable")
            return null
        }
        if (!Files.isRegularFile(executable)) {
            log.warn("LocaleBreeze executable does not exist: $executable")
            return null
        }
        if (!SystemInfoRt.isWindows && !executable.toFile().setExecutable(true)) {
            log.warn("Could not mark LocaleBreeze executable as executable: $executable")
        }
        return executable
    }

    fun configuredDevelopmentServer(project: Project): Path? {
        if (!LocaleBreezeDevelopment.enabled) return null
        val configured = LocaleBreezeSettings.getInstance(project).state.developmentServerPath
        if (configured.isNotBlank()) return resolveProjectPath(project, configured)
        LocaleBreezeDevelopment.serverPath?.let { return it }

        val executable = executableName()
        val repositoryRoot = runCatching {
            Path.of(LocaleBreezeExecutable::class.java.protectionDomain.codeSource.location.toURI())
        }.getOrNull()?.let { location ->
            generateSequence(if (Files.isRegularFile(location)) location.parent else location) { it.parent }
                .take(10)
                .firstOrNull { candidate ->
                    Files.isRegularFile(candidate.resolve("Cargo.toml")) &&
                        Files.isDirectory(candidate.resolve("editors").resolve("jetbrains"))
                }
        }
        return sequenceOf(
            repositoryRoot?.resolve(Path.of("target", "debug", executable)),
            project.basePath?.let(Path::of)?.resolve(Path.of("target", "debug", executable)),
        ).filterNotNull().firstOrNull(Files::isRegularFile)
    }

    fun resolveConfig(project: Project): Path? {
        val settings = LocaleBreezeSettings.getInstance(project)
        if (settings.configLocation() == LocaleBreezeSettings.ConfigLocation.WORKSPACE_ROOT) return null
        val configured = settings.state.configPath
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

internal object LocaleBreezeDevelopment {
    private const val PROPERTY = "dev.localebreeze.development"
    private const val SERVER_PROPERTY = "dev.localebreeze.server"
    val enabled: Boolean
        get() = java.lang.Boolean.getBoolean(PROPERTY)
    val serverPath: Path?
        get() = System.getProperty(SERVER_PROPERTY)?.takeIf(String::isNotBlank)?.let(Path::of)
}
