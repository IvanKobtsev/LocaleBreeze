package dev.localebreeze.jetbrains

import com.intellij.openapi.application.ApplicationManager
import com.intellij.openapi.project.Project
import com.intellij.openapi.startup.ProjectActivity
import com.intellij.openapi.vfs.VirtualFileManager
import com.intellij.openapi.vfs.newvfs.BulkFileListener
import com.intellij.openapi.vfs.newvfs.events.VFileEvent
import com.intellij.platform.lsp.api.LspClientManager
import com.intellij.notification.NotificationAction
import com.intellij.notification.NotificationGroupManager
import com.intellij.notification.NotificationType
import com.intellij.openapi.components.service
import com.intellij.openapi.options.ShowSettingsUtil
import java.nio.file.Files
import java.nio.file.Path

class LocaleBreezeProjectActivity : ProjectActivity {
    override suspend fun execute(project: Project) {
        offerEnablement(project)
        project.messageBus.connect(project).subscribe(
            VirtualFileManager.VFS_CHANGES,
            object : BulkFileListener {
                override fun after(events: List<VFileEvent>) {
                    if (!LocaleBreezeSettings.getInstance(project).state.enabled) return
                    if (events.none {
                        isConfigurationFile(project, it.path) ||
                            project.service<LocaleBreezeWarningCoordinator>().concerns(it.path)
                    }) return
                    ApplicationManager.getApplication().invokeLater {
                        if (!project.isDisposed) {
                            LspClientManager.getInstance(project).stopAndRestartClientsIfNeeded(
                                LocaleBreezeLspIntegrationProvider::class.java,
                            )
                        }
                    }
                }
            },
        )
    }

    private fun offerEnablement(project: Project) {
        val state = LocaleBreezeSettings.getInstance(project).state
        if (state.enabled || state.enablePromptDismissed) return
        val config = effectiveConfigPath(project) ?: return
        if (!Files.isRegularFile(config)) return
        state.enablePromptDismissed = true
        NotificationGroupManager.getInstance().getNotificationGroup("LocaleBreeze")
            .createNotification(
                "Enable LocaleBreeze for this workspace?",
                "A <code>locale-breeze.json</code> configuration was found.",
                NotificationType.INFORMATION,
            )
            .addAction(NotificationAction.createSimpleExpiring("Enable") {
                state.enabled = true
                LspClientManager.getInstance(project).stopAndRestartClientsIfNeeded(
                    LocaleBreezeLspIntegrationProvider::class.java,
                )
                project.service<LocaleBreezeKeyCache>().invalidate()
                com.intellij.codeInsight.daemon.DaemonCodeAnalyzer.getInstance(project).restart()
            })
            .addAction(NotificationAction.createSimpleExpiring("Open Settings") {
                ShowSettingsUtil.getInstance().showSettingsDialog(project, LocaleBreezeConfigurable::class.java)
            })
            .notify(project)
    }

    private fun effectiveConfigPath(project: Project): Path? {
        val configured = LocaleBreezeSettings.getInstance(project).state.configPath
        if (configured.isBlank()) return project.basePath?.let(Path::of)?.resolve("locale-breeze.json")
        return Path.of(configured).let { if (it.isAbsolute) it else project.basePath?.let(Path::of)?.resolve(it) }
    }

    private fun isConfigurationFile(project: Project, changedPath: String): Boolean {
        val normalizedChanged = Path.of(changedPath).normalize()
        val configured = LocaleBreezeSettings.getInstance(project).state.configPath
        if (configured.isNotBlank()) {
            val configuredPath = Path.of(configured).let { path ->
                if (path.isAbsolute) path else project.basePath?.let(Path::of)?.resolve(path) ?: path
            }.normalize()
            return pathsEqual(normalizedChanged, configuredPath)
        }
        val defaultPath = project.basePath?.let(Path::of)?.resolve("locale-breeze.json")?.normalize()
        return defaultPath != null && pathsEqual(normalizedChanged, defaultPath)
    }

    private fun pathsEqual(left: Path, right: Path): Boolean =
        if (System.getProperty("os.name").startsWith("Windows", ignoreCase = true)) {
            left.toString().equals(right.toString(), ignoreCase = true)
        } else {
            left == right
        }
}
