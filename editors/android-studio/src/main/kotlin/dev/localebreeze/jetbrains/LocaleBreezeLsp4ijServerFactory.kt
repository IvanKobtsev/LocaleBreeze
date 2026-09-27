package dev.localebreeze.androidstudio

import com.intellij.execution.configurations.GeneralCommandLine
import com.intellij.openapi.application.PathManager
import com.intellij.openapi.components.service
import com.intellij.openapi.diagnostic.Logger
import com.intellij.openapi.project.Project
import com.intellij.openapi.util.SystemInfoRt
import com.redhat.devtools.lsp4ij.LanguageServerEnablementSupport
import com.redhat.devtools.lsp4ij.LanguageServerFactory
import com.redhat.devtools.lsp4ij.LanguageServerManager
import com.redhat.devtools.lsp4ij.client.LanguageClientImpl
import com.redhat.devtools.lsp4ij.server.OSProcessStreamConnectionProvider
import com.redhat.devtools.lsp4ij.server.StreamConnectionProvider
import java.nio.file.Files
import java.nio.file.Path
import java.util.concurrent.TimeUnit
import org.eclipse.lsp4j.ExecuteCommandParams
import org.eclipse.lsp4j.jsonrpc.services.JsonNotification
import org.eclipse.lsp4j.services.LanguageServer

internal const val LOCALE_BREEZE_SERVER_ID = "locale-breeze"
internal val LOCALE_BREEZE_EXTENSIONS = setOf("js", "jsx", "ts", "tsx", "json")

internal fun isLocaleBreezeExtension(extension: String?): Boolean =
    extension?.lowercase() in LOCALE_BREEZE_EXTENSIONS

class LocaleBreezeLsp4ijServerFactory : LanguageServerFactory, LanguageServerEnablementSupport {
    override fun createConnectionProvider(project: Project): StreamConnectionProvider {
        val executable = checkNotNull(LocaleBreezeExecutable.resolve(project)) {
            "LocaleBreeze language-server executable is missing"
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
        return OSProcessStreamConnectionProvider(command)
    }

    override fun createLanguageClient(project: Project): LanguageClientImpl = LocaleBreezeLsp4ijClient(project)

    override fun isEnabled(project: Project): Boolean =
        LocaleBreezeSettings.getInstance(project).state.enabled && LocaleBreezeExecutable.resolve(project) != null

    override fun setEnabled(enabled: Boolean, project: Project) {
        LocaleBreezeSettings.getInstance(project).state.enabled = enabled
    }
}

private class LocaleBreezeLsp4ijClient(private val project: Project) : LanguageClientImpl(project) {
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

internal object LocaleBreezeLsp4ij {
    fun manager(project: Project): LanguageServerManager = LanguageServerManager.getInstance(project)

    fun start(project: Project) {
        manager(project).start(LOCALE_BREEZE_SERVER_ID)
    }

    fun stop(project: Project) {
        manager(project).stop(
            LOCALE_BREEZE_SERVER_ID,
            LanguageServerManager.StopOptions().apply { setWillDisable(false) },
        )
    }

    fun restart(project: Project) {
        stop(project)
        start(project)
    }

    fun server(project: Project, timeoutMillis: Long = 2_500): LanguageServer? =
        manager(project).getLanguageServer(LOCALE_BREEZE_SERVER_ID)
            .get(timeoutMillis, TimeUnit.MILLISECONDS)
            ?.server

    fun execute(project: Project, command: String, arguments: List<Any>, timeoutMillis: Long = 2_500): Any? =
        server(project, timeoutMillis)
            ?.workspaceService
            ?.executeCommand(ExecuteCommandParams(command, arguments))
            ?.get(timeoutMillis, TimeUnit.MILLISECONDS)
}

internal object LocaleBreezeExecutable {
    private val log = Logger.getInstance(LocaleBreezeExecutable::class.java)

    fun resolve(project: Project): Path? {
        val executable = configuredDevelopmentServer(project) ?: bundledExecutable()
        if (executable == null || !Files.isRegularFile(executable)) {
            log.warn("Could not locate the LocaleBreeze executable: $executable")
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
                    Files.isRegularFile(candidate.resolve("Cargo.toml")) && Files.isDirectory(candidate.resolve("editors"))
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
        return settings.state.configPath.takeIf(String::isNotBlank)?.let { resolveProjectPath(project, it) }
    }

    private fun resolveProjectPath(project: Project, value: String): Path {
        val path = Path.of(value)
        if (path.isAbsolute) return path.normalize()
        return (project.basePath?.let(Path::of) ?: Path.of(PathManager.getSystemPath())).resolve(path).normalize()
    }

    private fun bundledExecutable(): Path? {
        val relative = Path.of("bin", platformDirectory(System.getProperty("os.name"), System.getProperty("os.arch")), executableName())
        return sequenceOf(pluginRoot(), Path.of(PathManager.getPluginsPath()).resolve("locale-breeze-android-studio"))
            .filterNotNull()
            .map { it.resolve(relative).normalize() }
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

    internal fun platformDirectory(osName: String, architectureName: String): String {
        val os = when {
            osName.startsWith("Windows", ignoreCase = true) -> "win32"
            osName.startsWith("Mac", ignoreCase = true) -> "darwin"
            else -> "linux"
        }
        val architecture = if (architectureName.lowercase() in setOf("aarch64", "arm64")) "arm64" else "x64"
        return "$os-$architecture"
    }

    private fun executableName(): String = if (SystemInfoRt.isWindows) "locale-breeze.exe" else "locale-breeze"
}

internal object LocaleBreezeDevelopment {
    private const val PROPERTY = "dev.localebreeze.development"
    private const val SERVER_PROPERTY = "dev.localebreeze.server"
    val enabled: Boolean get() = java.lang.Boolean.getBoolean(PROPERTY)
    val serverPath: Path? get() = System.getProperty(SERVER_PROPERTY)?.takeIf(String::isNotBlank)?.let(Path::of)
}
