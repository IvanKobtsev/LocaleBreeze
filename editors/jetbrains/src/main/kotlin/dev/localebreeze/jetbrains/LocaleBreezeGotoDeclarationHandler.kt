package dev.localebreeze.jetbrains

import com.intellij.json.psi.JsonProperty
import com.intellij.codeInsight.navigation.actions.GotoDeclarationHandler
import com.intellij.find.actions.ShowUsagesAction
import com.intellij.find.actions.ShowUsagesActionHandler
import com.intellij.find.actions.ShowUsagesParameters
import com.intellij.internal.statistic.eventLog.events.EventPair
import com.intellij.lang.Language
import com.intellij.openapi.actionSystem.DataContext
import com.intellij.openapi.diagnostic.Logger
import com.intellij.openapi.editor.Editor
import com.intellij.openapi.fileEditor.FileDocumentManager
import com.intellij.openapi.fileEditor.OpenFileDescriptor
import com.intellij.openapi.util.SystemInfoRt
import com.intellij.openapi.vfs.VirtualFile
import com.intellij.platform.lsp.api.LspClient
import com.intellij.platform.lsp.api.LspClientManager
import com.intellij.psi.PsiElement
import com.intellij.psi.PsiManager
import com.intellij.psi.SmartPointerManager
import com.intellij.psi.SmartPsiElementPointer
import com.intellij.psi.impl.FakePsiElement
import com.intellij.psi.search.GlobalSearchScope
import com.intellij.psi.search.SearchScope
import com.intellij.psi.util.PsiTreeUtil
import com.intellij.ui.awt.RelativePoint
import com.intellij.usageView.UsageInfo
import com.intellij.usages.UsageInfo2UsageAdapter
import com.intellij.usages.UsageSearchPresentation
import com.intellij.usages.UsageSearcher
import com.intellij.util.concurrency.AppExecutorUtil
import org.eclipse.lsp4j.DefinitionParams
import org.eclipse.lsp4j.Position
import org.eclipse.lsp4j.ReferenceContext
import org.eclipse.lsp4j.ReferenceParams
import java.util.concurrent.Callable
import java.util.concurrent.TimeUnit

/**
 * Makes LocaleBreeze navigation authoritative when the server recognizes a key.
 *
 * WebStorm otherwise combines LSP targets with its JavaScript/JSON symbol targets,
 * which creates duplicates and unrelated same-name destinations. Dictionary
 * declarations use the richer usages popup so each source location includes a
 * code preview instead of WebStorm's generic target-name chooser.
 */
class LocaleBreezeGotoDeclarationHandler : GotoDeclarationHandler {
    override fun getGotoDeclarationTargets(
        sourceElement: PsiElement?,
        offset: Int,
        editor: Editor?,
    ): Array<PsiElement>? {
        val element = sourceElement ?: return null
        val actualEditor = editor ?: return null
        val project = element.project
        val file = element.containingFile?.virtualFile ?: return null
        if (!LocaleBreezeLspIntegrationProvider.isSupported(file)) return null

        val document = actualEditor.document
        if (offset !in 0..document.textLength) return null
        val line = document.getLineNumber(offset)
        val position = Position(line, offset - document.getLineStartOffset(line))

        val clients = LspClientManager.getInstance(project)
            .getClients(LocaleBreezeLspIntegrationProvider::class.java)
            .filter { it.descriptor.isSupportedFile(file) }

        for (client in clients) {
            val params = DefinitionParams(client.getDocumentIdentifier(file), position)
            val definitions = requestFromServer("definition") {
                client.sendRequestSync(2_000) { server ->
                    server.textDocumentService.definition(params)
                }
            }

            val definitionTargets = buildList {
                definitions?.left?.forEach { add(Target(it.uri, it.range)) }
                definitions?.right?.forEach { add(Target(it.targetUri, it.targetSelectionRange)) }
            }
            val isDictionaryDeclaration =
                PsiTreeUtil.getParentOfType(element, JsonProperty::class.java, false) != null
            val hasSelfDefinition = definitionTargets.any { it.isOrigin(client, file, position) }
            val externalDefinitionTargets = definitionTargets.filterNot { it.isOrigin(client, file, position) }

            if (isDictionaryDeclaration && externalDefinitionTargets.isNotEmpty()) {
                val usages = mapTargets(client, externalDefinitionTargets)
                return arrayOf<PsiElement>(LocaleBreezeUsagesTarget(element, actualEditor, usages))
            }

            mapTargets(client, externalDefinitionTargets).takeIf { it.isNotEmpty() }?.let {
                return it.toTypedArray()
            }

            // Compatibility with servers that return the dictionary declaration itself.
            if (!isDictionaryDeclaration || !hasSelfDefinition) continue

            val references = requestFromServer("references") {
                client.sendRequestSync(2_000) { server ->
                    server.textDocumentService.references(
                        ReferenceParams(
                            client.getDocumentIdentifier(file),
                            position,
                            ReferenceContext(false),
                        ),
                    )
                }
            }.orEmpty()

            val usages = mapTargets(client, references.map { Target(it.uri, it.range) })
            return arrayOf<PsiElement>(LocaleBreezeUsagesTarget(element, actualEditor, usages))
        }

        // Preserve normal WebStorm navigation when LocaleBreeze has no result.
        return null
    }

    override fun getActionText(context: DataContext): String? = null

    private fun <T> requestFromServer(operation: String, request: () -> T): T? =
        runCatching {
            AppExecutorUtil.getAppExecutorService()
                .submit(Callable(request))
                .get(2_500, TimeUnit.MILLISECONDS)
        }.onFailure {
            log.warn("LocaleBreeze $operation navigation request failed", it)
        }.getOrNull()

    private fun mapTargets(client: LspClient, targets: List<Target>): List<PsiElement> {
        val psiManager = PsiManager.getInstance(client.project)
        val seen = mutableSetOf<String>()
        return targets.mapNotNull { target ->
            val virtualFile = client.descriptor.findFileByUri(target.uri) ?: return@mapNotNull null
            val identity = "${virtualFile.navigationIdentity()}:${target.range.start.line}:${target.range.start.character}:" +
                "${target.range.end.line}:${target.range.end.character}"
            if (!seen.add(identity)) return@mapNotNull null
            val psiFile = psiManager.findFile(virtualFile) ?: return@mapNotNull null
            val document = FileDocumentManager.getInstance().getDocument(virtualFile) ?: return@mapNotNull null
            val targetOffset = document.offset(target.range.start) ?: return@mapNotNull null
            psiFile.findElementAt(targetOffset) ?: psiFile
        }
    }

    companion object {
        private val log = Logger.getInstance(LocaleBreezeGotoDeclarationHandler::class.java)
    }
}

private class LocaleBreezeUsagesTarget(
    declaration: PsiElement,
    private val editor: Editor,
    usages: List<PsiElement>,
) : FakePsiElement() {
    private val declarationPointer = SmartPointerManager.createPointer(declaration)
    private val usagePointers = usages.map(SmartPointerManager::createPointer)

    override fun getParent(): PsiElement? = declarationPointer.element

    override fun getName(): String? = declarationPointer.element?.text

    override fun canNavigate(): Boolean = true

    override fun canNavigateToSource(): Boolean = true

    override fun navigate(requestFocus: Boolean) {
        val declaration = declarationPointer.element ?: return
        val validUsages = usagePointers.mapNotNull(SmartPsiElementPointer<PsiElement>::getElement)
        if (validUsages.isEmpty()) return
        if (validUsages.size == 1) {
            val usage = validUsages.single()
            val usageFile = usage.containingFile?.virtualFile ?: return
            OpenFileDescriptor(usage.project, usageFile, usage.textOffset).navigate(requestFocus)
            return
        }

        val handler = LocaleBreezeShowUsagesHandler(declaration, validUsages)
        val parameters = ShowUsagesParameters.initial(
            declaration.project,
            editor,
            RelativePoint.getCenterOf(editor.contentComponent),
        )
        ShowUsagesAction.showElementUsagesWithResult(
            parameters,
            handler,
            handler.createUsageView(declaration.project),
        )
    }
}

private class LocaleBreezeShowUsagesHandler(
    private val declaration: PsiElement,
    usageElements: List<PsiElement>,
) : ShowUsagesActionHandler {
    private val usages = usageElements.map { UsageInfo2UsageAdapter(UsageInfo(it)) }
    private val scope = GlobalSearchScope.projectScope(declaration.project)

    override fun isValid(): Boolean = declaration.isValid

    override fun getPresentation(): UsageSearchPresentation = object : UsageSearchPresentation {
        override fun getSearchTargetString(): String = declaration.text
        override fun getOptionsString(): String = "LocaleBreeze usages"
    }

    override fun createUsageSearcher(): UsageSearcher = UsageSearcher { processor ->
        usages.forEach { if (!processor.process(it)) return@UsageSearcher }
    }

    override fun findUsages() = Unit

    override fun showDialog(): ShowUsagesActionHandler = this

    override fun withScope(scope: SearchScope): ShowUsagesActionHandler = this

    override fun moreUsages(parameters: ShowUsagesParameters): ShowUsagesParameters = parameters.moreUsages()

    override fun getSelectedScope(): SearchScope = scope

    override fun getMaximalScope(): SearchScope = scope

    override fun getTargetLanguage(): Language = declaration.language

    override fun getTargetClass(): Class<*> = declaration.javaClass

    // WebStorm appends its own telemetry fields to both returned lists.
    override fun getEventData(): List<EventPair<*>> = mutableListOf()

    override fun navigateToSingleUsageImmediately(): Boolean = false

    override fun buildFinishEventData(usage: UsageInfo?): List<EventPair<*>> = mutableListOf()
}

private data class Target(val uri: String, val range: org.eclipse.lsp4j.Range) {
    fun isOrigin(client: LspClient, file: VirtualFile, position: Position): Boolean {
        val targetFile = client.descriptor.findFileByUri(uri)
        val sameFile = targetFile == file ||
            targetFile?.navigationIdentity() == file.navigationIdentity() ||
            (SystemInfoRt.isWindows && uri.equals(client.descriptor.getFileUri(file), ignoreCase = true))
        return sameFile && position >= range.start && position <= range.end
    }
}

private fun VirtualFile.navigationIdentity(): String =
    if (SystemInfoRt.isWindows) path.lowercase() else path

private operator fun Position.compareTo(other: Position): Int =
    compareValuesBy(this, other, Position::getLine, Position::getCharacter)

private fun com.intellij.openapi.editor.Document.offset(position: Position): Int? {
    if (position.line !in 0 until lineCount) return null
    val start = getLineStartOffset(position.line)
    val end = getLineEndOffset(position.line)
    return (start + position.character).coerceIn(start, end)
}
