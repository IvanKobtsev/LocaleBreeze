package dev.localebreeze.jetbrains

import com.intellij.openapi.components.PersistentStateComponent
import com.intellij.openapi.components.Service
import com.intellij.openapi.components.State
import com.intellij.openapi.components.Storage
import com.intellij.openapi.components.service

@Service(Service.Level.APP)
@State(name = "LocaleBreezePreferences", storages = [Storage("localeBreeze.xml")])
class LocaleBreezePreferences : PersistentStateComponent<LocaleBreezePreferences.Data> {
    data class Data(
        var showUnusedKeys: Boolean = true,
        var initialized: Boolean = false,
    )

    private var data = Data()

    override fun getState(): Data = data

    override fun loadState(state: Data) {
        data = state
    }

    @Synchronized
    fun migrateLegacyPreference(overrideConfig: Boolean, showUnusedKeys: Boolean) {
        if (!data.initialized && overrideConfig) {
            data.showUnusedKeys = showUnusedKeys
            data.initialized = true
        }
    }

    fun setShowUnusedKeys(value: Boolean) {
        data.showUnusedKeys = value
        data.initialized = true
    }

    companion object {
        fun getInstance(): LocaleBreezePreferences = service()
    }
}
