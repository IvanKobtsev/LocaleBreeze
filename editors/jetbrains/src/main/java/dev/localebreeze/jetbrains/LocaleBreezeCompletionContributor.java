package dev.localebreeze.jetbrains;

import com.intellij.codeInsight.completion.CompletionContributor;
import com.intellij.codeInsight.completion.CompletionParameters;
import com.intellij.codeInsight.completion.PrioritizedLookupElement;
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
            if (!hasLocaleBreezeResults) {
                result.passResult(completionResult);
            } else if (isLocaleBreezeCompletion(completionResult)) {
                LspLookupElementDecorator lookupElement =
                    (LspLookupElementDecorator)completionResult.getLookupElement();
                // WebStorm applies its own fuzzy relevance after receiving LSP
                // results. Promote the server's sortText into the IDE's
                // highest-precedence priority while retaining the LSP matcher.
                double priority = priorityFromSortText(
                    lookupElement.getObject().getCompletionItem().getSortText()
                );
                result.passResult(completionResult.withLookupElement(
                    PrioritizedLookupElement.withPriority(lookupElement, priority)
                ));
            }
        }

        result.stopHere();
    }

    private static boolean isLocaleBreezeCompletion(CompletionResult result) {
        if (!(result.getLookupElement() instanceof LspLookupElementDecorator lookupElement)) {
            return false;
        }

        return "locale-breeze".equals(lookupElement.getObject().getCompletionItem().getData());
    }

    static double priorityFromSortText(String sortText) {
        if (sortText == null) return 0;
        int separator = sortText.lastIndexOf('-');
        if (separator < 0 || separator == sortText.length() - 1) return 0;
        try {
            return 10_000.0 - Long.parseLong(sortText.substring(separator + 1));
        } catch (NumberFormatException ignored) {
            return 0;
        }
    }
}
