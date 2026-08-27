package dev.localebreeze.jetbrains

import com.google.gson.JsonParser
import com.intellij.json.psi.JsonObject
import com.intellij.json.psi.JsonProperty
import com.intellij.openapi.actionSystem.ActionUpdateThread
import com.intellij.openapi.actionSystem.AnAction
import com.intellij.openapi.actionSystem.AnActionEvent
import com.intellij.openapi.actionSystem.CommonDataKeys
import com.intellij.openapi.ide.CopyPasteManager
import com.intellij.openapi.project.Project
import com.intellij.openapi.wm.StatusBar
import com.intellij.psi.PsiDocumentManager
import com.intellij.psi.util.PsiTreeUtil
import java.awt.datatransfer.StringSelection
import java.nio.file.Files
import java.nio.file.Path

class LocaleBreezeCopyFullKeyAction : AnAction() {
    override fun getActionUpdateThread(): ActionUpdateThread = ActionUpdateThread.BGT

    override fun update(event: AnActionEvent) {
        val file = event.getData(CommonDataKeys.VIRTUAL_FILE)
        event.presentation.isEnabledAndVisible =
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
        CopyPasteManager.getInstance().setContents(StringSelection(key))
        StatusBar.Info.set("LocaleBreeze: Copied $key", project)
    }

    private fun keySeparator(project: Project): String {
        val configured = LocaleBreezeSettings.getInstance(project).state.configPath
        val path = if (configured.isBlank()) {
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
