package dev.localebreeze.jetbrains

import com.intellij.openapi.fileChooser.FileChooserDescriptor
import com.intellij.openapi.options.Configurable
import com.intellij.openapi.project.Project
import com.intellij.openapi.ui.TextFieldWithBrowseButton
import com.intellij.platform.lsp.api.LspClientManager
import java.awt.GridBagConstraints
import java.awt.GridBagLayout
import java.awt.Insets
import javax.swing.JComponent
import javax.swing.JLabel
import javax.swing.JPanel

class LocaleBreezeConfigurable(private val project: Project) : Configurable {
    private val serverPath = TextFieldWithBrowseButton()
    private val configPath = TextFieldWithBrowseButton()
    private var panel: JPanel? = null

    override fun getDisplayName(): String = "LocaleBreeze"

    override fun createComponent(): JComponent {
        serverPath.addBrowseFolderListener(
            project,
            FileChooserDescriptor(true, false, false, false, false, false),
        )
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
            constraints.weightx = 0.0
            created.add(JLabel("Server executable:"), constraints)
            constraints.gridx = 1
            constraints.weightx = 1.0
            created.add(serverPath, constraints)
            constraints.gridx = 0
            constraints.gridy = 1
            constraints.weightx = 0.0
            created.add(JLabel("Configuration file:"), constraints)
            constraints.gridx = 1
            constraints.weightx = 1.0
            created.add(configPath, constraints)
            constraints.gridx = 0
            constraints.gridy = 2
            constraints.gridwidth = 2
            constraints.weighty = 1.0
            constraints.fill = GridBagConstraints.BOTH
            created.add(JPanel(), constraints)
            panel = created
        }
    }

    override fun isModified(): Boolean {
        val state = LocaleBreezeSettings.getInstance(project).state
        return serverPath.text.trim() != state.serverPath || configPath.text.trim() != state.configPath
    }

    override fun apply() {
        val state = LocaleBreezeSettings.getInstance(project).state
        state.serverPath = serverPath.text.trim()
        state.configPath = configPath.text.trim()
        LspClientManager.getInstance(project)
            .stopAndRestartClientsIfNeeded(LocaleBreezeLspIntegrationProvider::class.java)
    }

    override fun reset() {
        val state = LocaleBreezeSettings.getInstance(project).state
        serverPath.text = state.serverPath
        configPath.text = state.configPath
    }

    override fun disposeUIResources() {
        panel = null
    }
}
