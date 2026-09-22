package dev.localebreeze.jetbrains

import com.intellij.codeInsight.daemon.DaemonCodeAnalyzer
import com.intellij.icons.AllIcons
import com.intellij.openapi.Disposable
import com.intellij.openapi.components.service
import com.intellij.openapi.fileEditor.OpenFileDescriptor
import com.intellij.openapi.options.ShowSettingsUtil
import com.intellij.openapi.project.Project
import com.intellij.openapi.util.IconLoader
import com.intellij.openapi.util.text.StringUtil
import com.intellij.openapi.vfs.LocalFileSystem
import com.intellij.openapi.wm.ToolWindow
import com.intellij.openapi.wm.ToolWindowFactory
import com.intellij.platform.lsp.api.LspClientManager
import com.intellij.ui.JBColor
import com.intellij.ui.components.JBLabel
import com.intellij.ui.components.JBScrollPane
import com.intellij.ui.content.ContentFactory
import com.intellij.util.ui.JBUI
import java.awt.Component
import java.awt.FlowLayout
import java.nio.file.Path
import javax.swing.Box
import javax.swing.BoxLayout
import javax.swing.JButton
import javax.swing.JComponent
import javax.swing.JPanel
import javax.swing.JProgressBar

class LocaleBreezeToolWindowFactory : ToolWindowFactory {
    override fun createToolWindowContent(project: Project, toolWindow: ToolWindow) {
        val dashboard = LocaleBreezeToolWindowPanel(project, toolWindow)
        val content = ContentFactory.getInstance().createContent(dashboard.component, "", false)
        content.setDisposer(dashboard)
        toolWindow.contentManager.addContent(content)
    }
}

private class LocaleBreezeToolWindowPanel(
    private val project: Project,
    private val toolWindow: ToolWindow,
) : Disposable {
    private val body = JPanel()
    val component: JComponent = JBScrollPane(body).apply { border = JBUI.Borders.empty() }
    private val model = project.service<LocaleBreezeWarningCoordinator>()
    private val normalIcon = IconLoader.getIcon("/META-INF/pluginIcon.svg", javaClass)

    init {
        body.layout = BoxLayout(body, BoxLayout.Y_AXIS)
        body.border = JBUI.Borders.empty(12)
        model.addListener(this, ::render)
        render()
    }

    override fun dispose() = Unit

    private fun render() {
        val state = model.snapshot()
        body.removeAll()
        toolWindow.setIcon(if (state.issues.isEmpty()) normalIcon else AllIcons.General.Warning)

        when {
            state.issues.isNotEmpty() -> renderProblems(state)
            state.lifecycle == LocaleBreezeLifecycle.DISABLED -> renderDisabled()
            state.lifecycle == LocaleBreezeLifecycle.STARTING -> renderStarting()
            state.lifecycle == LocaleBreezeLifecycle.UNAVAILABLE -> renderUnavailable()
            else -> renderHealthy(state.status)
        }

        body.add(Box.createVerticalStrut(12))
        body.add(commonActions(state.lifecycle))
        body.add(Box.createVerticalGlue())
        body.revalidate()
        body.repaint()
    }

    private fun renderDisabled() {
        heading("LocaleBreeze is disabled")
        paragraph("Enable LocaleBreeze to index translations and show workspace health.")
    }

    private fun renderStarting() {
        heading("Starting LocaleBreeze…")
        paragraph("The language server is indexing this workspace.")
        body.add(JProgressBar().apply { isIndeterminate = true; alignmentX = Component.LEFT_ALIGNMENT })
    }

    private fun renderUnavailable() {
        heading("LocaleBreeze is unavailable")
        paragraph("Refresh the workspace or open settings to check the configuration.")
    }

    private fun renderHealthy(status: LocaleBreezeWorkspaceStatus?) {
        heading("Workspace is healthy")
        if (status == null) {
            paragraph("Waiting for workspace information.")
            return
        }
        metric(status.unusedKeyCount.toString(), "Unused keys")
        metric(status.totalKeyCount.toString(), "Default-locale keys")
        metric(status.dictionaryFileCount.toString(), "Dictionaries")
        paragraph("Default locale: ${status.defaultLocale}")
        fileActions(status)
    }

    private fun renderProblems(state: LocaleBreezeDashboardState) {
        heading("Problems detected")
        paragraph("${state.issues.size} ${if (state.issues.size == 1) "problem needs" else "problems need"} attention.")
        state.status?.let {
            metric(it.unusedKeyCount.toString(), "Unused keys")
            fileActions(it)
        }
        state.issues.forEach { issue ->
            body.add(Box.createVerticalStrut(10))
            body.add(JPanel().apply {
                layout = BoxLayout(this, BoxLayout.Y_AXIS)
                alignmentX = Component.LEFT_ALIGNMENT
                border = JBUI.Borders.compound(
                    JBUI.Borders.customLine(JBColor.border(), 1),
                    JBUI.Borders.empty(8),
                )
                add(JBLabel("<html><b>${html(issue.summary)}</b></html>"))
                add(JBLabel("<html><div width='260'>${html(issue.remediation)}</div></html>"))
                issue.path?.let { path ->
                    add(JBLabel("<html><small>${html(locationText(issue))}</small></html>"))
                    add(button("Open file") { openPath(path, issue.line, issue.column) })
                }
            })
        }
    }

    private fun fileActions(status: LocaleBreezeWorkspaceStatus) {
        body.add(Box.createVerticalStrut(8))
        body.add(row(
            button("Open config") { openPath(status.configPath) },
            button("Open dictionary", status.defaultDictionaryPath != null) {
                status.defaultDictionaryPath?.let(::openPath)
            },
        ))
    }

    private fun commonActions(lifecycle: LocaleBreezeLifecycle): JComponent = row(
        button("Refresh", lifecycle != LocaleBreezeLifecycle.DISABLED) { refresh() },
        button("Settings") {
            ShowSettingsUtil.getInstance().showSettingsDialog(project, LocaleBreezeConfigurable::class.java)
        },
        button(if (lifecycle == LocaleBreezeLifecycle.DISABLED) "Enable" else "Disable") {
            setEnabled(lifecycle == LocaleBreezeLifecycle.DISABLED)
        },
    )

    private fun refresh() {
        model.starting()
        LspClientManager.getInstance(project)
            .stopAndRestartClientsIfNeeded(LocaleBreezeLspIntegrationProvider::class.java)
        project.service<LocaleBreezeKeyCache>().invalidate()
        DaemonCodeAnalyzer.getInstance(project).restart()
    }

    private fun setEnabled(value: Boolean) {
        val settings = LocaleBreezeSettings.getInstance(project)
        settings.setActivationMode(
            if (value) LocaleBreezeSettings.ActivationMode.ENABLED
            else LocaleBreezeSettings.ActivationMode.DISABLED,
        )
        settings.state.enabled = value
        if (value) model.starting() else model.disabled()
        LspClientManager.getInstance(project)
            .stopAndRestartClientsIfNeeded(LocaleBreezeLspIntegrationProvider::class.java)
        project.service<LocaleBreezeKeyCache>().invalidate()
        DaemonCodeAnalyzer.getInstance(project).restart()
    }

    private fun openPath(value: String, line: Int? = null, column: Int? = null) {
        val file = runCatching { Path.of(value) }.getOrNull()
            ?.let { LocalFileSystem.getInstance().refreshAndFindFileByNioFile(it) }
            ?: return
        OpenFileDescriptor(project, file, (line ?: 1) - 1, (column ?: 1) - 1).navigate(true)
    }

    private fun heading(text: String) {
        body.add(JBLabel("<html><h2>${html(text)}</h2></html>").apply {
            alignmentX = Component.LEFT_ALIGNMENT
        })
    }

    private fun paragraph(text: String) {
        body.add(JBLabel("<html><div width='280'>${html(text)}</div></html>").apply {
            alignmentX = Component.LEFT_ALIGNMENT
        })
        body.add(Box.createVerticalStrut(8))
    }

    private fun metric(value: String, label: String) {
        body.add(JBLabel("<html><b>${html(value)}</b>&nbsp;&nbsp;${html(label)}</html>").apply {
            alignmentX = Component.LEFT_ALIGNMENT
        })
    }

    private fun row(vararg components: JComponent): JComponent = JPanel(FlowLayout(FlowLayout.LEFT, 0, 4)).apply {
        alignmentX = Component.LEFT_ALIGNMENT
        isOpaque = false
        components.forEachIndexed { index, component ->
            if (index > 0) add(Box.createHorizontalStrut(6))
            add(component)
        }
    }

    private fun button(text: String, enabled: Boolean = true, action: () -> Unit): JButton =
        JButton(text).apply {
            isEnabled = enabled
            alignmentX = Component.LEFT_ALIGNMENT
            addActionListener { action() }
        }

    private fun locationText(issue: LocaleBreezeWorkspaceIssue): String = buildString {
        append(issue.path)
        issue.line?.let {
            append(":").append(it)
            issue.column?.let { column -> append(":").append(column) }
        }
    }

    private fun html(value: String): String = StringUtil.escapeXmlEntities(value)
}
