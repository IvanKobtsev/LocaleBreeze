package dev.localebreeze.jetbrains

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse

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
}
