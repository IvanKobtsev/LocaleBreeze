package dev.localebreeze.jetbrains

import com.google.gson.Gson
import com.intellij.codeInsight.daemon.DaemonCodeAnalyzer
import com.intellij.lang.annotation.AnnotationHolder
import com.intellij.lang.annotation.Annotator
import com.intellij.lang.annotation.HighlightSeverity
import com.intellij.openapi.actionSystem.ActionUpdateThread
import com.intellij.openapi.actionSystem.AnAction
import com.intellij.openapi.actionSystem.AnActionEvent
import com.intellij.openapi.actionSystem.CommonDataKeys
import com.intellij.openapi.application.ApplicationManager
import com.intellij.openapi.command.WriteCommandAction
import com.intellij.openapi.components.Service
import com.intellij.openapi.components.service
import com.intellij.openapi.editor.DefaultLanguageHighlighterColors
import com.intellij.openapi.editor.Document
import com.intellij.openapi.editor.colors.TextAttributesKey
import com.intellij.openapi.fileEditor.FileDocumentManager
import com.intellij.openapi.fileTypes.PlainSyntaxHighlighter
import com.intellij.openapi.fileTypes.SyntaxHighlighter
import com.intellij.openapi.options.colors.AttributesDescriptor
import com.intellij.openapi.options.colors.ColorDescriptor
import com.intellij.openapi.options.colors.ColorSettingsPage
import com.intellij.openapi.project.Project
import com.intellij.openapi.ui.Messages
import com.intellij.openapi.util.TextRange
import com.intellij.openapi.vfs.VirtualFile
import com.intellij.openapi.vfs.VirtualFileManager
import com.intellij.platform.lsp.api.LspClient
import com.intellij.platform.lsp.api.LspClientManager
import com.intellij.psi.PsiElement
import com.intellij.psi.PsiFile
import com.intellij.psi.PsiDocumentManager
import com.intellij.util.concurrency.AppExecutorUtil
import org.eclipse.lsp4j.ExecuteCommandParams
import org.eclipse.lsp4j.Position
import java.util.concurrent.Callable
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.TimeUnit
import javax.swing.Icon

object LocaleBreezeColors {
    val KEY_STRING: TextAttributesKey = TextAttributesKey.createTextAttributesKey(
        "LOCALE_BREEZE_KEY_STRING",
        DefaultLanguageHighlighterColors.STRING,
    )
}

class LocaleBreezeColorSettingsPage : ColorSettingsPage {
    override fun getIcon(): Icon? = null
    override fun getHighlighter(): SyntaxHighlighter = PlainSyntaxHighlighter()
    override fun getDemoText(): String = """i18n.t(<translationKey>'Page.title'</translationKey>);
const ordinary = 'Page.title';"""
    override fun getAdditionalHighlightingTagToDescriptorMap(): Map<String, TextAttributesKey> =
        mapOf("translationKey" to LocaleBreezeColors.KEY_STRING)
    override fun getAttributeDescriptors(): Array<AttributesDescriptor> =
        arrayOf(AttributesDescriptor("Translation key string", LocaleBreezeColors.KEY_STRING))
    override fun getColorDescriptors(): Array<ColorDescriptor> = ColorDescriptor.EMPTY_ARRAY
    override fun getDisplayName(): String = "LocaleBreeze"
}

data class LocaleBreezeKeyRange(
    val range: TextRange,
    val key: String,
    val declarationExists: Boolean,
    val canAdd: Boolean,
)

private data class CachedKeys(val stamp: Long, val version: Int?, val keys: List<LocaleBreezeKeyRange>)
private data class DocumentKeysPayload(val version: Int?, val keys: List<KeyPayload>)
private data class KeyPayload(
    val range: org.eclipse.lsp4j.Range,
    val key: String,
    val declarationExists: Boolean,
    val canAdd: Boolean,
)
data class PreparedKeyEdit(
    val uri: String,
    val version: Int?,
    val range: org.eclipse.lsp4j.Range,
    val newText: String,
)

@Service(Service.Level.PROJECT)
class LocaleBreezeKeyCache(private val project: Project) {
    private val values = ConcurrentHashMap<String, CachedKeys>()
    private val pending = ConcurrentHashMap.newKeySet<String>()
    private val gson = Gson()

    fun invalidate() {
        values.clear()
        pending.clear()
    }

    fun ranges(file: VirtualFile, document: Document): List<LocaleBreezeKeyRange> {
        if (!LocaleBreezeSettings.getInstance(project).state.enabled) return emptyList()
        ensure(file, document)
        return values[file.url]?.takeIf { it.stamp == document.modificationStamp }?.keys.orEmpty()
    }

    fun at(file: VirtualFile, document: Document, offset: Int): LocaleBreezeKeyRange? =
        ranges(file, document).firstOrNull { offset in it.range.startOffset..it.range.endOffset }

    fun prepareAdd(
        file: VirtualFile,
        document: Document,
        offset: Int,
        value: String,
    ): PreparedKeyEdit? {
        val client = client(file) ?: return null
        val line = document.getLineNumber(offset)
        val position = Position(line, offset - document.getLineStartOffset(line))
        val cachedVersion = values[file.url]
            ?.takeIf { it.stamp == document.modificationStamp }
            ?.version
        val result = AppExecutorUtil.getAppExecutorService().submit(Callable {
            client.sendRequestSync(2_500) { server ->
                server.workspaceService.executeCommand(
                    ExecuteCommandParams(
                        "localeBreeze.prepareAddKey",
                        listOf(
                            mapOf(
                                "textDocument" to mapOf("uri" to client.getDocumentIdentifier(file).uri),
                                "position" to mapOf("line" to position.line, "character" to position.character),
                                "value" to value,
                                "version" to cachedVersion,
                            ),
                        ),
                    ),
                )
            }
        }).get(3_000, TimeUnit.MILLISECONDS) ?: return null
        return gson.fromJson(gson.toJsonTree(result), PreparedKeyEdit::class.java)
    }

    private fun ensure(file: VirtualFile, document: Document) {
        val stamp = document.modificationStamp
        if (values[file.url]?.stamp == stamp || !pending.add(file.url)) return
        val client = client(file) ?: run { pending.remove(file.url); return }
        AppExecutorUtil.getAppExecutorService().execute {
            try {
                val result = client.sendRequestSync(2_500) { server ->
                    server.workspaceService.executeCommand(
                        ExecuteCommandParams(
                            "localeBreeze.documentKeys",
                            listOf(mapOf("uri" to client.getDocumentIdentifier(file).uri)),
                        ),
                    )
                } ?: return@execute
                val payload = gson.fromJson(gson.toJsonTree(result), DocumentKeysPayload::class.java)
                if (document.modificationStamp != stamp) return@execute
                val keys = payload.keys.mapNotNull { key ->
                    val start = document.offset(key.range.start) ?: return@mapNotNull null
                    val end = document.offset(key.range.end) ?: return@mapNotNull null
                    LocaleBreezeKeyRange(TextRange(start, end), key.key, key.declarationExists, key.canAdd)
                }
                values[file.url] = CachedKeys(stamp, payload.version, keys)
                ApplicationManager.getApplication().invokeLater {
                    if (!project.isDisposed) DaemonCodeAnalyzer.getInstance(project).restart()
                }
            } finally {
                pending.remove(file.url)
            }
        }
    }

    private fun client(file: VirtualFile): LspClient? =
        LspClientManager.getInstance(project)
            .getClients(LocaleBreezeLspIntegrationProvider::class.java)
            .firstOrNull { it.descriptor.isSupportedFile(file) }

}

class LocaleBreezeKeyAnnotator : Annotator {
    override fun annotate(element: PsiElement, holder: AnnotationHolder) {
        val file = element as? PsiFile ?: return
        if (!LocaleBreezeSettings.getInstance(file.project).state.enabled) return
        val virtualFile = file.virtualFile ?: return
        if (virtualFile.extension?.lowercase() !in setOf("js", "jsx", "ts", "tsx")) return
        val document = file.viewProvider.document ?: return
        for (key in file.project.service<LocaleBreezeKeyCache>().ranges(virtualFile, document)) {
            holder.newSilentAnnotation(HighlightSeverity.INFORMATION)
                .range(key.range)
                .textAttributes(LocaleBreezeColors.KEY_STRING)
                .create()
        }
    }
}

class LocaleBreezeAddKeyAction : AnAction() {
    override fun getActionUpdateThread(): ActionUpdateThread = ActionUpdateThread.BGT

    override fun update(event: AnActionEvent) {
        val project = event.project
        val file = event.getData(CommonDataKeys.VIRTUAL_FILE)
        val editor = event.getData(CommonDataKeys.EDITOR)
        val available = if (project != null && LocaleBreezeSettings.getInstance(project).state.enabled && file != null && editor != null) {
            project.service<LocaleBreezeKeyCache>()
                .at(file, editor.document, editor.caretModel.offset)
                ?.canAdd == true
        } else false
        event.presentation.isEnabledAndVisible = available
    }

    override fun actionPerformed(event: AnActionEvent) {
        val project = event.project ?: return
        val file = event.getData(CommonDataKeys.VIRTUAL_FILE) ?: return
        val editor = event.getData(CommonDataKeys.EDITOR) ?: return
        val key = project.service<LocaleBreezeKeyCache>()
            .at(file, editor.document, editor.caretModel.offset)
            ?.takeIf { it.canAdd } ?: return
        val value = Messages.showInputDialog(
            project,
            "Enter the default-locale value for ${key.key}",
            "Add Translation Key",
            Messages.getQuestionIcon(),
        ) ?: return
        val edit = runCatching {
            project.service<LocaleBreezeKeyCache>()
                .prepareAdd(file, editor.document, editor.caretModel.offset, value)
        }.getOrNull() ?: run {
            Messages.showWarningDialog(project, "The translation file changed or the key can no longer be added.", "LocaleBreeze")
            return
        }
        val target = VirtualFileManager.getInstance().findFileByUrl(edit.uri) ?: return
        val targetDocument = FileDocumentManager.getInstance().getDocument(target) ?: return
        val start = targetDocument.offset(edit.range.start) ?: return
        val end = targetDocument.offset(edit.range.end) ?: return
        WriteCommandAction.runWriteCommandAction(project) {
            targetDocument.replaceString(start, end, edit.newText)
            PsiDocumentManager.getInstance(project).commitDocument(targetDocument)
        }
    }
}

private fun Document.offset(position: Position): Int? {
    if (position.line !in 0 until lineCount) return null
    val start = getLineStartOffset(position.line)
    return (start + position.character).coerceAtMost(getLineEndOffset(position.line))
}
