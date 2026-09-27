package dev.localebreeze.jetbrains

import com.intellij.openapi.application.ApplicationManager
import com.intellij.openapi.components.PersistentStateComponent
import com.intellij.openapi.components.Service
import com.intellij.openapi.components.State
import com.intellij.openapi.components.Storage
import com.intellij.openapi.components.StoragePathMacros
import com.intellij.openapi.module.ModuleManager
import com.intellij.openapi.project.Project
import com.intellij.openapi.roots.ModuleRootManager
import com.intellij.openapi.roots.ModuleRootModificationUtil
import com.intellij.openapi.roots.ProjectFileIndex
import com.intellij.openapi.roots.ProjectRootManager
import com.intellij.openapi.util.SystemInfoRt
import com.intellij.openapi.vfs.LocalFileSystem
import com.intellij.openapi.vfs.VfsUtilCore
import java.nio.file.Files
import java.nio.file.Path

@Service(Service.Level.PROJECT)
@State(name = "LocaleBreezeContentRoots", storages = [Storage(StoragePathMacros.WORKSPACE_FILE)])
class LocaleBreezeContentRoots(private val project: Project) : PersistentStateComponent<LocaleBreezeContentRoots.StoredState> {
    data class StoredState(
        var attachedPaths: MutableList<String> = mutableListOf(),
        var attachmentModelVersion: Int = 0,
    )

    private var storedState = StoredState()

    override fun getState(): StoredState = storedState

    override fun loadState(state: StoredState) {
        storedState = state
    }

    fun isAttached(path: Path): Boolean {
        val normalized = path.toAbsolutePath().normalize()
        val root = LocalFileSystem.getInstance().refreshAndFindFileByNioFile(normalized) ?: return false
        return ApplicationManager.getApplication().runReadAction<Boolean> {
            ProjectFileIndex.getInstance(project).isInContent(root) ||
                ProjectRootManager.getInstance(project).contentRoots.any {
                    VfsUtilCore.isAncestor(it, root, false)
                }
        }
    }

    fun attach(path: Path): Boolean {
        val normalized = path.toAbsolutePath().normalize()
        val root = LocalFileSystem.getInstance().refreshAndFindFileByNioFile(normalized) ?: return false
        var moduleAvailable = false
        ApplicationManager.getApplication().runWriteAction {
            val module = ModuleManager.getInstance(project).modules.firstOrNull()
            if (module != null) {
                moduleAvailable = true
                if (ModuleRootManager.getInstance(module).contentEntries.none { it.url == root.url }) {
                    ModuleRootModificationUtil.addContentRoot(module, root)
                }
            }
        }
        if (!moduleAvailable) return false
        val value = normalized.toString()
        if (value !in storedState.attachedPaths) storedState.attachedPaths.add(value)
        storedState.attachmentModelVersion = CURRENT_ATTACHMENT_MODEL_VERSION
        return isAttached(normalized)
    }

    fun reconcile(activePath: String) {
        if (activePath.isBlank()) return
        val active = runCatching { Path.of(activePath).toAbsolutePath().normalize() }.getOrNull() ?: return
        val stale = storedState.attachedPaths.filterNot { value ->
            runCatching { samePath(Path.of(value), active) }.getOrDefault(false)
        }
        val ownsActive = storedState.attachedPaths.any { value ->
            runCatching { samePath(Path.of(value), active) }.getOrDefault(false)
        }
        val needsMigration = ownsActive && storedState.attachmentModelVersion < CURRENT_ATTACHMENT_MODEL_VERSION
        if (stale.isEmpty() && !needsMigration) return
        ApplicationManager.getApplication().invokeLater {
            if (project.isDisposed) return@invokeLater
            val activeRoot = needsMigration
                .takeIf { it }
                ?.let { LocalFileSystem.getInstance().findFileByNioFile(active) }
            val staleRoots = stale.mapNotNull { value ->
                runCatching { Path.of(value) }.getOrNull()
                    ?.let { LocalFileSystem.getInstance().findFileByNioFile(it) }
            }
            ApplicationManager.getApplication().runWriteAction {
                val module = ModuleManager.getInstance(project).modules.firstOrNull()
                if (module != null && staleRoots.isNotEmpty()) {
                    ModuleRootModificationUtil.updateModel(module) { model ->
                        val staleUrls = staleRoots.mapTo(mutableSetOf()) { it.url }
                        model.contentEntries
                            .filter { it.url in staleUrls }
                            .forEach(model::removeContentEntry)
                    }
                }
                if (module != null && activeRoot != null) {
                    ModuleRootModificationUtil.addContentRoot(module, activeRoot)
                    storedState.attachmentModelVersion = CURRENT_ATTACHMENT_MODEL_VERSION
                }
            }
            storedState.attachedPaths.removeAll(stale.toSet())
        }
    }

    private fun samePath(left: Path, right: Path): Boolean {
        if (runCatching { Files.isSameFile(left, right) }.getOrDefault(false)) return true
        val leftValue = left.toAbsolutePath().normalize().toString()
        val rightValue = right.toAbsolutePath().normalize().toString()
        return if (SystemInfoRt.isWindows) leftValue.equals(rightValue, ignoreCase = true) else leftValue == rightValue
    }

    companion object {
        private const val CURRENT_ATTACHMENT_MODEL_VERSION = 1
    }
}
