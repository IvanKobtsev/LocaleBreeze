package dev.localebreeze.jetbrains

import com.intellij.codeInsight.daemon.DaemonCodeAnalyzer
import com.intellij.notification.NotificationAction
import com.intellij.notification.NotificationGroupManager
import com.intellij.notification.NotificationType
import com.intellij.openapi.application.WriteAction
import com.intellij.openapi.application.ModalityState
import com.intellij.openapi.application.ReadAction
import com.intellij.openapi.components.Service
import com.intellij.openapi.components.service
import com.intellij.openapi.fileEditor.OpenFileDescriptor
import com.intellij.openapi.project.Project
import com.intellij.openapi.vfs.LocalFileSystem
import com.intellij.openapi.wm.ToolWindowManager
import com.intellij.platform.lsp.api.LspClientManager
import com.intellij.psi.search.FilenameIndex
import com.intellij.psi.search.GlobalSearchScope
import com.intellij.util.Alarm
import com.intellij.util.concurrency.AppExecutorUtil
import java.nio.file.Files
import java.nio.file.Path
import java.util.concurrent.atomic.AtomicLong

sealed interface LocaleBreezeConfigSetupState {
    data object Disabled : LocaleBreezeConfigSetupState
    data object Searching : LocaleBreezeConfigSetupState
    data class RootConfig(val path: String) : LocaleBreezeConfigSetupState
    data class Configured(val path: String) : LocaleBreezeConfigSetupState
    data class Candidates(val paths: List<String>) : LocaleBreezeConfigSetupState
    data object NotFound : LocaleBreezeConfigSetupState
}

@Service(Service.Level.PROJECT)
class LocaleBreezeConfigDiscovery(private val project: Project) {
    private val generation = AtomicLong()
    private var notificationIdentity: String? = null
    private val refreshAlarm = Alarm(Alarm.ThreadToUse.SWING_THREAD, project)
    private var pendingRestart = false

    @Synchronized
    fun scheduleRefresh(restartConfigured: Boolean = false) {
        pendingRestart = pendingRestart || restartConfigured
        refreshAlarm.cancelAllRequests()
        refreshAlarm.addRequest({
            val restart = synchronized(this) {
                pendingRestart.also { pendingRestart = false }
            }
            refresh(restart)
        }, 200)
    }

    fun refresh(restartConfigured: Boolean = false) {
        val currentGeneration = generation.incrementAndGet()
        val settings = LocaleBreezeSettings.getInstance(project)
        if (settings.activationMode() == LocaleBreezeSettings.ActivationMode.DISABLED) {
            deactivate(LocaleBreezeConfigSetupState.Disabled)
            return
        }

        val configured = if (settings.configLocation() == LocaleBreezeSettings.ConfigLocation.CUSTOM_PATH) {
            resolveSavedPath(settings.state.configPath)
        } else null
        if (configured != null && Files.isRegularFile(configured)) {
            activate(LocaleBreezeConfigSetupState.Configured(configured.toString()), restartConfigured)
            return
        }

        val rootConfig = projectRoot()?.resolve(CONFIG_FILE_NAME)?.normalize()
        if (rootConfig != null && Files.isRegularFile(rootConfig)) {
            settings.state.configLocation = LocaleBreezeSettings.ConfigLocation.WORKSPACE_ROOT.name
            activate(LocaleBreezeConfigSetupState.RootConfig(rootConfig.toString()), restartConfigured)
            return
        }

        deactivate(LocaleBreezeConfigSetupState.Searching)
        ReadAction.nonBlocking<List<Path>> {
            FilenameIndex.getVirtualFilesByName(
                CONFIG_FILE_NAME,
                GlobalSearchScope.projectScope(project),
            ).map { it.toNioPath().toAbsolutePath().normalize() }
                .filter { rootConfig == null || !pathsEqual(it, rootConfig) }
                .distinctBy { normalizedIdentity(it) }
                .sortedBy(::displayPath)
        }.inSmartMode(project)
            .expireWith(project)
            .finishOnUiThread(ModalityState.any()) { candidates ->
            if (project.isDisposed || currentGeneration != generation.get()) return@finishOnUiThread
            val setup = if (candidates.isEmpty()) {
                LocaleBreezeConfigSetupState.NotFound
            } else {
                LocaleBreezeConfigSetupState.Candidates(candidates.map(Path::toString))
            }
            update(setup)
            if (setup is LocaleBreezeConfigSetupState.Candidates) {
                notifyCandidatesOnce(setup)
            } else {
                notificationIdentity = null
            }
        }.submit(AppExecutorUtil.getAppExecutorService())
    }

    fun applySelectedConfig(path: Path) {
        val normalized = path.toAbsolutePath().normalize()
        if (!Files.isRegularFile(normalized)) return
        generation.incrementAndGet()
        LocaleBreezeSettings.getInstance(project).apply {
            state.configPath = normalizeForStorage(normalized.toString()).ifBlank { CONFIG_FILE_NAME }
            state.configLocation = LocaleBreezeSettings.ConfigLocation.CUSTOM_PATH.name
            setActivationMode(LocaleBreezeSettings.ActivationMode.AUTO)
        }
        activate(LocaleBreezeConfigSetupState.Configured(normalized.toString()), restart = true)
    }

    fun createRootConfig() {
        val root = projectRoot() ?: return
        val config = root.resolve(CONFIG_FILE_NAME)
        generation.incrementAndGet()
        WriteAction.run<RuntimeException> {
            if (!Files.exists(config)) Files.writeString(config, STARTER_CONFIG)
        }
        val settings = LocaleBreezeSettings.getInstance(project)
        settings.state.configLocation = LocaleBreezeSettings.ConfigLocation.WORKSPACE_ROOT.name
        settings.setActivationMode(LocaleBreezeSettings.ActivationMode.AUTO)
        LocalFileSystem.getInstance().refreshAndFindFileByNioFile(config)?.let { file ->
            OpenFileDescriptor(project, file).navigate(true)
        }
        activate(LocaleBreezeConfigSetupState.RootConfig(config.toString()), restart = true)
    }

    fun leaveDisabled() {
        generation.incrementAndGet()
        LocaleBreezeSettings.getInstance(project).setActivationMode(LocaleBreezeSettings.ActivationMode.DISABLED)
        notificationIdentity = null
        deactivate(LocaleBreezeConfigSetupState.Disabled)
    }

    fun enableAndDiscover() {
        val settings = LocaleBreezeSettings.getInstance(project)
        settings.setActivationMode(LocaleBreezeSettings.ActivationMode.ENABLED)
        settings.state.enabled = false
        refresh()
    }

    fun normalizeForStorage(value: String): String {
        if (value.isBlank()) return ""
        val path = runCatching { Path.of(value) }.getOrNull() ?: return value
        val absolute = (if (path.isAbsolute) path else projectRoot()?.resolve(path) ?: path)
            .toAbsolutePath().normalize()
        val root = projectRoot() ?: return absolute.toString()
        return runCatching { root.relativize(absolute).toString() }.getOrDefault(absolute.toString())
    }

    fun concerns(path: String): Boolean {
        val changed = runCatching { Path.of(path).toAbsolutePath().normalize() }.getOrNull() ?: return false
        if (changed.fileName?.toString() == CONFIG_FILE_NAME) return true
        val configured = resolveSavedPath(LocaleBreezeSettings.getInstance(project).state.configPath)
        return configured != null && pathsEqual(changed, configured)
    }

    private fun activate(setup: LocaleBreezeConfigSetupState, restart: Boolean) {
        notificationIdentity = null
        LocaleBreezeSettings.getInstance(project).state.enabled = true
        update(setup)
        project.service<LocaleBreezeWarningCoordinator>().waiting()
        val clients = LspClientManager.getInstance(project)
        if (restart) clients.stopAndRestartClientsIfNeeded(LocaleBreezeLspIntegrationProvider::class.java)
        clients.startClientsIfNeeded(LocaleBreezeLspIntegrationProvider::class.java)
        if (restart) refreshEditorFeatures()
    }

    private fun deactivate(setup: LocaleBreezeConfigSetupState) {
        val settings = LocaleBreezeSettings.getInstance(project)
        val wasEnabled = settings.state.enabled
        settings.state.enabled = false
        update(setup)
        project.service<LocaleBreezeWarningCoordinator>().disabled()
        if (wasEnabled) {
            LspClientManager.getInstance(project)
                .stopAndRestartClientsIfNeeded(LocaleBreezeLspIntegrationProvider::class.java)
            refreshEditorFeatures()
        }
    }

    private fun refreshEditorFeatures() {
        project.service<LocaleBreezeKeyCache>().invalidate()
        DaemonCodeAnalyzer.getInstance(project).restart()
    }

    private fun update(setup: LocaleBreezeConfigSetupState) {
        project.service<LocaleBreezeWarningCoordinator>().setup(setup)
    }

    private fun notifyCandidatesOnce(setup: LocaleBreezeConfigSetupState.Candidates) {
        val identity = setup.paths.joinToString("|")
        if (notificationIdentity == identity) return
        notificationIdentity = identity
        val count = setup.paths.size
        NotificationGroupManager.getInstance().getNotificationGroup("LocaleBreeze")
            .createNotification(
                "LocaleBreeze configuration found",
                if (count == 1) "A configuration was found in this project."
                else "$count configurations were found in this project.",
                NotificationType.INFORMATION,
            )
            .addAction(NotificationAction.createSimpleExpiring("Open LocaleBreeze") {
                enableAndDiscover()
                ToolWindowManager.getInstance(project).getToolWindow("LocaleBreeze")?.show(null)
            })
            .addAction(NotificationAction.createSimpleExpiring("Leave disabled") { leaveDisabled() })
            .notify(project)
    }

    private fun resolveSavedPath(value: String): Path? {
        if (value.isBlank()) return null
        val path = runCatching { Path.of(value) }.getOrNull() ?: return null
        return (if (path.isAbsolute) path else projectRoot()?.resolve(path) ?: path).normalize()
    }

    private fun projectRoot(): Path? = project.basePath?.let(Path::of)?.toAbsolutePath()?.normalize()

    private fun displayPath(path: Path): String {
        val root = projectRoot() ?: return path.toString()
        return runCatching { root.relativize(path).toString() }.getOrDefault(path.toString())
    }

    private fun normalizedIdentity(path: Path): String =
        if (System.getProperty("os.name").startsWith("Windows", ignoreCase = true)) {
            path.toString().lowercase()
        } else path.toString()

    companion object {
        const val CONFIG_FILE_NAME = "locale-breeze.json"
        private val STARTER_CONFIG = """{
  "${'$'}schema": "https://raw.githubusercontent.com/IvanKobtsev/LocaleBreeze/main/schemas/config-v1.schema.json",
  "dictionaries": "public/dictionaries/translation.{locale}.json",
  "defaultLocale": "en",
  "keySeparator": ".",
  "scopedFunctions": ["useScopedTranslation"],
  "translationMethods": ["t"],
  "fullKeyFunctions": ["i18next.t"],
  "translationKeyTypes": ["TranslationKey"],
  "translationKeyProps": ["transKey"],
  "ignoredScopes": ["Server_Errors"]
}
"""

        fun pathsEqual(left: Path, right: Path): Boolean =
            if (System.getProperty("os.name").startsWith("Windows", ignoreCase = true)) {
                left.normalize().toString().equals(right.normalize().toString(), ignoreCase = true)
            } else {
                left.normalize() == right.normalize()
            }
    }
}
