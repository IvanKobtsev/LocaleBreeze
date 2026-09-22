package dev.localebreeze.jetbrains

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertTrue

class LocaleBreezeSettingsTest {
    @Test
    fun `workspace is disabled by default`() {
        assertFalse(LocaleBreezeSettings.Data().enabled)
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
        assertEquals(1, settings.state.schemaVersion)
    }

    @Test
    fun `legacy enabled workspace remains explicitly enabled`() {
        val settings = LocaleBreezeSettings()
        settings.loadState(LocaleBreezeSettings.Data(enabled = true))

        assertEquals(LocaleBreezeSettings.ActivationMode.ENABLED, settings.activationMode())
        assertTrue(settings.state.enabled)
    }

    @Test
    fun `manual disable is persisted separately from runtime state`() {
        val settings = LocaleBreezeSettings()
        settings.setActivationMode(LocaleBreezeSettings.ActivationMode.DISABLED)

        assertEquals(LocaleBreezeSettings.ActivationMode.DISABLED, settings.activationMode())
        assertFalse(settings.state.enabled)
    }
}
