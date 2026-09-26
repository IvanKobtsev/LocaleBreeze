package dev.localebreeze.jetbrains

import com.intellij.icons.AllIcons
import com.intellij.openapi.Disposable
import com.intellij.openapi.components.service
import com.intellij.openapi.fileEditor.OpenFileDescriptor
import com.intellij.codeInsight.daemon.DaemonCodeAnalyzer
import com.intellij.openapi.fileChooser.FileChooser
import com.intellij.openapi.fileChooser.FileChooserDescriptor
import com.intellij.openapi.options.ShowSettingsUtil
import com.intellij.openapi.project.Project
import com.intellij.openapi.ui.Messages
import com.intellij.openapi.util.IconLoader
import com.intellij.openapi.util.text.StringUtil
import com.intellij.openapi.vfs.LocalFileSystem
import com.intellij.openapi.wm.ToolWindow
import com.intellij.openapi.wm.ToolWindowFactory
import com.intellij.ui.JBColor
import com.intellij.ui.components.JBLabel
import com.intellij.ui.components.JBScrollPane
import com.intellij.ui.content.ContentFactory
import com.intellij.util.ui.JBUI
import com.intellij.util.ui.UIUtil
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
import javax.swing.JComboBox
import javax.swing.JComponent
import javax.swing.JPanel
import javax.swing.JProgressBar
import javax.swing.JSeparator
import javax.swing.Scrollable
import javax.swing.SwingUtilities
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
    private val scrollPane = JBScrollPane(body).apply { border = JBUI.Borders.empty() }
    val component: JComponent = scrollPane
    private val model = project.service<LocaleBreezeWarningCoordinator>()
    private val normalToolIcon = IconLoader.getIcon("/icons/tool_window_icon/default/localeBreeze.svg", javaClass)
    private val fadedToolIcon = IconLoader.getIcon("/icons/tool_window_icon/disabled/localeBreeze.svg", javaClass)
    private val openDictionaryIcon = IconLoader.getIcon("/icons/open_translation_dictionary/localeBreeze.svg", javaClass)
    private val configFileIcon = IconLoader.getIcon("/icons/config_file/localeBreeze.svg", javaClass)
    private val settingsIcon = IconLoader.getIcon("/icons/settings_icon/localeBreeze.svg", javaClass)
    private val powerIcon = IconLoader.getIcon("/icons/power_icon/localeBreeze.svg", javaClass)
    private val restartIcon = IconLoader.getIcon("/icons/restart_icon/localeBreeze.svg", javaClass)
    private val sleepPluginIcon = IconLoader.getIcon("/icons/sleep_picture/localeBreeze.svg", javaClass)
    private val accentColor = JBColor(Color(0x3574F0), Color(0x3574F0))

    init {
        body.layout = BoxLayout(body, BoxLayout.Y_AXIS)
        body.border = JBUI.Borders.empty(28, 30)
        model.addListener(this, ::render)
        render()
    }

    override fun dispose() = Unit

    private fun render() {
        val state = model.snapshot()
        val setupEnabled = LocaleBreezeSettings.getInstance(project).isEnabledInSettings()
        body.removeAll()

        if (state.issues.isNotEmpty())
            toolWindow.setIcon(AllIcons.General.Warning)
        else if (!setupEnabled)
            toolWindow.setIcon(fadedToolIcon)
        else toolWindow.setIcon(normalToolIcon)

        body.add(toolBar(state))
        body.add(Box.createVerticalStrut(10))
        body.add(JSeparator().apply {
            alignmentX = Component.LEFT_ALIGNMENT
            maximumSize = Dimension(Int.MAX_VALUE, preferredSize.height)
        })
        body.add(Box.createVerticalStrut(28))

        when {
            state.lifecycle == LocaleBreezeLifecycle.DISABLED && !setupEnabled -> renderDisabled()
            state.setup == LocaleBreezeConfigSetupState.Searching -> renderConfigSearching()
            state.setup is LocaleBreezeConfigSetupState.Candidates -> renderConfigCandidates(state.setup)
            state.setup == LocaleBreezeConfigSetupState.NotFound -> renderConfigNotFound()
            state.issues.isNotEmpty() -> renderProblems(state)
            state.lifecycle == LocaleBreezeLifecycle.DISABLED -> renderDisabled()
            state.lifecycle == LocaleBreezeLifecycle.WAITING -> renderWaiting()
            state.lifecycle == LocaleBreezeLifecycle.STARTING -> renderStarting()
            state.lifecycle == LocaleBreezeLifecycle.UNAVAILABLE -> renderUnavailable()
            else -> renderHealthy(state.status)
        }

        body.add(Box.createVerticalGlue())
        body.invalidate()
        body.revalidate()
        body.repaint()
        scrollPane.viewport.invalidate()
        scrollPane.viewport.revalidate()
        component.revalidate()
        component.repaint()
        SwingUtilities.invokeLater {
            if (project.isDisposed) return@invokeLater
            body.invalidate()
            body.revalidate()
            scrollPane.viewport.revalidate()
            component.repaint()
        }
    }

    private fun renderConfigSearching() {
        infoCard("Looking for a configuration…", "LocaleBreeze is searching indexed project files.") {
            add(JProgressBar().apply { isIndeterminate = true; alignmentX = Component.LEFT_ALIGNMENT })
        }
    }

    private fun renderConfigCandidates(setup: LocaleBreezeConfigSetupState.Candidates) {
        if (setup.paths.size == 1) {
            val path = setup.paths.single()
            infoCard("Configuration found", relativePath(path)) {
                add(actionRow(
                    cardButton("Use this config", primary = true) { useConfig(path) },
                    cardButton("Create new") { createRootConfig() },
                ))
            }
        } else {
            infoCard(
                "Choose a configuration",
                "LocaleBreeze found ${setup.paths.size} configuration files in this project.",
            ) {
                val selector = JComboBox(setup.paths.map(::relativePath).toTypedArray()).apply {
                    alignmentX = Component.LEFT_ALIGNMENT
                    maximumSize = Dimension(Int.MAX_VALUE, preferredSize.height)
                }
                add(selector)
                add(Box.createVerticalStrut(8))
                add(actionRow(
                    cardButton("Use selected config", primary = true) {
                        useConfig(setup.paths[selector.selectedIndex])
                    },
                    cardButton("Create new") { createRootConfig() },
                ))
            }
        }
    }

    private fun renderConfigNotFound() {
        infoCard("Configuration not found", "No configuration file exists at the selected location.") {
            add(actionRow(
                cardButton("Create new") { createRootConfig() },
                cardButton("Point to existing…") { chooseExistingConfig() },
                cardButton("Open settings…") {
                    ShowSettingsUtil.getInstance().showSettingsDialog(project, LocaleBreezeConfigurable::class.java)
                },
            ))
        }
    }

    private fun createRootConfig() {
        project.service<LocaleBreezeConfigDiscovery>().createRootConfig()
    }

    private fun useConfig(path: String) {
        project.service<LocaleBreezeConfigDiscovery>().applySelectedConfig(Path.of(path))
    }

    private fun relativePath(value: String): String {
        val path = Path.of(value).toAbsolutePath().normalize()
        val root = project.basePath?.let(Path::of)?.toAbsolutePath()?.normalize() ?: return value
        return runCatching { root.relativize(path).toString() }.getOrDefault(value)
    }

    private fun chooseExistingConfig() {
        val descriptor = FileChooserDescriptor(true, false, false, false, false, false)
            .withTitle("Choose LocaleBreeze Configuration")
            .withFileFilter { it.name == LocaleBreezeConfigDiscovery.CONFIG_FILE_NAME || it.extension.equals("json", true) }
        FileChooser.chooseFile(descriptor, project, null) { file ->
            project.service<LocaleBreezeConfigDiscovery>().applySelectedConfig(file.toNioPath())
        }
    }

    private fun actionRow(vararg components: JComponent): JComponent = row(*components).apply {
        alignmentX = Component.LEFT_ALIGNMENT
    }

    private fun infoCard(title: String, message: String, content: (JPanel.() -> Unit)? = null) {
        body.add(InfoCard().apply {
            layout = BoxLayout(this, BoxLayout.Y_AXIS)
            alignmentX = Component.LEFT_ALIGNMENT
            border = JBUI.Borders.empty(12, 15)
            add(WrappingText(title, emphasized = true))
            if (message.isNotBlank()) {
                add(Box.createVerticalStrut(8))
                add(WrappingText(message))
            }
            if (content != null) {
                add(Box.createVerticalStrut(12))
                content()
            }
        })
    }

    private fun pluginDisabledMessage() {
        body.add(InfoCard(JBColor(Color(0x343538), Color(0x343538))).apply {
            layout = BoxLayout(this, BoxLayout.Y_AXIS)
            alignmentX = Component.LEFT_ALIGNMENT
            border = JBUI.Borders.empty(12, 15)
            add(WrappingText("LocaleBreeze is disabled in this workspace", emphasized = true, color = JBColor.YELLOW))
            add(Box.createVerticalStrut(8))
            add(WrappingText("Enable LocaleBreeze to find a configuration and start indexing translations."))
        })
    }

    private fun renderDisabled() {
        body.add(JBLabel(sleepPluginIcon).apply {
            alignmentX = Component.LEFT_ALIGNMENT
        })
        pluginDisabledMessage()
    }

    private fun renderStarting() {
        infoCard("Starting LocaleBreeze…", "The language server is indexing this workspace.") {
            add(JProgressBar().apply { isIndeterminate = true; alignmentX = Component.LEFT_ALIGNMENT })
        }
    }

    private fun renderWaiting() {
        infoCard("LocaleBreeze is ready", "Open a JavaScript, TypeScript, or JSON file to start the language server.")
    }

    private fun renderUnavailable() {
        infoCard("LocaleBreeze is unavailable", "Refresh the workspace or open settings to check the configuration.")
    }

    private fun renderHealthy(status: LocaleBreezeWorkspaceStatus?) {
        if (status == null) {
            infoCard("Workspace is healthy", "Waiting for workspace information.")
            return
        }
        infoCard("Workspace is healthy", "Default locale: ${status.defaultLocale}") {
            add(metric(status.unusedKeyCount.toString(), "Unused keys"))
            add(metric(status.totalKeyCount.toString(), "Default-locale keys"))
            add(metric(status.dictionaryFileCount.toString(), "Dictionaries"))
            if (dictionaryRootNeedsAttachment(status)) {
                add(Box.createVerticalStrut(12))
                add(WrappingText("The dictionary directory is outside this WebStorm project. Attach it to enable standard editor diagnostics."))
                add(Box.createVerticalStrut(8))
                add(cardButton("Attach dictionary directory", primary = true) {
                    attachDictionaryRoot(status)
                })
            }
        }
    }

    private fun dictionaryRootNeedsAttachment(status: LocaleBreezeWorkspaceStatus): Boolean {
        if (status.dictionaryRootPath.isBlank()) return false
        val path = runCatching { Path.of(status.dictionaryRootPath) }.getOrNull() ?: return false
        return !project.service<LocaleBreezeContentRoots>().isAttached(path)
    }

    private fun attachDictionaryRoot(status: LocaleBreezeWorkspaceStatus) {
        val root = runCatching { Path.of(status.dictionaryRootPath).toAbsolutePath().normalize() }.getOrNull()
            ?.let { LocalFileSystem.getInstance().refreshAndFindFileByNioFile(it) }
            ?: return
        if (!project.service<LocaleBreezeContentRoots>().attach(root.toNioPath())) {
            Messages.showErrorDialog(
                project,
                "WebStorm could not attach the dictionary directory to this project.",
                "LocaleBreeze",
            )
            return
        }
        runCatching { Path.of(status.configPath).toAbsolutePath().normalize() }
            .getOrNull()
            ?.takeIf { java.nio.file.Files.isRegularFile(it) }
            ?.let { configPath ->
                LocaleBreezeSettings.getInstance(project).state.apply {
                    this.configPath = configPath.toString()
                    configLocation = LocaleBreezeSettings.ConfigLocation.CUSTOM_PATH.name
                }
            }
        DaemonCodeAnalyzer.getInstance(project).restart()
        refresh()
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
        val enabledInSettings = LocaleBreezeSettings.getInstance(project).isEnabledInSettings()
        val dictionaryPath = state.status?.defaultDictionaryPath
        val configPath = when (val setup = state.setup) {
            is LocaleBreezeConfigSetupState.Configured -> setup.path
            is LocaleBreezeConfigSetupState.RootConfig -> setup.path
            else -> null
        }
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
            iconButton(restartIcon, "Refresh", enabledInSettings) { refresh() },
            iconButton(powerIcon, if (enabledInSettings) "Disable" else "Enable") {
                setPluginEnabled(!enabledInSettings)
            },
        ), BorderLayout.EAST)
    }

    private fun refresh() {
        project.service<LocaleBreezeConfigDiscovery>().refresh(true)
    }

    private fun setPluginEnabled(value: Boolean) {
        if (value) {
            project.service<LocaleBreezeConfigDiscovery>().enableAndDiscover()
            return
        }
        project.service<LocaleBreezeConfigDiscovery>().leaveDisabled()
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

    private fun metric(value: String, label: String): JComponent =
        JBLabel("<html><b>${html(value)}</b>&nbsp;&nbsp;${html(label)}</html>").apply {
            alignmentX = Component.LEFT_ALIGNMENT
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

    private fun cardButton(text: String, primary: Boolean = false, action: () -> Unit): JButton =
        RoundedCardButton(text, primary).apply {
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

    private class InfoCard(private var fillColor: Color? = null) : JPanel() {
        private val fill = JBColor.namedColor("Notification.background", UIUtil.getPanelBackground())

        init {
            isOpaque = false
        }

        override fun getMaximumSize(): Dimension = Dimension(Int.MAX_VALUE, preferredSize.height)

        override fun paintComponent(graphics: Graphics) {
            val g = graphics.create() as Graphics2D
            g.setRenderingHint(RenderingHints.KEY_ANTIALIASING, RenderingHints.VALUE_ANTIALIAS_ON)
            g.color = fillColor ?: fill
            g.fillRoundRect(0, 0, width - 1, height - 1, JBUI.scale(8), JBUI.scale(8))
            g.dispose()
            super.paintComponent(graphics)
        }
    }

    private class RoundedCardButton(text: String, private val primary: Boolean) : JButton(text) {
        private val outline = JBColor.namedColor("Component.borderColor", JBColor.border())
        private val accent = JBColor(Color(0x3574F0), Color(0x3574F0))

        init {
            isContentAreaFilled = false
            isOpaque = false
            isFocusPainted = false
            border = JBUI.Borders.empty(3, 6)
            if (primary) {
                foreground = JBColor(Color.WHITE, Color.WHITE)
                font = font.deriveFont(java.awt.Font.BOLD)
            }
        }

        override fun paintComponent(graphics: Graphics) {
            val g = graphics.create() as Graphics2D
            g.setRenderingHint(RenderingHints.KEY_ANTIALIASING, RenderingHints.VALUE_ANTIALIAS_ON)
            val arc = JBUI.scale(8)
            if (primary) {
                g.color = accent
                g.fillRoundRect(0, 0, width - 1, height - 1, arc, arc)
            }
            g.color = if (primary) accent else outline
            g.drawRoundRect(0, 0, width - 1, height - 1, arc, arc)
            g.dispose()
            super.paintComponent(graphics)
        }
    }

    private class WrappingText(text: String, emphasized: Boolean = false, color: Color? = null) : JTextArea(text) {
        init {
            isEditable = false
            isFocusable = false
            isOpaque = false
            lineWrap = true
            wrapStyleWord = true
            border = JBUI.Borders.empty()
            margin = Insets(0, 0, 0, 0)
            font = JBLabel().font.let { if (emphasized) it.deriveFont(java.awt.Font.BOLD,it.size2D + 2f) else it }
            foreground = color ?: JBColor.foreground()
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
