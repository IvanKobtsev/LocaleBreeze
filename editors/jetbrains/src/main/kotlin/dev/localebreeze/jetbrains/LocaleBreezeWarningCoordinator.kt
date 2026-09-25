package dev.localebreeze.jetbrains

import com.intellij.notification.NotificationAction
import com.intellij.notification.NotificationGroupManager
import com.intellij.notification.NotificationType
import com.intellij.icons.AllIcons
import com.intellij.openapi.Disposable
import com.intellij.openapi.application.ApplicationManager
import com.intellij.openapi.components.Service
import com.intellij.openapi.project.Project
import com.intellij.openapi.util.Disposer
import com.intellij.openapi.util.IconLoader
import com.intellij.openapi.wm.ToolWindowManager
import java.nio.file.Path
import java.util.concurrent.CopyOnWriteArrayList

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

data class LocaleBreezeWorkspaceStatus(
    val version: Int = 1,
    val workspaceRoot: String = "",
    val configPath: String = "",
    val defaultLocale: String = "",
    val defaultDictionaryPath: String? = null,
    val dictionaryFileCount: Int = 0,
    val totalKeyCount: Int = 0,
    val unusedKeyCount: Int = 0,
    val generation: Long = 0,
)

enum class LocaleBreezeLifecycle { DISABLED, WAITING, STARTING, READY, UNAVAILABLE }

data class LocaleBreezeDashboardState(
    val lifecycle: LocaleBreezeLifecycle,
    val setup: LocaleBreezeConfigSetupState,
    val status: LocaleBreezeWorkspaceStatus? = null,
    val issues: List<LocaleBreezeWorkspaceIssue> = emptyList(),
)

@Service(Service.Level.PROJECT)
class LocaleBreezeWarningCoordinator(private val project: Project) {
    private val listeners = CopyOnWriteArrayList<() -> Unit>()
    private val issues = linkedMapOf<String, LocaleBreezeWorkspaceIssue>()
    private var lifecycle = if (LocaleBreezeSettings.getInstance(project).state.enabled) {
        LocaleBreezeLifecycle.WAITING
    } else {
        LocaleBreezeLifecycle.DISABLED
    }
    private var workspaceStatus: LocaleBreezeWorkspaceStatus? = null
    private var setupState: LocaleBreezeConfigSetupState = LocaleBreezeConfigSetupState.NotFound
    private val normalToolIcon = IconLoader.getIcon("/icons/tool_window_icon/default/localeBreeze.svg", javaClass)
    private val fadedToolIcon = IconLoader.getIcon("/icons/tool_window_icon/disabled/localeBreeze.svg", javaClass)

    @Synchronized
    fun snapshot(): LocaleBreezeDashboardState = LocaleBreezeDashboardState(
        lifecycle = lifecycle,
        setup = setupState,
        status = workspaceStatus,
        issues = issues.values.sortedWith(compareBy({ it.path.orEmpty() }, { it.code })),
    )

    fun addListener(parent: Disposable, listener: () -> Unit) {
        listeners += listener
        Disposer.register(parent) { listeners -= listener }
    }

    @Synchronized
    fun setup(state: LocaleBreezeConfigSetupState) {
        if (setupState == state) return
        setupState = state
        changed()
    }

    @Synchronized
    fun starting() {
        issues.clear()
        workspaceStatus = null
        lifecycle = LocaleBreezeLifecycle.STARTING
        changed()
    }

    @Synchronized
    fun startingIfNeeded() {
        if (lifecycle != LocaleBreezeLifecycle.STARTING && lifecycle != LocaleBreezeLifecycle.READY) {
            starting()
        }
    }

    @Synchronized
    fun waiting() {
        issues.clear()
        workspaceStatus = null
        lifecycle = LocaleBreezeLifecycle.WAITING
        changed()
    }

    @Synchronized
    fun disabled() {
        issues.clear()
        workspaceStatus = null
        lifecycle = LocaleBreezeLifecycle.DISABLED
        changed()
    }

    @Synchronized
    fun unavailable(issue: LocaleBreezeWorkspaceIssue) {
        lifecycle = LocaleBreezeLifecycle.UNAVAILABLE
        addIssue(issue)
    }

    @Synchronized
    fun status(status: LocaleBreezeWorkspaceStatus) {
        if (workspaceStatus?.generation?.let { status.generation < it } == true) return
        workspaceStatus = status
        lifecycle = LocaleBreezeLifecycle.READY
        changed()
    }

    @Synchronized
    fun ready() {
        if (lifecycle == LocaleBreezeLifecycle.STARTING || lifecycle == LocaleBreezeLifecycle.WAITING) {
            lifecycle = LocaleBreezeLifecycle.READY
            changed()
        }
    }

    @Synchronized
    fun show(issue: LocaleBreezeWorkspaceIssue) {
        if (!LocaleBreezeSettings.getInstance(project).state.enabled) return
        if (workspaceStatus == null) lifecycle = LocaleBreezeLifecycle.UNAVAILABLE
        addIssue(issue)
    }

    @Synchronized
    private fun addIssue(issue: LocaleBreezeWorkspaceIssue) {
        val wasHealthy = issues.isEmpty()
        if (issues[issue.identity] == issue) return
        issues[issue.identity] = issue
        changed()
        if (wasHealthy) showAttentionNotification(issue.summary)
    }

    @Synchronized
    fun clear(identity: String? = null) {
        val didChange = if (identity == null) {
            val hadIssues = issues.isNotEmpty()
            issues.clear()
            hadIssues
        } else {
            issues.remove(identity) != null
        }
        if (didChange) changed()
    }

    @Synchronized
    fun concerns(path: String): Boolean = issues.values.any { issue ->
        issue.path?.let {
            runCatching { Path.of(it).normalize() == Path.of(path).normalize() }.getOrDefault(false)
        } == true
    }

    private fun changed() {
        ApplicationManager.getApplication().invokeLater {
            if (project.isDisposed) return@invokeLater
            val state = snapshot()
            ToolWindowManager.getInstance(project).getToolWindow("LocaleBreeze")?.setIcon(
                when {
                    state.issues.isNotEmpty() -> AllIcons.General.Warning
                    !LocaleBreezeSettings.getInstance(project).isEnabledInSettings() -> fadedToolIcon
                    else -> normalToolIcon
                },
            )
            listeners.forEach { it() }
        }
    }

    private fun showAttentionNotification(summary: String) {
        ApplicationManager.getApplication().invokeLater {
            if (project.isDisposed) return@invokeLater
            NotificationGroupManager.getInstance().getNotificationGroup("LocaleBreeze")
                .createNotification(
                    "LocaleBreeze needs attention",
                    summary,
                    NotificationType.WARNING,
                )
                .addAction(NotificationAction.createSimpleExpiring("Open LocaleBreeze") {
                    ToolWindowManager.getInstance(project).getToolWindow("LocaleBreeze")?.show(null)
                })
                .notify(project)
        }
    }
}
