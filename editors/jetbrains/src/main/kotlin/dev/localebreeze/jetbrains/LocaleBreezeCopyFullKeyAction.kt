package dev.localebreeze.jetbrains

import com.intellij.openapi.actionSystem.ActionUpdateThread
import com.intellij.openapi.actionSystem.AnAction
import com.intellij.openapi.actionSystem.AnActionEvent
import com.intellij.openapi.actionSystem.CommonDataKeys
import com.intellij.openapi.application.ApplicationManager
import com.intellij.openapi.ide.CopyPasteManager
import com.intellij.openapi.wm.StatusBar
import com.intellij.platform.lsp.api.LspClientManager
import com.intellij.util.concurrency.AppExecutorUtil
import org.eclipse.lsp4j.ExecuteCommandParams
import org.eclipse.lsp4j.Position
import org.eclipse.lsp4j.TextDocumentPositionParams
import java.awt.datatransfer.StringSelection
import java.util.concurrent.CompletableFuture

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
        val file = event.getData(CommonDataKeys.VIRTUAL_FILE) ?: return
        val offset = editor.caretModel.offset
        val document = editor.document
        val line = document.getLineNumber(offset)
        val position = Position(line, offset - document.getLineStartOffset(line))
        val client = LspClientManager.getInstance(project)
            .getClients(LocaleBreezeLspIntegrationProvider::class.java)
            .firstOrNull { it.descriptor.isSupportedFile(file) }
            ?: return
        val argument = TextDocumentPositionParams(client.getDocumentIdentifier(file), position)
        val params = ExecuteCommandParams("localeBreeze.copyFullKey", listOf(argument))

        CompletableFuture.supplyAsync(
            {
                client.sendRequestSync(2_000) { server ->
                    server.workspaceService.executeCommand(params)
                }
            },
            AppExecutorUtil.getAppExecutorService(),
        ).whenComplete { result, error ->
            ApplicationManager.getApplication().invokeLater {
                if (project.isDisposed || error != null) return@invokeLater
                val key = result as? String
                if (key == null) {
                    StatusBar.Info.set("LocaleBreeze: Place the cursor on a translation key", project)
                    return@invokeLater
                }
                CopyPasteManager.getInstance().setContents(StringSelection(key))
                StatusBar.Info.set("LocaleBreeze: Copied $key", project)
            }
        }
    }
}
