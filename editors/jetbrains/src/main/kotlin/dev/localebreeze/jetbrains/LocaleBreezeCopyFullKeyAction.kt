package dev.localebreeze.jetbrains

import com.google.gson.JsonParser
import com.intellij.json.psi.JsonObject
import com.intellij.json.psi.JsonProperty
import com.intellij.openapi.actionSystem.ActionUpdateThread
import com.intellij.openapi.actionSystem.AnAction
import com.intellij.openapi.actionSystem.AnActionEvent
import com.intellij.openapi.actionSystem.CommonDataKeys
import com.intellij.openapi.application.ApplicationManager
import com.intellij.openapi.ide.CopyPasteManager
import com.intellij.openapi.project.Project
import com.intellij.openapi.wm.StatusBar
import com.intellij.platform.lsp.api.LspClientManager
import com.intellij.psi.PsiDocumentManager
import com.intellij.psi.util.PsiTreeUtil
import com.intellij.util.concurrency.AppExecutorUtil
import java.awt.datatransfer.StringSelection
import java.nio.file.Files
import java.nio.file.Path
import org.eclipse.lsp4j.ExecuteCommandParams
import org.eclipse.lsp4j.Position

open class LocaleBreezeCopyFullKeyAction(
    private val includeNamespace: Boolean = false,
) : AnAction() {
    override fun getActionUpdateThread(): ActionUpdateThread = ActionUpdateThread.BGT

    override fun update(event: AnActionEvent) {
        val project = event.project
        val file = event.getData(CommonDataKeys.VIRTUAL_FILE)
        event.presentation.isEnabledAndVisible =
            project != null && LocaleBreezeSettings.getInstance(project).state.enabled &&
                file?.extension.equals("json", ignoreCase = true) &&
                event.getData(CommonDataKeys.EDITOR) != null
    }

    override fun actionPerformed(event: AnActionEvent) {
        val project = event.project ?: return
        val editor = event.getData(CommonDataKeys.EDITOR) ?: return
        val psiFile = PsiDocumentManager.getInstance(project).getPsiFile(editor.document) ?: return
        if (psiFile.textLength == 0) return
        val offset = editor.caretModel.offset.coerceAtMost(psiFile.textLength - 1)
        val element = psiFile.findElementAt(offset)
        val property = PsiTreeUtil.getParentOfType(element, JsonProperty::class.java, false)
        if (property == null) {
            StatusBar.Info.set("LocaleBreeze: Place the cursor on a translation key", project)
            return
        }

        val segments = buildList {
            var current: JsonProperty? = property
            while (current != null) {
                add(current.name)
                current = (current.parent as? JsonObject)?.parent as? JsonProperty
            }
        }
        val key = segments.asReversed().joinToString(keySeparator(project))
        if (!includeNamespace) {
            copyKey(project, key)
            return
        }

        val file = event.getData(CommonDataKeys.VIRTUAL_FILE) ?: return
        val document = editor.document
        val line = document.getLineNumber(offset)
        val position = Position(line, offset - document.getLineStartOffset(line))
        val clients = LspClientManager.getInstance(project)
            .getClients(LocaleBreezeLspIntegrationProvider::class.java)
            .filter { it.descriptor.isSupportedFile(file) }
        if (clients.isEmpty()) {
            StatusBar.Info.set("LocaleBreeze: The language server is not running", project)
            return
        }

        AppExecutorUtil.getAppExecutorService().execute {
            val qualifiedKey: String? = clients.firstNotNullOfOrNull { client ->
                runCatching {
                    client.sendRequestSync(2_000) { server ->
                        server.workspaceService.executeCommand(
                            ExecuteCommandParams(
                                "localeBreeze.resolveFullKey",
                                listOf(
                                    mapOf(
                                        "textDocument" to mapOf(
                                            "uri" to client.getDocumentIdentifier(file).uri,
                                        ),
                                        "position" to mapOf(
                                            "line" to position.line,
                                            "character" to position.character,
                                        ),
                                    ),
                                ),
                            ),
                        )
                    } as? String
                }.getOrNull()
            }
            ApplicationManager.getApplication().invokeLater {
                if (project.isDisposed) return@invokeLater
                if (qualifiedKey == null) {
                    StatusBar.Info.set("LocaleBreeze: Could not resolve the translation namespace", project)
                } else {
                    copyKey(project, qualifiedKey)
                }
            }
        }
    }

    private fun copyKey(project: Project, key: String) {
        CopyPasteManager.getInstance().setContents(StringSelection(key))
        StatusBar.Info.set("LocaleBreeze: Copied $key", project)
    }

    private fun keySeparator(project: Project): String {
        val settings = LocaleBreezeSettings.getInstance(project)
        val configured = settings.state.configPath
        val path = if (settings.configLocation() == LocaleBreezeSettings.ConfigLocation.WORKSPACE_ROOT) {
            project.basePath?.let(Path::of)?.resolve("locale-breeze.json")
        } else {
            Path.of(configured).let { configuredPath ->
                if (configuredPath.isAbsolute) configuredPath
                else project.basePath?.let(Path::of)?.resolve(configuredPath)
            }
        } ?: return "."
        return runCatching {
            JsonParser.parseString(Files.readString(path))
                .asJsonObject
                .get("keySeparator")
                ?.asString
                ?.takeIf(String::isNotEmpty)
        }.getOrNull() ?: "."
    }
}

class LocaleBreezeCopyFullKeyWithNamespaceAction : LocaleBreezeCopyFullKeyAction(includeNamespace = true)
