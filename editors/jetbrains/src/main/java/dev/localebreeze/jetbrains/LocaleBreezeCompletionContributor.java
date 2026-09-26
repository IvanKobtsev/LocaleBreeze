package dev.localebreeze.jetbrains;

import com.intellij.codeInsight.completion.CompletionContributor;
import com.intellij.codeInsight.completion.CompletionParameters;
import com.intellij.codeInsight.completion.CompletionResult;
import com.intellij.codeInsight.completion.CompletionResultSet;
import com.intellij.platform.lsp.impl.features.completion.LspLookupElementDecorator;
import org.jetbrains.annotations.NotNull;

import java.util.LinkedHashSet;

/**
 * Keeps translation-key completion deterministic when WebStorm combines LSP
 * results with its own string-literal suggestions.
 *
 * <p>The original LSP lookup elements are passed through unchanged so their
 * rendering, insertion behavior, documentation, and resolve handling remain
 * owned by the platform LSP integration.</p>
 */
public final class LocaleBreezeCompletionContributor extends CompletionContributor {
    @Override
    public void fillCompletionVariants(
        @NotNull CompletionParameters parameters,
        @NotNull CompletionResultSet result
    ) {
        LinkedHashSet<CompletionResult> remainingResults =
            result.runRemainingContributors(parameters, false);
        boolean hasLocaleBreezeResults = remainingResults.stream()
            .anyMatch(LocaleBreezeCompletionContributor::isLocaleBreezeCompletion);

        for (CompletionResult completionResult : remainingResults) {
            if (!hasLocaleBreezeResults || isLocaleBreezeCompletion(completionResult)) {
                result.passResult(completionResult);
            }
        }

        result.stopHere();
    }

    private static boolean isLocaleBreezeCompletion(CompletionResult result) {
        if (!(result.getLookupElement() instanceof LspLookupElementDecorator lookupElement)) {
            return false;
        }

        return lookupElement.getObject().getLspClient().getProviderClass()
            == LocaleBreezeLspIntegrationProvider.class;
    }
}
