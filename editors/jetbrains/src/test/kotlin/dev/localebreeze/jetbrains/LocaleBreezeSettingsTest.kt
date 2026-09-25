package dev.localebreeze.jetbrains

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertTrue

class LocaleBreezeSettingsTest {
    @Test
    fun `unused keys are shown by default`() {
        assertTrue(LocaleBreezePreferences.Data().showUnusedKeys)
    }

    @Test
    fun `first explicit legacy override seeds global preference once`() {
        val preferences = LocaleBreezePreferences()
        preferences.migrateLegacyPreference(overrideConfig = true, showUnusedKeys = false)
        preferences.migrateLegacyPreference(overrideConfig = true, showUnusedKeys = true)

        assertFalse(preferences.state.showUnusedKeys)
        assertTrue(preferences.state.initialized)
    }

    @Test
    fun `workspace is disabled by default`() {
        val settings = LocaleBreezeSettings()
        val state = settings.state

        assertFalse(state.enabled)
        assertEquals(LocaleBreezeSettings.ActivationMode.AUTO, settings.activationMode())
        assertFalse(settings.isEnabledInSettings())
        assertEquals("", state.developmentServerPath)
    }

    @Test
    fun `runtime activation is presented as enabled for automatic workspaces`() {
        val settings = LocaleBreezeSettings()
        settings.state.enabled = true

        assertTrue(settings.isEnabledInSettings())
    }

    @Test
    fun `explicit enable remains selected while configuration is unresolved`() {
        val settings = LocaleBreezeSettings()
        settings.setActivationMode(LocaleBreezeSettings.ActivationMode.ENABLED)

        assertFalse(settings.state.enabled)
        assertTrue(settings.isEnabledInSettings())
    }

    @Test
    fun `warning identity is stable for category and path`() {
        val issue = LocaleBreezeWorkspaceIssue(code = "config_json", path = "locale-breeze.json")
        assertEquals("config_json:locale-breeze.json", issue.identity)
    }

    @Test
    fun `legacy disabled workspace migrates to automatic activation`() {
        val settings = LocaleBreezeSettings()
        settings.loadState(LocaleBreezeSettings.Data(enabled = false))

        assertEquals(LocaleBreezeSettings.ActivationMode.AUTO, settings.activationMode())
        assertEquals(3, settings.state.schemaVersion)
        assertEquals(LocaleBreezeSettings.ConfigLocation.WORKSPACE_ROOT, settings.configLocation())
    }

    @Test
    fun `legacy enabled workspace remains explicitly enabled`() {
        val settings = LocaleBreezeSettings()
        settings.loadState(LocaleBreezeSettings.Data(enabled = true))

        assertEquals(LocaleBreezeSettings.ActivationMode.ENABLED, settings.activationMode())
        assertTrue(settings.state.enabled)
    }

    @Test
    fun `legacy configured path migrates to custom location`() {
        val settings = LocaleBreezeSettings()
        settings.loadState(LocaleBreezeSettings.Data(configPath = "packages/app/locale-breeze.json"))

        assertEquals(LocaleBreezeSettings.ConfigLocation.CUSTOM_PATH, settings.configLocation())
    }

    @Test
    fun `manual disable is persisted separately from runtime state`() {
        val settings = LocaleBreezeSettings()
        settings.setActivationMode(LocaleBreezeSettings.ActivationMode.DISABLED)

        assertEquals(LocaleBreezeSettings.ActivationMode.DISABLED, settings.activationMode())
        assertFalse(settings.state.enabled)
    }

    @Test
    fun `windows config path matching ignores casing`() {
        if (!System.getProperty("os.name").startsWith("Windows", ignoreCase = true)) return

        assertTrue(LocaleBreezeConfigDiscovery.pathsEqual(
            java.nio.file.Path.of("C:/Project/locale-breeze.json"),
            java.nio.file.Path.of("c:/project/locale-breeze.json"),
        ))
    }
}
