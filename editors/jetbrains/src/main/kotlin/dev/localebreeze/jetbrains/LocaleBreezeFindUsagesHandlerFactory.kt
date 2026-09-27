package dev.localebreeze.jetbrains

import com.intellij.find.findUsages.FindUsagesHandler
import com.intellij.find.findUsages.FindUsagesHandlerFactory
import com.intellij.find.findUsages.FindUsagesOptions
import com.intellij.openapi.util.Key
import com.intellij.psi.PsiElement
import com.intellij.psi.SmartPsiElementPointer
import com.intellij.usageView.UsageInfo
import com.intellij.util.Processor

internal val localeBreezeUsagesKey =
    Key.create<List<SmartPsiElementPointer<PsiElement>>>("dev.localebreeze.jetbrains.usages")

class LocaleBreezeFindUsagesHandlerFactory : FindUsagesHandlerFactory() {
    override fun canFindUsages(element: PsiElement): Boolean =
        element.getUserData(localeBreezeUsagesKey) != null

    override fun createFindUsagesHandler(
        element: PsiElement,
        forHighlightUsages: Boolean,
    ): FindUsagesHandler? {
        val pointers = element.getUserData(localeBreezeUsagesKey) ?: return null
        element.putUserData(localeBreezeUsagesKey, null)
        return LocaleBreezeFindUsagesHandler(element, pointers)
    }
}

private class LocaleBreezeFindUsagesHandler(
    element: PsiElement,
    private val usagePointers: List<SmartPsiElementPointer<PsiElement>>,
) : FindUsagesHandler(element) {
    override fun processElementUsages(
        element: PsiElement,
        processor: Processor<in UsageInfo>,
        options: FindUsagesOptions,
    ): Boolean {
        for (pointer in usagePointers) {
            val usage = pointer.element ?: continue
            if (!processor.process(UsageInfo(usage))) return false
        }
        return true
    }
}
