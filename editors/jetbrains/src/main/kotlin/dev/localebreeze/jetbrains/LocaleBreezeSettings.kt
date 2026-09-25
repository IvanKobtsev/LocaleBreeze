package dev.localebreeze.jetbrains

import com.intellij.openapi.components.PersistentStateComponent
import com.intellij.openapi.components.Service
import com.intellij.openapi.components.State
import com.intellij.openapi.components.Storage
import com.intellij.openapi.components.StoragePathMacros
import com.intellij.openapi.components.service
import com.intellij.openapi.application.ApplicationManager
import com.intellij.openapi.project.Project

@Service(Service.Level.PROJECT)
@State(name = "LocaleBreezeSettings", storages = [Storage(StoragePathMacros.WORKSPACE_FILE)])
class LocaleBreezeSettings : PersistentStateComponent<LocaleBreezeSettings.Data> {
    data class Data(
        var schemaVersion: Int = 0,
        var enabled: Boolean = false,
        var enablePromptDismissed: Boolean = false,
        var activationMode: String = ActivationMode.AUTO.name,
        var configLocation: String = ConfigLocation.WORKSPACE_ROOT.name,
        var configPath: String = "",
        var developmentServerPath: String = "",
        var overrideConfig: Boolean = false,
        var showUnusedKeys: Boolean = true,
    )

    enum class ActivationMode { AUTO, ENABLED, DISABLED }
    enum class ConfigLocation { WORKSPACE_ROOT, CUSTOM_PATH }

    private var data = Data(schemaVersion = CURRENT_SCHEMA_VERSION)

    override fun getState(): Data = data

    override fun loadState(state: Data) {
        ApplicationManager.getApplication()
            ?.getService(LocaleBreezePreferences::class.java)
            ?.migrateLegacyPreference(state.overrideConfig, state.showUnusedKeys)
        if (state.schemaVersion < 1) {
            state.activationMode = if (state.enabled) ActivationMode.ENABLED.name else ActivationMode.AUTO.name
        }
        if (state.schemaVersion < 3) {
            state.configLocation = if (state.configPath.isBlank()) {
                ConfigLocation.WORKSPACE_ROOT.name
            } else {
                ConfigLocation.CUSTOM_PATH.name
            }
        }
        state.schemaVersion = CURRENT_SCHEMA_VERSION
        state.overrideConfig = false
        state.showUnusedKeys = true
        data = state
    }

    fun activationMode(): ActivationMode =
        runCatching { ActivationMode.valueOf(data.activationMode) }.getOrDefault(ActivationMode.AUTO)

    fun setActivationMode(mode: ActivationMode) {
        data.activationMode = mode.name
        if (mode == ActivationMode.DISABLED) data.enabled = false
    }

    fun isEnabledInSettings(): Boolean =
        activationMode() == ActivationMode.ENABLED || data.enabled

    fun configLocation(): ConfigLocation =
        runCatching { ConfigLocation.valueOf(data.configLocation) }.getOrDefault(ConfigLocation.WORKSPACE_ROOT)

    companion object {
        private const val CURRENT_SCHEMA_VERSION = 3
        fun getInstance(project: Project): LocaleBreezeSettings = project.service()
    }
}
