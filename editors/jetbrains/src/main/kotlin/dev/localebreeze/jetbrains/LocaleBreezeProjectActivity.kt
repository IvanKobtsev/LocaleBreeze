package dev.localebreeze.jetbrains

import com.intellij.openapi.application.ApplicationManager
import com.intellij.openapi.project.Project
import com.intellij.openapi.startup.ProjectActivity
import com.intellij.openapi.vfs.VirtualFileManager
import com.intellij.openapi.vfs.newvfs.BulkFileListener
import com.intellij.openapi.vfs.newvfs.events.VFileEvent
import com.intellij.platform.lsp.api.LspClientManager
import com.intellij.openapi.components.service

class LocaleBreezeProjectActivity : ProjectActivity {
    override suspend fun execute(project: Project) {
        project.service<LocaleBreezeConfigDiscovery>().refresh()
        project.messageBus.connect(project).subscribe(
            VirtualFileManager.VFS_CHANGES,
            object : BulkFileListener {
                override fun after(events: List<VFileEvent>) {
                    val discovery = project.service<LocaleBreezeConfigDiscovery>()
                    val configChanged = events.any { discovery.concerns(it.path) }
                    val issueChanged = events.any { project.service<LocaleBreezeWarningCoordinator>().concerns(it.path) }
                    if (!configChanged && !issueChanged) return
                    ApplicationManager.getApplication().invokeLater {
                        if (!project.isDisposed) {
                            if (configChanged) project.service<LocaleBreezeConfigDiscovery>().scheduleRefresh(true)
                            else LspClientManager.getInstance(project).stopAndRestartClientsIfNeeded(
                                LocaleBreezeLspIntegrationProvider::class.java,
                            )
                        }
                    }
                }
            },
        )
    }

}
