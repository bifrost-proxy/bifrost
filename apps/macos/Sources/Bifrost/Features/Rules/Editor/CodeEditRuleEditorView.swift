import AppKit
import BifrostNativeCore

#if canImport(CodeEditSourceEditor) && canImport(CodeEditTextView)
import CodeEditLanguages
import CodeEditSourceEditor
import CodeEditTextView
import SwiftUI

enum RuleEditorBackendAvailability {
    static let codeEditSourceEditor = true
}

struct CodeEditRuleEditorView: View {
    @Binding var text: String
    var context: BifrostRuleEditorContext
    var isReadOnly = false
    var onSave: () -> Void
    var onTextChanged: ((String) -> Void)?

    @StateObject private var bridge = CodeEditRuleEditorBridge()
    @State private var editorState = SourceEditorState()

    var body: some View {
        SourceEditor(
            $text,
            language: .default,
            configuration: configuration,
            state: $editorState,
            highlightProviders: [bridge.highlighter],
            coordinators: [bridge.coordinator],
            completionDelegate: bridge.completionDelegate
        )
        .id(editorIdentity)
        .onAppear {
            bridge.update(context: context, onSave: onSave, onTextChanged: onTextChanged)
        }
        .onChange(of: context) { newContext in
            bridge.update(context: newContext, onSave: onSave, onTextChanged: onTextChanged)
        }
        .onChange(of: isReadOnly) { _ in
            bridge.update(context: context, onSave: onSave, onTextChanged: onTextChanged)
        }
    }

    private var editorIdentity: String {
        [
            context.currentGroupName ?? "local",
            context.currentRuleName ?? "none",
        ].joined(separator: "/")
    }

    private var configuration: SourceEditorConfiguration {
        SourceEditorConfiguration(
            appearance: .init(
                theme: CodeEditRuleTheme.make(appearance: NSApp.effectiveAppearance),
                font: .monospacedSystemFont(ofSize: 13, weight: .regular),
                lineHeightMultiple: 1.2,
                letterSpacing: 0,
                wrapLines: true,
                tabWidth: 4
            ),
            behavior: .init(isEditable: !isReadOnly, indentOption: .spaces(count: 4)),
            layout: .init(contentInsets: NSEdgeInsets(top: 8, left: 0, bottom: 8, right: 0)),
            peripherals: .init(
                showGutter: true,
                showMinimap: false,
                showFoldingRibbon: false,
                codeSuggestionTriggerCharacters: bridge.completionDelegate.completionTriggerCharacters()
            )
        )
    }
}

@MainActor
private final class CodeEditRuleEditorBridge: ObservableObject {
    let highlighter = CodeEditRuleHighlightProvider()
    let coordinator = CodeEditRuleTextCoordinator()
    let completionDelegate = CodeEditRuleCompletionDelegate()

    func update(
        context: BifrostRuleEditorContext,
        onSave: @escaping () -> Void,
        onTextChanged: ((String) -> Void)?
    ) {
        highlighter.editorContext = context
        completionDelegate.editorContext = context
        coordinator.onSave = onSave
        coordinator.onTextChanged = onTextChanged
    }
}

@MainActor
private final class CodeEditRuleTextCoordinator: @preconcurrency TextViewCoordinator {
    var onSave: (() -> Void)?
    var onTextChanged: ((String) -> Void)?

    private weak var controller: TextViewController?
    private var keyMonitor: Any?

    func prepareCoordinator(controller: TextViewController) {
        self.controller = controller
        installSaveMonitor()
    }

    func textViewDidChangeText(controller: TextViewController) {
        onTextChanged?(controller.text)
    }

    func destroy() {
        if let keyMonitor {
            NSEvent.removeMonitor(keyMonitor)
        }
        keyMonitor = nil
    }

    private func installSaveMonitor() {
        guard keyMonitor == nil else {
            return
        }
        keyMonitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { [weak self] event in
            guard let self,
                  let controller = self.controller,
                  event.modifierFlags.intersection(.deviceIndependentFlagsMask).contains(.command),
                  event.charactersIgnoringModifiers?.lowercased() == "s",
                  self.eventTargetsEditor(event, controller: controller) else {
                return event
            }
            self.onSave?()
            return nil
        }
    }

    private func eventTargetsEditor(_ event: NSEvent, controller: TextViewController) -> Bool {
        guard event.window === controller.view.window else {
            return false
        }
        guard let responder = event.window?.firstResponder as? NSView else {
            return false
        }
        return responder === controller.view || responder.isDescendant(of: controller.view)
    }
}

@MainActor
private final class CodeEditRuleHighlightProvider: HighlightProviding {
    var editorContext = BifrostRuleEditorContext.empty

    private let languageService = BifrostRuleLanguageService()

    func setUp(textView: TextView, codeLanguage: CodeLanguage) {}

    func willApplyEdit(textView: TextView, range: NSRange) {}

    func applyEdit(
        textView: TextView,
        range: NSRange,
        delta: Int,
        completion: @escaping @MainActor (Result<IndexSet, Error>) -> Void
    ) {
        completion(.success(IndexSet(integersIn: 0..<max(textView.textStorage.length, 0))))
    }

    func queryHighlightsFor(
        textView: TextView,
        range: NSRange,
        completion: @escaping @MainActor (Result<[HighlightRange], Error>) -> Void
    ) {
        let tokens = languageService.tokenize(textView.textStorage.string, context: editorContext)
        let highlights = tokens.compactMap { token -> HighlightRange? in
            guard let clipped = token.range.bifrostIntersection(range) else {
                return nil
            }
            return HighlightRange(range: clipped, capture: captureName(for: token.kind))
        }
        completion(.success(highlights))
    }

    private func captureName(for kind: BifrostRuleTokenKind) -> CaptureName? {
        switch kind {
        case .comment:
            return .comment
        case .keyword, .urlScheme, .scheme, .codeFence:
            return .keyword
        case .string, .attributeValue, .regexp:
            return .string
        case .variable, .localVariable, .globalValue, .ruleReference:
            return .variable
        case .requestScript, .responseScript, .parserScript:
            return .function
        case .attributeName:
            return .typeAlternate
        case .delimiter, .bracket, .plain, .invalid:
            return nil
        }
    }
}

@MainActor
private final class CodeEditRuleCompletionDelegate: CodeSuggestionDelegate {
    var editorContext = BifrostRuleEditorContext.empty

    private let languageService = BifrostRuleLanguageService()
    private var lastItems: [CodeEditRuleSuggestionEntry] = []

    func completionTriggerCharacters() -> Set<String> {
        ["@", "{", "/", ":"]
    }

    func completionSuggestionsRequested(
        textView: TextViewController,
        cursorPosition: CursorPosition
    ) async -> (windowPosition: CursorPosition, items: [CodeSuggestionEntry])? {
        let items = completions(text: textView.text, cursor: cursorPosition)
        guard !items.isEmpty else {
            return nil
        }
        lastItems = items
        return (cursorPosition, items)
    }

    func completionOnCursorMove(
        textView: TextViewController,
        cursorPosition: CursorPosition
    ) -> [CodeSuggestionEntry]? {
        let items = completions(text: textView.text, cursor: cursorPosition)
        lastItems = items
        return items.isEmpty ? nil : items
    }

    func completionWindowApplyCompletion(
        item: CodeSuggestionEntry,
        textView: TextViewController,
        cursorPosition: CursorPosition?
    ) {
        guard let item = item as? CodeEditRuleSuggestionEntry,
              NSMaxRange(item.replacementRange) <= textView.textView.textStorage.length else {
            return
        }
        textView.textView.replaceCharacters(in: item.replacementRange, with: item.insertText)
        let cursor = NSRange(location: item.replacementRange.location + (item.insertText as NSString).length, length: 0)
        textView.setCursorPositions([CursorPosition(range: cursor)])
    }

    private func completions(text: String, cursor: CursorPosition) -> [CodeEditRuleSuggestionEntry] {
        languageService.completions(
            in: text,
            cursor: BifrostTextPosition(utf16Offset: cursor.range.location),
            context: editorContext
        )
        .prefix(16)
        .map(CodeEditRuleSuggestionEntry.init)
    }
}

private struct CodeEditRuleSuggestionEntry: CodeSuggestionEntry {
    let label: String
    let detail: String?
    let documentation: String?
    let pathComponents: [String]?
    let targetPosition: CursorPosition?
    let sourcePreview: String?
    let image: Image
    let imageColor: Color
    let deprecated: Bool
    let insertText: String
    let replacementRange: NSRange

    init(item: BifrostCompletionItem) {
        label = item.label
        detail = item.detail
        documentation = nil
        pathComponents = nil
        targetPosition = nil
        sourcePreview = nil
        image = CodeEditRuleSuggestionEntry.image(for: item.kind)
        imageColor = CodeEditRuleSuggestionEntry.color(for: item.kind)
        deprecated = false
        insertText = item.insertText
        replacementRange = item.replacementRange
    }

    private static func image(for kind: BifrostCompletionKind) -> Image {
        switch kind {
        case .rule:
            return Image(systemName: "arrow.triangle.branch")
        case .value, .localVariable:
            return Image(systemName: "curlybraces")
        case .requestScript, .responseScript, .parserScript:
            return Image(systemName: "function")
        }
    }

    private static func color(for kind: BifrostCompletionKind) -> Color {
        switch kind {
        case .rule:
            return .blue
        case .value, .localVariable:
            return .purple
        case .requestScript, .responseScript, .parserScript:
            return .orange
        }
    }
}

private enum CodeEditRuleTheme {
    static func make(appearance: NSAppearance) -> EditorTheme {
        let theme = BifrostRuleEditorTheme(appearance: appearance)
        return EditorTheme(
            text: .init(color: theme.text.codeEditRuleRGB),
            insertionPoint: theme.text.codeEditRuleRGB,
            invisibles: .init(color: theme.comment.codeEditRuleRGB),
            background: theme.background.codeEditRuleRGB,
            lineHighlight: theme.rulerBackground.withAlphaComponent(0.68).codeEditRuleRGB,
            selection: NSColor.selectedTextBackgroundColor.codeEditRuleRGB,
            keywords: .init(color: theme.keyword.codeEditRuleRGB),
            commands: .init(color: theme.script.codeEditRuleRGB),
            types: .init(color: theme.attribute.codeEditRuleRGB),
            attributes: .init(color: theme.attribute.codeEditRuleRGB),
            variables: .init(color: theme.variable.codeEditRuleRGB),
            values: .init(color: theme.reference.codeEditRuleRGB),
            numbers: .init(color: theme.regexp.codeEditRuleRGB),
            strings: .init(color: theme.string.codeEditRuleRGB),
            characters: .init(color: theme.regexp.codeEditRuleRGB),
            comments: .init(color: theme.comment.codeEditRuleRGB, italic: true)
        )
    }
}

private extension NSColor {
    var codeEditRuleRGB: NSColor {
        if let converted = usingColorSpace(.deviceRGB) {
            return converted
        }
        return NSColor(deviceRed: 0.12, green: 0.14, blue: 0.18, alpha: 1)
    }
}

private extension NSRange {
    func bifrostIntersection(_ other: NSRange) -> NSRange? {
        let lower = Swift.max(location, other.location)
        let upper = Swift.min(NSMaxRange(self), NSMaxRange(other))
        guard upper > lower else {
            return nil
        }
        return NSRange(location: lower, length: upper - lower)
    }
}
#else
import SwiftUI

enum RuleEditorBackendAvailability {
    static let codeEditSourceEditor = false
}

struct CodeEditRuleEditorView: View {
    @Binding var text: String
    var context: BifrostRuleEditorContext
    var isReadOnly = false
    var onSave: () -> Void
    var onTextChanged: ((String) -> Void)?

    var body: some View {
        BifrostRuleEditorView(
            text: $text,
            context: context,
            isReadOnly: isReadOnly,
            onNavigate: { _ in },
            onSave: onSave,
            onTextChanged: onTextChanged
        )
    }
}
#endif
