package dev.localebreeze.jetbrains

import com.intellij.openapi.components.PersistentStateComponent
import com.intellij.openapi.components.Service
import com.intellij.openapi.components.State
import com.intellij.openapi.components.Storage
import com.intellij.openapi.components.StoragePathMacros
import com.intellij.openapi.components.service
import com.intellij.openapi.project.Project

@Service(Service.Level.PROJECT)
@State(name = "LocaleBreezeSettings", storages = [Storage(StoragePathMacros.WORKSPACE_FILE)])
class LocaleBreezeSettings : PersistentStateComponent<LocaleBreezeSettings.Data> {
    data class Data(
        var schemaVersion: Int = 0,
        var enabled: Boolean = false,
        var enablePromptDismissed: Boolean = false,
        var activationMode: String = ActivationMode.AUTO.name,
        var configPath: String = "",
        var overrideConfig: Boolean = false,
        var showUnusedKeys: Boolean = true,
    )

    enum class ActivationMode { AUTO, ENABLED, DISABLED }

    private var data = Data(schemaVersion = CURRENT_SCHEMA_VERSION)

    override fun getState(): Data = data

    override fun loadState(state: Data) {
        if (state.schemaVersion < CURRENT_SCHEMA_VERSION) {
            state.activationMode = if (state.enabled) ActivationMode.ENABLED.name else ActivationMode.AUTO.name
            state.schemaVersion = CURRENT_SCHEMA_VERSION
        }
        data = state
    }

    fun activationMode(): ActivationMode =
        runCatching { ActivationMode.valueOf(data.activationMode) }.getOrDefault(ActivationMode.AUTO)

    fun setActivationMode(mode: ActivationMode) {
        data.activationMode = mode.name
        data.enabled = mode == ActivationMode.ENABLED || (mode == ActivationMode.AUTO && data.enabled)
    }

    companion object {
        private const val CURRENT_SCHEMA_VERSION = 1
        fun getInstance(project: Project): LocaleBreezeSettings = project.service()
    }
}
