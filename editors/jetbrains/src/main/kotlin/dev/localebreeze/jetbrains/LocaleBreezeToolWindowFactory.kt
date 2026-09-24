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
import java.awt.AlphaComposite
import java.awt.BorderLayout
import java.awt.Color
import java.awt.Component
import java.awt.Dimension
import java.awt.FlowLayout
import java.awt.Graphics
import java.awt.Graphics2D
import java.awt.Insets
import java.awt.Rectangle
import java.awt.RenderingHints
import java.nio.file.Path
import javax.swing.Box
import javax.swing.BoxLayout
import javax.swing.JButton
import javax.swing.JComponent
import javax.swing.JPanel
import javax.swing.JProgressBar
import javax.swing.Scrollable
import javax.swing.JTextArea

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
    private val body = ToolWindowBody()
    val component: JComponent = JBScrollPane(body).apply { border = JBUI.Borders.empty() }
    private val model = project.service<LocaleBreezeWarningCoordinator>()
    private val normalToolIcon = IconLoader.getIcon("/icons/tool_window_icon/default/localeBreeze.svg", javaClass)
    private val fadedToolIcon = IconLoader.getIcon("/icons/tool_window_icon/disabled/localeBreeze.svg", javaClass)
    private val openDictionaryIcon = IconLoader.getIcon("/icons/open_translation_dictionary/localeBreeze.svg", javaClass)
    private val configFileIcon = IconLoader.getIcon("/icons/config_file/localeBreeze.svg", javaClass)
    private val settingsIcon = IconLoader.getIcon("/icons/settings_icon/localeBreeze.svg", javaClass)
    private val powerIcon = IconLoader.getIcon("/icons/power_icon/localeBreeze.svg", javaClass)
    private val restartIcon = IconLoader.getIcon("/icons/restart_icon/localeBreeze.svg", javaClass)

    init {
        body.layout = BoxLayout(body, BoxLayout.Y_AXIS)
        body.border = JBUI.Borders.empty(28, 30)
        model.addListener(this, ::render)
        render()
    }

    override fun dispose() = Unit

    private fun render() {
        val state = model.snapshot()
        body.removeAll()

        if (state.issues.isNotEmpty())
            toolWindow.setIcon(AllIcons.General.Warning)
        else if (state.lifecycle == LocaleBreezeLifecycle.DISABLED)
            toolWindow.setIcon(fadedToolIcon)
        else toolWindow.setIcon(normalToolIcon)

        body.add(toolBar(state))
        body.add(Box.createVerticalStrut(40))

        when {
            state.issues.isNotEmpty() -> renderProblems(state)
            state.lifecycle == LocaleBreezeLifecycle.DISABLED -> renderDisabled()
            state.lifecycle == LocaleBreezeLifecycle.WAITING -> renderWaiting()
            state.lifecycle == LocaleBreezeLifecycle.STARTING -> renderStarting()
            state.lifecycle == LocaleBreezeLifecycle.UNAVAILABLE -> renderUnavailable()
            else -> renderHealthy(state.status)
        }

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

    private fun renderWaiting() {
        heading("LocaleBreeze is ready")
        paragraph("Open a JavaScript, TypeScript, or JSON file to start the language server.")
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
    }

    private fun renderProblems(state: LocaleBreezeDashboardState) {
        state.issues.forEach { issue ->
            body.add(ErrorCard().apply {
                layout = BoxLayout(this, BoxLayout.Y_AXIS)
                alignmentX = Component.LEFT_ALIGNMENT
                border = JBUI.Borders.empty(20)

                add(row(
                    JBLabel(AllIcons.General.Error).apply { border = JBUI.Borders.emptyRight(10) },
                    JBLabel("Error").apply {
                        foreground = errorForeground
                        font = font.deriveFont(font.size2D + 2f)
                    },
                ))
                add(Box.createVerticalStrut(16))
                add(WrappingText(issue.summary, emphasized = true))
                if (issue.remediation.isNotBlank()) {
                    add(Box.createVerticalStrut(8))
                    add(WrappingText(issue.remediation))
                }
                add(Box.createVerticalStrut(12))
                add(row(*buildList<JComponent> {
                    issue.path?.let { path ->
                        add(linkButton("Open file") { openPath(path, issue.line, issue.column) })
                    }
                    add(linkButton("Open settings") {
                        ShowSettingsUtil.getInstance().showSettingsDialog(project, LocaleBreezeConfigurable::class.java)
                    })
                }.toTypedArray()))
            })
            body.add(Box.createVerticalStrut(12))
        }
    }

    private fun toolBar(state: LocaleBreezeDashboardState): JComponent = JPanel(BorderLayout()).apply {
        val lifecycle = state.lifecycle
        val dictionaryPath = state.status?.defaultDictionaryPath
        val configPath = state.status?.configPath
        alignmentX = Component.LEFT_ALIGNMENT
        maximumSize = Dimension(Int.MAX_VALUE, JBUI.scale(30))
        isOpaque = false
        add(toolBarRow(
            iconButton(openDictionaryIcon, "Open dictionary", dictionaryPath != null) {
                dictionaryPath?.let(::openPath)
            },
            iconButton(configFileIcon, "Open workspace config", configPath != null) {
                configPath?.let(::openPath)
            },
        ), BorderLayout.WEST)
        add(toolBarRow(
            iconButton(settingsIcon, "Open LocaleBreeze settings") {
                ShowSettingsUtil.getInstance().showSettingsDialog(project, LocaleBreezeConfigurable::class.java)
            },
            iconButton(restartIcon, "Refresh", lifecycle != LocaleBreezeLifecycle.DISABLED) { refresh() },
            iconButton(powerIcon, if (lifecycle == LocaleBreezeLifecycle.DISABLED) "Enable" else "Disable") {
                setPluginEnabled(lifecycle == LocaleBreezeLifecycle.DISABLED)
            },
        ), BorderLayout.EAST)
    }

    private fun refresh() {
        model.starting()
        LspClientManager.getInstance(project)
            .stopAndRestartClientsIfNeeded(LocaleBreezeLspIntegrationProvider::class.java)
        project.service<LocaleBreezeKeyCache>().invalidate()
        DaemonCodeAnalyzer.getInstance(project).restart()
    }

    private fun setPluginEnabled(value: Boolean) {
        val settings = LocaleBreezeSettings.getInstance(project)
        settings.setActivationMode(
            if (value) LocaleBreezeSettings.ActivationMode.ENABLED
            else LocaleBreezeSettings.ActivationMode.DISABLED,
        )
        settings.state.enabled = value
        if (value) model.waiting() else model.disabled()
        val lspClients = LspClientManager.getInstance(project)
        lspClients.stopAndRestartClientsIfNeeded(LocaleBreezeLspIntegrationProvider::class.java)
        if (value) {
            lspClients.startClientsIfNeeded(LocaleBreezeLspIntegrationProvider::class.java)
        }
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

    private fun toolBarRow(vararg components: JComponent): JComponent =
        JPanel(FlowLayout(FlowLayout.LEFT, 0, 0)).apply {
            isOpaque = false
            components.forEachIndexed { index, component ->
                if (index > 0) add(Box.createHorizontalStrut(JBUI.scale(2)))
                add(component)
            }
        }

    private fun button(text: String, enabled: Boolean = true, action: () -> Unit): JButton =
        JButton(text).apply {
            isEnabled = enabled
            alignmentX = Component.LEFT_ALIGNMENT
            addActionListener { action() }
        }

    private fun iconButton(icon: javax.swing.Icon, tooltip: String, enabled: Boolean = true, action: (() -> Unit)? = null): JButton =
        ToolbarIconButton(icon).apply {
            val buttonSize = JBUI.size(26, 26)
            preferredSize = buttonSize
            minimumSize = buttonSize
            maximumSize = buttonSize
            margin = Insets(0, 0, 0, 0)
            isEnabled = enabled
            toolTipText = tooltip
            isContentAreaFilled = false
            isFocusPainted = false
            isRolloverEnabled = true
            border = JBUI.Borders.empty()
            action?.let { callback -> addActionListener { callback() } }
        }

    private fun linkButton(text: String, action: () -> Unit): JButton = JButton("<html><u>${html(text)}</u></html>").apply {
        foreground = JBColor.namedColor("Link.activeForeground", JBColor(0x2F65CA, 0xA8C7FA))
        isContentAreaFilled = false
        isFocusPainted = false
        border = JBUI.Borders.empty()
        cursor = java.awt.Cursor.getPredefinedCursor(java.awt.Cursor.HAND_CURSOR)
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

    private val errorForeground = JBColor(Color(0xB42318), Color(0xF28B8D))

    private class ErrorCard : JPanel() {
        private val fill = JBColor(Color(0xFFF2F0), Color(0x713536))
        private val stroke = JBColor(Color(0xD92D20), Color(0xC85153))

        init {
            isOpaque = false
        }

        override fun getMaximumSize(): Dimension = Dimension(Int.MAX_VALUE, preferredSize.height)

        override fun paintComponent(graphics: Graphics) {
            val g = graphics.create() as Graphics2D
            g.setRenderingHint(RenderingHints.KEY_ANTIALIASING, RenderingHints.VALUE_ANTIALIAS_ON)
            g.color = fill
            g.fillRoundRect(0, 0, width - 1, height - 1, JBUI.scale(8), JBUI.scale(8))
            g.color = stroke
            g.drawRoundRect(0, 0, width - 1, height - 1, JBUI.scale(8), JBUI.scale(8))
            g.dispose()
            super.paintComponent(graphics)
        }
    }

    private class WrappingText(text: String, emphasized: Boolean = false) : JTextArea(text) {
        init {
            isEditable = false
            isFocusable = false
            isOpaque = false
            lineWrap = true
            wrapStyleWord = true
            border = JBUI.Borders.empty()
            margin = Insets(0, 0, 0, 0)
            font = JBLabel().font.let { if (emphasized) it.deriveFont(it.size2D + 2f) else it }
            foreground = JBColor.foreground()
            alignmentX = Component.LEFT_ALIGNMENT
        }

        override fun getPreferredSize(): Dimension {
            val availableWidth = parent
                ?.let { it.width - it.insets.left - it.insets.right }
                ?.takeIf { it > 0 }
                ?: JBUI.scale(240)
            setSize(availableWidth, Short.MAX_VALUE.toInt())
            return super.getPreferredSize().apply { width = availableWidth }
        }

        override fun getMaximumSize(): Dimension = Dimension(Int.MAX_VALUE, preferredSize.height)
    }

    private class ToolWindowBody : JPanel(), Scrollable {
        override fun getPreferredScrollableViewportSize(): Dimension = preferredSize

        override fun getScrollableUnitIncrement(
            visibleRect: Rectangle,
            orientation: Int,
            direction: Int,
        ): Int = JBUI.scale(16)

        override fun getScrollableBlockIncrement(
            visibleRect: Rectangle,
            orientation: Int,
            direction: Int,
        ): Int = if (orientation == javax.swing.SwingConstants.VERTICAL) {
            visibleRect.height.coerceAtLeast(JBUI.scale(16))
        } else {
            visibleRect.width.coerceAtLeast(JBUI.scale(16))
        }

        override fun getScrollableTracksViewportWidth(): Boolean = true

        override fun getScrollableTracksViewportHeight(): Boolean = false
    }

    private class ToolbarIconButton(icon: javax.swing.Icon) : JButton(icon) {
        private val hoverBackground = JBColor.namedColor(
            "ActionButton.hoverBackground",
            JBColor(Color(0xDFE1E5), Color(0x4C5052)),
        )
        private val pressedBackground = JBColor.namedColor(
            "ActionButton.pressedBackground",
            JBColor(Color(0xC9CCD1), Color(0x5A5D5F)),
        )

        override fun paintComponent(graphics: Graphics) {
            if (!isEnabled) {
                val g = graphics.create() as Graphics2D
                g.composite = AlphaComposite.getInstance(AlphaComposite.SRC_OVER, 0.4f)
                super.paintComponent(g)
                g.dispose()
                return
            }

            if (isEnabled && (model.isRollover || model.isPressed)) {
                val g = graphics.create() as Graphics2D
                g.setRenderingHint(RenderingHints.KEY_ANTIALIASING, RenderingHints.VALUE_ANTIALIAS_ON)
                g.color = if (model.isPressed) pressedBackground else hoverBackground
                g.fillRoundRect(0, 0, width, height, JBUI.scale(6), JBUI.scale(6))
                g.dispose()
            }
            super.paintComponent(graphics)
        }
    }
}
