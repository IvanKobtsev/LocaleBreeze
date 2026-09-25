package dev.localebreeze.jetbrains

import com.intellij.openapi.fileChooser.FileChooserDescriptor
import com.intellij.openapi.options.Configurable
import com.intellij.openapi.options.ConfigurationException
import com.intellij.openapi.project.Project
import com.intellij.openapi.components.service
import com.intellij.openapi.ui.TextFieldWithBrowseButton
import com.intellij.ui.TitledSeparator
import com.intellij.util.ui.JBUI
import java.awt.GridBagConstraints
import java.awt.GridBagLayout
import java.awt.Insets
import javax.swing.JComponent
import javax.swing.JLabel
import javax.swing.JCheckBox
import javax.swing.JPanel
import javax.swing.JRadioButton
import javax.swing.ButtonGroup

class LocaleBreezeConfigurable(private val project: Project) : Configurable {
    private val enabled = JCheckBox("Enable LocaleBreeze in this workspace")
    private val configSection = TitledSeparator("Config file location")
    private val preferencesSection = TitledSeparator("User preferences")
    private val developmentSection = TitledSeparator("Development")
    private val workspaceRoot = JRadioButton("Workspace root")
    private val customPath = JRadioButton("Custom path")
    private val customPathLabel = JLabel("Path:")
    private val configPath = TextFieldWithBrowseButton()
    private val developmentServerLabel = JLabel("Development language server executable:")
    private val developmentServerPath = TextFieldWithBrowseButton()
    private val showUnusedKeys = JCheckBox("Show unused keys")
    private var panel: JPanel? = null

    override fun getDisplayName(): String = "LocaleBreeze"

    override fun createComponent(): JComponent {
        ButtonGroup().apply {
            add(workspaceRoot)
            add(customPath)
        }
        enabled.addActionListener { updateOptionsEnabled() }
        workspaceRoot.addActionListener(::updateConfigPathVisibility)
        customPath.addActionListener(::updateConfigPathVisibility)
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
            var row = 1
            constraints.gridy = row++
            constraints.weightx = 1.0
            created.add(configSection, constraints)
            constraints.gridy = row++
            created.add(workspaceRoot, constraints)
            constraints.gridy = row++
            created.add(customPath, constraints)
            constraints.gridy = row++
            constraints.gridwidth = 1
            constraints.weightx = 0.0
            created.add(customPathLabel, constraints)
            constraints.gridx = 1
            constraints.weightx = 1.0
            created.add(configPath, constraints)
            constraints.gridx = 0
            if (LocaleBreezeDevelopment.enabled) {
                constraints.gridy = row++
                constraints.gridwidth = 2
                created.add(developmentSection, constraints)
                constraints.gridy = row++
                constraints.gridwidth = 1
                constraints.weightx = 0.0
                created.add(developmentServerLabel, constraints)
                constraints.gridx = 1
                constraints.weightx = 1.0
                created.add(developmentServerPath, constraints)
                constraints.gridx = 0
            }
            constraints.gridy = row++
            constraints.gridwidth = 2
            constraints.weightx = 1.0
            created.add(preferencesSection, constraints)
            constraints.gridy = row++
            created.add(showUnusedKeys, constraints)
            constraints.gridx = 0
            constraints.gridy = row
            constraints.gridwidth = 2
            constraints.weighty = 1.0
            constraints.fill = GridBagConstraints.BOTH
            created.add(JPanel(), constraints)
            panel = created
        }
    }

    override fun isModified(): Boolean {
        val settings = LocaleBreezeSettings.getInstance(project)
        val state = settings.state
        return enabled.isSelected != settings.isEnabledInSettings() ||
            selectedConfigLocation() != settings.configLocation() ||
            configPath.text.trim() != state.configPath ||
            (LocaleBreezeDevelopment.enabled &&
                developmentServerPath.text.trim() != state.developmentServerPath) ||
            showUnusedKeys.isSelected != LocaleBreezePreferences.getInstance().state.showUnusedKeys
    }

    override fun apply() {
        val settings = LocaleBreezeSettings.getInstance(project)
        val state = settings.state
        if (enabled.isSelected && customPath.isSelected && configPath.text.isBlank()) {
            throw ConfigurationException("Choose a LocaleBreeze configuration file for Custom path.")
        }
        if (enabled.isSelected != settings.isEnabledInSettings()) {
            settings.setActivationMode(
                if (enabled.isSelected) LocaleBreezeSettings.ActivationMode.ENABLED
                else LocaleBreezeSettings.ActivationMode.DISABLED,
            )
        }
        state.configPath = project.service<LocaleBreezeConfigDiscovery>()
            .normalizeForStorage(configPath.text.trim())
        state.configLocation = selectedConfigLocation().name
        configPath.text = state.configPath
        if (LocaleBreezeDevelopment.enabled) {
            state.developmentServerPath = developmentServerPath.text.trim()
        }
        LocaleBreezePreferences.getInstance().setShowUnusedKeys(showUnusedKeys.isSelected)
        project.service<LocaleBreezeConfigDiscovery>().refresh(true)
    }

    override fun reset() {
        val state = LocaleBreezeSettings.getInstance(project).state
        enabled.isSelected = LocaleBreezeSettings.getInstance(project).isEnabledInSettings()
        workspaceRoot.isSelected = LocaleBreezeSettings.getInstance(project).configLocation() ==
            LocaleBreezeSettings.ConfigLocation.WORKSPACE_ROOT
        customPath.isSelected = !workspaceRoot.isSelected
        configPath.text = state.configPath
        updateConfigPathVisibility()
        developmentServerPath.text = state.developmentServerPath
        showUnusedKeys.isSelected = LocaleBreezePreferences.getInstance().state.showUnusedKeys
        updateOptionsEnabled()
    }

    override fun disposeUIResources() {
        panel = null
    }

    private fun selectedConfigLocation(): LocaleBreezeSettings.ConfigLocation =
        if (customPath.isSelected) LocaleBreezeSettings.ConfigLocation.CUSTOM_PATH
        else LocaleBreezeSettings.ConfigLocation.WORKSPACE_ROOT

    private fun updateConfigPathVisibility(@Suppress("UNUSED_PARAMETER") event: java.awt.event.ActionEvent? = null) {
        val visible = customPath.isSelected
        customPathLabel.isVisible = visible
        configPath.isVisible = visible
        updateOptionsEnabled()
        panel?.revalidate()
        panel?.repaint()
    }

    private fun updateOptionsEnabled() {
        val workspaceEnabled = enabled.isSelected
        configSection.isEnabled = workspaceEnabled
        preferencesSection.isEnabled = workspaceEnabled
        developmentSection.isEnabled = workspaceEnabled
        workspaceRoot.isEnabled = workspaceEnabled
        customPath.isEnabled = workspaceEnabled
        customPathLabel.isEnabled = workspaceEnabled
        configPath.isEnabled = workspaceEnabled && customPath.isSelected
        developmentServerLabel.isEnabled = workspaceEnabled
        developmentServerPath.isEnabled = workspaceEnabled
        showUnusedKeys.isEnabled = workspaceEnabled
    }
}
