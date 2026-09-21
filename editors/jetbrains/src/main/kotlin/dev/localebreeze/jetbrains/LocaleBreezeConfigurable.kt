package dev.localebreeze.jetbrains

import com.intellij.openapi.fileChooser.FileChooserDescriptor
import com.intellij.codeInsight.daemon.DaemonCodeAnalyzer
import com.intellij.openapi.options.Configurable
import com.intellij.openapi.project.Project
import com.intellij.openapi.components.service
import com.intellij.openapi.ui.TextFieldWithBrowseButton
import com.intellij.platform.lsp.api.LspClientManager
import java.awt.GridBagConstraints
import java.awt.GridBagLayout
import java.awt.Insets
import javax.swing.JComponent
import javax.swing.JLabel
import javax.swing.JCheckBox
import javax.swing.JPanel

class LocaleBreezeConfigurable(private val project: Project) : Configurable {
    private val enabled = JCheckBox("Enable LocaleBreeze in this workspace")
    private val configPath = TextFieldWithBrowseButton()
    private val overrideConfig = JCheckBox("Override LocaleBreeze config")
    private val showUnusedKeys = JCheckBox("Show unused keys")
    private var panel: JPanel? = null

    override fun getDisplayName(): String = "LocaleBreeze"

    override fun createComponent(): JComponent {
        overrideConfig.addActionListener {
            showUnusedKeys.isEnabled = overrideConfig.isSelected
        }
        configPath.addBrowseFolderListener(
            project,
            FileChooserDescriptor(true, false, false, false, false, false)
                .withFileFilter { it.extension.equals("json", ignoreCase = true) },
        )
        reset()
        return JPanel(GridBagLayout()).also { created ->
            val constraints = GridBagConstraints().apply {
                anchor = GridBagConstraints.WEST
                fill = GridBagConstraints.HORIZONTAL
                insets = Insets(4, 4, 4, 4)
            }
            constraints.gridx = 0
            constraints.gridy = 0
            constraints.gridwidth = 2
            created.add(enabled, constraints)
            constraints.gridy = 1
            constraints.gridwidth = 1
            constraints.weightx = 0.0
            created.add(JLabel("Configuration file:"), constraints)
            constraints.gridx = 1
            constraints.weightx = 1.0
            created.add(configPath, constraints)
            constraints.gridx = 0
            constraints.gridy = 2
            constraints.gridwidth = 2
            created.add(overrideConfig, constraints)
            constraints.gridy = 3
            created.add(showUnusedKeys, constraints)
            constraints.gridx = 0
            constraints.gridy = 4
            constraints.gridwidth = 2
            constraints.weighty = 1.0
            constraints.fill = GridBagConstraints.BOTH
            created.add(JPanel(), constraints)
            panel = created
        }
    }

    override fun isModified(): Boolean {
        val state = LocaleBreezeSettings.getInstance(project).state
        return enabled.isSelected != state.enabled ||
            configPath.text.trim() != state.configPath ||
            overrideConfig.isSelected != state.overrideConfig ||
            showUnusedKeys.isSelected != state.showUnusedKeys
    }

    override fun apply() {
        val state = LocaleBreezeSettings.getInstance(project).state
        state.enabled = enabled.isSelected
        state.configPath = configPath.text.trim()
        state.overrideConfig = overrideConfig.isSelected
        state.showUnusedKeys = showUnusedKeys.isSelected
        LspClientManager.getInstance(project)
            .stopAndRestartClientsIfNeeded(LocaleBreezeLspIntegrationProvider::class.java)
        project.service<LocaleBreezeKeyCache>().invalidate()
        if (!state.enabled) project.service<LocaleBreezeWarningCoordinator>().clear()
        DaemonCodeAnalyzer.getInstance(project).restart()
    }

    override fun reset() {
        val state = LocaleBreezeSettings.getInstance(project).state
        enabled.isSelected = state.enabled
        configPath.text = state.configPath
        overrideConfig.isSelected = state.overrideConfig
        showUnusedKeys.isSelected = state.showUnusedKeys
        showUnusedKeys.isEnabled = state.overrideConfig
    }

    override fun disposeUIResources() {
        panel = null
    }
}
