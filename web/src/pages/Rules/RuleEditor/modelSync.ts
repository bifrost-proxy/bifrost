export function normalizeRuleEditorContent(
  content: string | undefined | null,
): string {
  return (content ?? "").replace(/\r\n/g, "\n").replace(/\n+$/g, "");
}

interface ShouldReplaceRuleEditorModelOptions {
  currentContent: string;
  nextContent: string;
  currentRuleName: string | null;
  nextRuleName: string;
}

export function shouldReplaceRuleEditorModel({
  currentContent,
  nextContent,
  currentRuleName,
  nextRuleName,
}: ShouldReplaceRuleEditorModelOptions): boolean {
  if (currentContent === nextContent) return false;
  if (currentRuleName !== nextRuleName) return true;

  return (
    normalizeRuleEditorContent(currentContent) !==
    normalizeRuleEditorContent(nextContent)
  );
}
