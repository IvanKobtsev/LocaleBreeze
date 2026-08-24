package dev.localebreeze.jetbrains

import com.intellij.openapi.application.ApplicationManager
import com.intellij.openapi.project.Project
import com.intellij.openapi.startup.ProjectActivity
import com.intellij.openapi.vfs.VirtualFileManager
import com.intellij.openapi.vfs.newvfs.BulkFileListener
import com.intellij.openapi.vfs.newvfs.events.VFileEvent
import com.intellij.platform.lsp.api.LspClientManager
import java.nio.file.Path

class LocaleBreezeProjectActivity : ProjectActivity {
    override suspend fun execute(project: Project) {
        project.messageBus.connect(project).subscribe(
            VirtualFileManager.VFS_CHANGES,
            object : BulkFileListener {
                override fun after(events: List<VFileEvent>) {
                    if (events.none { isConfigurationFile(project, it.path) }) return
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
