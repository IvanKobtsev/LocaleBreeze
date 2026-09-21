package dev.localebreeze.jetbrains

import com.intellij.notification.Notification
import com.intellij.notification.NotificationAction
import com.intellij.notification.NotificationGroupManager
import com.intellij.notification.NotificationType
import com.intellij.openapi.components.Service
import com.intellij.openapi.fileEditor.OpenFileDescriptor
import com.intellij.openapi.options.ShowSettingsUtil
import com.intellij.openapi.project.Project
import com.intellij.openapi.vfs.LocalFileSystem
import java.nio.file.Path

data class LocaleBreezeWorkspaceIssue(
    val version: Int = 1,
    val active: Boolean = true,
    val code: String = "internal",
    val summary: String = "LocaleBreeze could not start",
    val remediation: String = "Check the LocaleBreeze configuration and try again.",
    val path: String? = null,
    val line: Int? = null,
    val column: Int? = null,
) {
    val identity: String get() = "$code:${path.orEmpty()}"
}

@Service(Service.Level.PROJECT)
class LocaleBreezeWarningCoordinator(private val project: Project) {
    private var currentIdentity: String? = null
    private var currentIssue: LocaleBreezeWorkspaceIssue? = null
    private var current: Notification? = null

    @Synchronized
    fun show(issue: LocaleBreezeWorkspaceIssue) {
        if (!LocaleBreezeSettings.getInstance(project).state.enabled) return
        if (currentIssue == issue && current?.isExpired == false) return
        clear()
        val location = buildString {
            issue.path?.let { append("<br><code>").append(it).append("</code>") }
            issue.line?.let {
                append(" (line ").append(it)
                issue.column?.let { column -> append(", column ").append(column) }
                append(")")
            }
        }
        currentIdentity = issue.identity
        currentIssue = issue
        current = NotificationGroupManager.getInstance()
            .getNotificationGroup("LocaleBreeze")
            .createNotification(
                issue.summary,
                issue.remediation + location,
                NotificationType.WARNING,
            )
            .addAction(NotificationAction.createSimpleExpiring("Open Settings") {
                ShowSettingsUtil.getInstance().showSettingsDialog(project, LocaleBreezeConfigurable::class.java)
            })
            .also { notification ->
                issue.path?.let { value ->
                    notification.addAction(NotificationAction.createSimpleExpiring("Open File") {
                        val file = runCatching { Path.of(value) }.getOrNull()
                            ?.let { LocalFileSystem.getInstance().refreshAndFindFileByNioFile(it) }
                        if (file != null) OpenFileDescriptor(project, file, (issue.line ?: 1) - 1, (issue.column ?: 1) - 1).navigate(true)
                    })
                }
                notification.notify(project)
            }
    }

    @Synchronized
    fun clear(identity: String? = null) {
        if (identity != null && identity != currentIdentity) return
        current?.expire()
        current = null
        currentIdentity = null
        currentIssue = null
    }

    @Synchronized
    fun concerns(path: String): Boolean = currentIssue?.path?.let {
        runCatching { Path.of(it).normalize() == Path.of(path).normalize() }.getOrDefault(false)
    } == true
}
