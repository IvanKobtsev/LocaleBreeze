package dev.localebreeze.jetbrains

import com.intellij.openapi.fileChooser.FileChooserDescriptor
import com.intellij.codeInsight.daemon.DaemonCodeAnalyzer
import com.intellij.openapi.options.Configurable
import com.intellij.openapi.project.Project
import com.intellij.openapi.components.service
import com.intellij.openapi.ui.TextFieldWithBrowseButton
import com.intellij.platform.lsp.api.LspClientManager
import com.intellij.util.ui.JBUI
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
    private val developmentServerPath = TextFieldWithBrowseButton()
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
        developmentServerPath.addBrowseFolderListener(
            project,
            FileChooserDescriptor(true, false, false, false, false, false),
        )
        reset()
        return JPanel(GridBagLayout()).also { created ->
            val constraints = GridBagConstraints().apply {
                anchor = GridBagConstraints.WEST
                fill = GridBagConstraints.HORIZONTAL
                insets = JBUI.insets(4)
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
            if (LocaleBreezeDevelopment.enabled) {
                constraints.weightx = 0.0
                created.add(JLabel("Development language server executable:"), constraints)
                constraints.gridx = 1
                constraints.weightx = 1.0
                created.add(developmentServerPath, constraints)
                constraints.gridx = 0
                constraints.gridy++
            }
            constraints.gridwidth = 2
            created.add(overrideConfig, constraints)
            constraints.gridy++
            created.add(showUnusedKeys, constraints)
            constraints.gridx = 0
            constraints.gridy++
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
            (LocaleBreezeDevelopment.enabled &&
                developmentServerPath.text.trim() != state.developmentServerPath) ||
            overrideConfig.isSelected != state.overrideConfig ||
            showUnusedKeys.isSelected != state.showUnusedKeys
    }

    override fun apply() {
        val settings = LocaleBreezeSettings.getInstance(project)
        val state = settings.state
        if (enabled.isSelected != state.enabled) {
            settings.setActivationMode(
                if (enabled.isSelected) LocaleBreezeSettings.ActivationMode.ENABLED
                else LocaleBreezeSettings.ActivationMode.DISABLED,
            )
        }
        state.enabled = enabled.isSelected
        state.configPath = configPath.text.trim()
        if (LocaleBreezeDevelopment.enabled) {
        state.developmentServerPath = developmentServerPath.text.trim()
        }
        state.overrideConfig = overrideConfig.isSelected
        state.showUnusedKeys = showUnusedKeys.isSelected
        if (state.enabled) project.service<LocaleBreezeWarningCoordinator>().waiting()
        else project.service<LocaleBreezeWarningCoordinator>().disabled()
        val lspClients = LspClientManager.getInstance(project)
        lspClients.stopAndRestartClientsIfNeeded(LocaleBreezeLspIntegrationProvider::class.java)
        if (state.enabled) {
            lspClients.startClientsIfNeeded(LocaleBreezeLspIntegrationProvider::class.java)
        }
        project.service<LocaleBreezeKeyCache>().invalidate()
        DaemonCodeAnalyzer.getInstance(project).restart()
    }

    override fun reset() {
        val state = LocaleBreezeSettings.getInstance(project).state
        enabled.isSelected = state.enabled
        configPath.text = state.configPath
        developmentServerPath.text = state.developmentServerPath
        overrideConfig.isSelected = state.overrideConfig
        showUnusedKeys.isSelected = state.showUnusedKeys
        showUnusedKeys.isEnabled = state.overrideConfig
    }

    override fun disposeUIResources() {
        panel = null
    }
}
