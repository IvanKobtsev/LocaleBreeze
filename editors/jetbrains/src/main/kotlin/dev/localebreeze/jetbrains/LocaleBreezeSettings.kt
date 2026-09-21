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
        var enabled: Boolean = false,
        var enablePromptDismissed: Boolean = false,
        var configPath: String = "",
        var overrideConfig: Boolean = false,
        var showUnusedKeys: Boolean = true,
    )

    private var data = Data()

    override fun getState(): Data = data

    override fun loadState(state: Data) {
        data = state
    }

    companion object {
        fun getInstance(project: Project): LocaleBreezeSettings = project.service()
    }
}
