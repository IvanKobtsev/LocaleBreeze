package dev.localebreeze.jetbrains

import com.intellij.execution.configurations.GeneralCommandLine
import com.intellij.openapi.components.service
import com.intellij.openapi.project.Project
import com.redhat.devtools.lsp4ij.LanguageServerEnablementSupport
import com.redhat.devtools.lsp4ij.LanguageServerFactory
import com.redhat.devtools.lsp4ij.client.LanguageClientImpl
import com.redhat.devtools.lsp4ij.server.OSProcessStreamConnectionProvider
import com.redhat.devtools.lsp4ij.server.StreamConnectionProvider
import org.eclipse.lsp4j.jsonrpc.services.JsonNotification
import java.nio.file.Files
import java.nio.file.Path

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

    override fun createLanguageClient(project: Project): LanguageClientImpl =
        LocaleBreezeLsp4ijClient(project)

    override fun isEnabled(project: Project): Boolean =
        !hasBuiltInLsp() &&
            hasConfiguration(project) &&
            LocaleBreezeExecutable.resolve(project) != null

    override fun setEnabled(enabled: Boolean, project: Project) {
        LocaleBreezeSettings.getInstance(project).state.enabled = enabled
    }

    private fun hasConfiguration(project: Project): Boolean {
        val settings = LocaleBreezeSettings.getInstance(project)
        if (settings.activationMode() == LocaleBreezeSettings.ActivationMode.DISABLED) return false
        LocaleBreezeExecutable.resolveConfig(project)?.let { return Files.isRegularFile(it) }
        return project.basePath
            ?.let(Path::of)
            ?.resolve(LocaleBreezeConfigDiscovery.CONFIG_FILE_NAME)
            ?.let(Files::isRegularFile)
            ?: false
    }

    private fun hasBuiltInLsp(): Boolean = runCatching {
        Class.forName("com.intellij.platform.lsp.api.LspClientManager", false, javaClass.classLoader)
    }.isSuccess
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
        if (settings.configLocation() == LocaleBreezeSettings.ConfigLocation.WORKSPACE_ROOT &&
            status.configPath.isNotBlank()
        ) {
            settings.state.configPath = status.configPath
        }
        project.service<LocaleBreezeContentRoots>().reconcile(status.dictionaryRootPath)
        project.service<LocaleBreezeWarningCoordinator>().status(status)
    }
}
