import AppKit
import BifrostNativeCore
import SwiftUI

struct BifrostRuleEditorView: NSViewRepresentable {
    @Binding var text: String
    var context: BifrostRuleEditorContext
    var isReadOnly = false
    var onNavigate: (BifrostNavigationTarget) -> Void
    var onSave: () -> Void
    var onTextChanged: ((String) -> Void)?

    func makeCoordinator() -> Coordinator {
        Coordinator(self)
    }

    func makeNSView(context: Context) -> BifrostRuleEditorContainerView {
        let view = BifrostRuleEditorContainerView()
        view.textView.delegate = context.coordinator
        view.textView.onSave = onSave
        view.textView.onNavigate = onNavigate
        view.textView.languageService = context.coordinator.languageService
        view.textView.completionController = context.coordinator.completionController
        context.coordinator.container = view
        view.update(text: text, editorContext: self.context, isReadOnly: isReadOnly)
        return view
    }

    func updateNSView(_ nsView: BifrostRuleEditorContainerView, context: Context) {
        context.coordinator.parent = self
        nsView.textView.onSave = onSave
        nsView.textView.onNavigate = onNavigate
        nsView.textView.languageService = context.coordinator.languageService
        nsView.textView.completionController = context.coordinator.completionController
        nsView.update(text: text, editorContext: self.context, isReadOnly: isReadOnly)
    }

    final class Coordinator: NSObject, NSTextViewDelegate {
        var parent: BifrostRuleEditorView
        weak var container: BifrostRuleEditorContainerView?
        let languageService = BifrostRuleLanguageService()
        let completionController = BifrostRuleCompletionController()

        init(_ parent: BifrostRuleEditorView) {
            self.parent = parent
        }

        func textDidChange(_ notification: Notification) {
            guard let textView = notification.object as? BifrostRuleTextView else {
                return
            }
            parent.text = textView.string
            parent.onTextChanged?(textView.string)
            container?.highlight()
            refreshCompletion(for: textView)
        }

        func textViewDidChangeSelection(_ notification: Notification) {
            guard let textView = notification.object as? BifrostRuleTextView else {
                return
            }
            textView.lineNumberRuler?.needsDisplay = true
        }

        private func refreshCompletion(for textView: BifrostRuleTextView) {
            let items = languageService.completions(
                in: textView.string,
                cursor: BifrostTextPosition(utf16Offset: textView.selectedRange().location),
                context: textView.editorContext
            )
            completionController.update(textView: textView, items: items)
        }
    }
}

final class BifrostRuleEditorContainerView: NSView {
    let scrollView = NSScrollView()
    let textView = BifrostRuleTextView()
    private let highlighter = BifrostRuleHighlighter()
    private var latestText = ""
    private var latestContext = BifrostRuleEditorContext.empty

    override init(frame frameRect: NSRect) {
        super.init(frame: frameRect)
        setup()
    }

    required init?(coder: NSCoder) {
        super.init(coder: coder)
        setup()
    }

    func update(text: String, editorContext: BifrostRuleEditorContext, isReadOnly: Bool) {
        latestContext = editorContext
        textView.editorContext = editorContext
        textView.isEditable = !isReadOnly
        if textView.string != text {
            let selectedRanges = textView.selectedRanges
            textView.string = text
            textView.selectedRanges = selectedRanges.compactMap { value in
                guard let range = value.rangeValue.clamped(to: (text as NSString).length) else {
                    return nil
                }
                return NSValue(range: range)
            }
            textView.undoManager?.removeAllActions()
        }
        latestText = textView.string
        highlight()
        textView.lineNumberRuler?.needsDisplay = true
    }

    func highlight() {
        guard latestText != textView.string || textView.textStorage?.length ?? 0 > 0 else {
            return
        }
        latestText = textView.string
        highlighter.applyHighlighting(to: textView, context: latestContext)
    }

    private func setup() {
        wantsLayer = false
        applyTheme()

        textView.minSize = NSSize(width: 0, height: 0)
        textView.maxSize = NSSize(width: CGFloat.greatestFiniteMagnitude, height: CGFloat.greatestFiniteMagnitude)
        textView.isVerticallyResizable = true
        textView.isHorizontallyResizable = false
        textView.autoresizingMask = [.width, .height]
        textView.textContainerInset = NSSize(width: 8, height: 8)
        textView.textContainer?.containerSize = NSSize(width: 0, height: CGFloat.greatestFiniteMagnitude)
        textView.textContainer?.widthTracksTextView = true
        textView.drawsBackground = true
        textView.font = .monospacedSystemFont(ofSize: 13, weight: .regular)
        textView.allowsUndo = true
        textView.isRichText = false
        textView.importsGraphics = false
        textView.isAutomaticQuoteSubstitutionEnabled = false
        textView.isAutomaticDashSubstitutionEnabled = false
        textView.isAutomaticTextReplacementEnabled = false
        textView.isAutomaticSpellingCorrectionEnabled = false
        textView.isAutomaticLinkDetectionEnabled = false
        textView.isContinuousSpellCheckingEnabled = false
        textView.isGrammarCheckingEnabled = false

        let ruler = BifrostLineNumberRulerView(textView: textView)
        textView.lineNumberRuler = ruler

        scrollView.contentView = BifrostRuleClipView()
        scrollView.documentView = textView
        scrollView.hasVerticalScroller = true
        scrollView.hasHorizontalScroller = false
        scrollView.autohidesScrollers = false
        scrollView.drawsBackground = true
        scrollView.contentView.drawsBackground = true
        scrollView.borderType = .noBorder
        scrollView.hasVerticalRuler = true
        scrollView.rulersVisible = true
        scrollView.verticalRulerView = ruler
        addSubview(scrollView)
        applyTheme()
    }

    override func layout() {
        super.layout()
        scrollView.frame = bounds
        let clipSize = scrollView.contentView.bounds.size
        let targetWidth = max(clipSize.width, 1)
        textView.frame = NSRect(
            x: 0,
            y: 0,
            width: targetWidth,
            height: max(clipSize.height, 1)
        )

        var usedHeight: CGFloat = 0
        if let textContainer = textView.textContainer {
            textContainer.widthTracksTextView = true
            textContainer.containerSize = NSSize(width: targetWidth, height: CGFloat.greatestFiniteMagnitude)
            textView.layoutManager?.invalidateLayout(
                forCharacterRange: NSRange(location: 0, length: (textView.string as NSString).length),
                actualCharacterRange: nil
            )
            textView.layoutManager?.ensureLayout(for: textContainer)
            usedHeight = textView.layoutManager?.usedRect(for: textContainer).height ?? 0
        }

        textView.frame.size.height = max(clipSize.height, usedHeight + textView.textContainerInset.height * 2)
        scrollView.contentView.scroll(to: .zero)
        textView.needsDisplay = true
        scrollView.contentView.needsDisplay = true
    }

    override func viewDidChangeEffectiveAppearance() {
        super.viewDidChangeEffectiveAppearance()
        applyTheme()
        highlight()
        textView.lineNumberRuler?.needsDisplay = true
    }

    private func applyTheme() {
        let theme = BifrostRuleEditorTheme(appearance: effectiveAppearance)
        textView.backgroundColor = theme.background
        textView.textColor = theme.text
        textView.insertionPointColor = theme.text
        scrollView.backgroundColor = theme.background
        scrollView.contentView.backgroundColor = theme.background
        scrollView.verticalRulerView?.needsDisplay = true
    }
}

final class BifrostRuleClipView: NSClipView {
    override var isFlipped: Bool { true }
}

final class BifrostRuleTextView: NSTextView {
    var onSave: (() -> Void)?
    var onNavigate: ((BifrostNavigationTarget) -> Void)?
    var languageService = BifrostRuleLanguageService()
    var editorContext = BifrostRuleEditorContext.empty
    weak var lineNumberRuler: BifrostLineNumberRulerView?
    weak var completionController: BifrostRuleCompletionController?

    override func keyDown(with event: NSEvent) {
        if event.modifierFlags.intersection(.deviceIndependentFlagsMask).contains(.command),
           event.charactersIgnoringModifiers?.lowercased() == "s" {
            onSave?()
            return
        }
        if completionController?.handleKeyDown(event, textView: self) == true {
            return
        }
        if isF12(event), navigateFromSelection() {
            return
        }
        super.keyDown(with: event)
    }

    override func mouseDown(with event: NSEvent) {
        if event.modifierFlags.intersection(.deviceIndependentFlagsMask).contains(.command),
           navigate(at: characterIndex(for: event)) {
            return
        }
        completionController?.close()
        super.mouseDown(with: event)
    }

    override func resetCursorRects() {
        super.resetCursorRects()
        addCursorRect(visibleRect, cursor: .iBeam)
    }

    private func navigateFromSelection() -> Bool {
        navigate(at: selectedRange().location)
    }

    private func navigate(at index: Int?) -> Bool {
        guard let index,
              let reference = languageService.reference(
                in: string,
                cursor: BifrostTextPosition(utf16Offset: index),
                context: editorContext
              ),
              let target = languageService.navigationTarget(for: reference, context: editorContext) else {
            return false
        }
        if case .editorLine(let line) = target {
            scrollToLine(line)
            return true
        }
        onNavigate?(target)
        return true
    }

    private func scrollToLine(_ line: Int) {
        let nsString = string as NSString
        var currentLine = 1
        var location = 0
        while currentLine < line && location < nsString.length {
            let range = nsString.lineRange(for: NSRange(location: location, length: 0))
            location = NSMaxRange(range)
            currentLine += 1
        }
        setSelectedRange(NSRange(location: min(location, nsString.length), length: 0))
        scrollRangeToVisible(selectedRange())
    }

    private func characterIndex(for event: NSEvent) -> Int? {
        let point = convert(event.locationInWindow, from: nil)
        guard let layoutManager, let textContainer else {
            return nil
        }
        let origin = textContainerOrigin
        let containerPoint = NSPoint(x: point.x - origin.x, y: point.y - origin.y)
        let glyphIndex = layoutManager.glyphIndex(for: containerPoint, in: textContainer)
        return layoutManager.characterIndexForGlyph(at: glyphIndex)
    }

    private func isF12(_ event: NSEvent) -> Bool {
        guard let scalar = event.charactersIgnoringModifiers?.unicodeScalars.first else {
            return false
        }
        return scalar.value == NSF12FunctionKey
    }
}

final class BifrostRuleHighlighter {
    private let service = BifrostRuleLanguageService()

    func applyHighlighting(to textView: NSTextView, context: BifrostRuleEditorContext) {
        guard let storage = textView.textStorage else {
            return
        }
        let theme = BifrostRuleEditorTheme(appearance: textView.effectiveAppearance)
        let fullRange = NSRange(location: 0, length: storage.length)
        let selection = textView.selectedRanges
        storage.beginEditing()
        storage.setAttributes(theme.baseAttributes, range: fullRange)
        for token in service.tokenize(textView.string, context: context) where NSMaxRange(token.range) <= storage.length {
            storage.addAttributes(theme.attributes(for: token.kind), range: token.range)
        }
        storage.endEditing()
        textView.selectedRanges = selection
    }
}

struct BifrostRuleEditorTheme {
    let background: NSColor
    let rulerBackground: NSColor
    let completionBackground: NSColor
    let text: NSColor
    let comment: NSColor
    let keyword: NSColor
    let string: NSColor
    let variable: NSColor
    let reference: NSColor
    let script: NSColor
    let attribute: NSColor
    let regexp: NSColor

    init(appearance: NSAppearance = NSApp.effectiveAppearance) {
        let isDark = appearance.bestMatch(from: [.darkAqua, .aqua, .vibrantDark, .vibrantLight]) == .darkAqua
            || appearance.bestMatch(from: [.darkAqua, .aqua, .vibrantDark, .vibrantLight]) == .vibrantDark
        background = isDark
            ? NSColor(calibratedRed: 0.090, green: 0.102, blue: 0.122, alpha: 1)
            : NSColor(calibratedWhite: 1.0, alpha: 1)
        rulerBackground = isDark
            ? NSColor(calibratedRed: 0.072, green: 0.082, blue: 0.100, alpha: 1)
            : NSColor(calibratedWhite: 0.98, alpha: 1)
        completionBackground = isDark
            ? NSColor(calibratedRed: 0.125, green: 0.145, blue: 0.175, alpha: 1)
            : NSColor(calibratedWhite: 0.965, alpha: 1)
        text = isDark
            ? NSColor(calibratedRed: 0.890, green: 0.920, blue: 0.960, alpha: 1)
            : NSColor(calibratedRed: 0.120, green: 0.145, blue: 0.180, alpha: 1)
        comment = isDark
            ? NSColor(calibratedRed: 0.560, green: 0.620, blue: 0.700, alpha: 1)
            : NSColor(calibratedRed: 0.430, green: 0.475, blue: 0.535, alpha: 1)
        keyword = isDark
            ? NSColor(calibratedRed: 0.420, green: 0.670, blue: 1.000, alpha: 1)
            : NSColor(calibratedRed: 0.030, green: 0.330, blue: 0.780, alpha: 1)
        string = isDark
            ? NSColor(calibratedRed: 0.270, green: 0.780, blue: 0.780, alpha: 1)
            : NSColor(calibratedRed: 0.000, green: 0.470, blue: 0.490, alpha: 1)
        variable = isDark
            ? NSColor(calibratedRed: 0.760, green: 0.560, blue: 1.000, alpha: 1)
            : NSColor(calibratedRed: 0.480, green: 0.220, blue: 0.760, alpha: 1)
        reference = isDark
            ? NSColor(calibratedRed: 0.610, green: 0.650, blue: 1.000, alpha: 1)
            : NSColor(calibratedRed: 0.250, green: 0.310, blue: 0.780, alpha: 1)
        script = isDark
            ? NSColor(calibratedRed: 1.000, green: 0.640, blue: 0.330, alpha: 1)
            : NSColor(calibratedRed: 0.760, green: 0.340, blue: 0.020, alpha: 1)
        attribute = isDark
            ? NSColor(calibratedRed: 0.840, green: 0.650, blue: 0.430, alpha: 1)
            : NSColor(calibratedRed: 0.540, green: 0.330, blue: 0.130, alpha: 1)
        regexp = isDark
            ? NSColor(calibratedRed: 1.000, green: 0.540, blue: 0.780, alpha: 1)
            : NSColor(calibratedRed: 0.780, green: 0.180, blue: 0.470, alpha: 1)
    }

    var baseAttributes: [NSAttributedString.Key: Any] {
        [
            .foregroundColor: text,
            .font: NSFont.monospacedSystemFont(ofSize: 13, weight: .regular),
        ]
    }

    func attributes(for kind: BifrostRuleTokenKind) -> [NSAttributedString.Key: Any] {
        var color = text
        switch kind {
        case .comment:
            color = comment
        case .keyword, .urlScheme, .scheme:
            color = keyword
        case .string, .attributeValue:
            color = string
        case .variable, .localVariable, .globalValue:
            color = variable
        case .ruleReference:
            color = reference
        case .requestScript, .responseScript, .parserScript:
            color = script
        case .attributeName:
            color = attribute
        case .regexp:
            color = regexp
        case .delimiter, .bracket, .codeFence, .plain, .invalid:
            color = text
        }
        return [
            .foregroundColor: color,
            .font: NSFont.monospacedSystemFont(ofSize: 13, weight: .regular),
        ]
    }
}

final class BifrostRuleCompletionController: NSObject, NSTableViewDataSource, NSTableViewDelegate {
    private var panel: NSPanel?
    private var tableView: NSTableView?
    private var items: [BifrostCompletionItem] = []
    private var selectedIndex = 0

    func update(textView: NSTextView, items: [BifrostCompletionItem]) {
        self.items = Array(items.prefix(12))
        selectedIndex = 0
        if self.items.isEmpty {
            close()
            return
        }
        let panel = ensurePanel()
        tableView?.reloadData()
        tableView?.selectRowIndexes(IndexSet(integer: 0), byExtendingSelection: false)
        position(panel: panel, near: textView)
        if !panel.isVisible {
            panel.orderFront(nil)
        }
    }

    func handleKeyDown(_ event: NSEvent, textView: NSTextView) -> Bool {
        guard panel?.isVisible == true else {
            return false
        }
        switch event.keyCode {
        case 125:
            moveSelection(1)
            return true
        case 126:
            moveSelection(-1)
            return true
        case 36, 48:
            acceptSelection(in: textView)
            return true
        case 53:
            close()
            return true
        default:
            return false
        }
    }

    func close() {
        panel?.orderOut(nil)
    }

    func numberOfRows(in tableView: NSTableView) -> Int {
        items.count
    }

    func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView? {
        let item = items[row]
        let cell = CompletionRowView()
        cell.configure(item)
        return cell
    }

    func tableViewSelectionDidChange(_ notification: Notification) {
        selectedIndex = max(0, tableView?.selectedRow ?? 0)
    }

    private func moveSelection(_ delta: Int) {
        guard !items.isEmpty else {
            return
        }
        selectedIndex = min(max(selectedIndex + delta, 0), items.count - 1)
        tableView?.selectRowIndexes(IndexSet(integer: selectedIndex), byExtendingSelection: false)
        tableView?.scrollRowToVisible(selectedIndex)
    }

    private func acceptSelection(in textView: NSTextView) {
        guard items.indices.contains(selectedIndex) else {
            return
        }
        let item = items[selectedIndex]
        if textView.shouldChangeText(in: item.replacementRange, replacementString: item.insertText) {
            textView.textStorage?.replaceCharacters(in: item.replacementRange, with: item.insertText)
            textView.didChangeText()
            let location = item.replacementRange.location + (item.insertText as NSString).length
            textView.setSelectedRange(NSRange(location: location, length: 0))
        }
        close()
    }

    private func ensurePanel() -> NSPanel {
        if let panel {
            return panel
        }
        let panel = NSPanel(
            contentRect: NSRect(x: 0, y: 0, width: 280, height: 220),
            styleMask: [.borderless, .nonactivatingPanel],
            backing: .buffered,
            defer: false
        )
        panel.level = .floating
        panel.hasShadow = true
        panel.backgroundColor = BifrostRuleEditorTheme().completionBackground

        let scrollView = NSScrollView(frame: panel.contentView?.bounds ?? .zero)
        scrollView.autoresizingMask = [.width, .height]
        scrollView.hasVerticalScroller = true
        scrollView.drawsBackground = true
        scrollView.backgroundColor = BifrostRuleEditorTheme().completionBackground
        let table = NSTableView()
        table.headerView = nil
        table.rowHeight = 34
        table.backgroundColor = BifrostRuleEditorTheme().completionBackground
        table.delegate = self
        table.dataSource = self
        let column = NSTableColumn(identifier: NSUserInterfaceItemIdentifier("completion"))
        column.width = 280
        table.addTableColumn(column)
        scrollView.documentView = table
        panel.contentView = scrollView
        self.panel = panel
        tableView = table
        return panel
    }

    private func position(panel: NSPanel, near textView: NSTextView) {
        let range = textView.selectedRange()
        var rect = textView.firstRect(forCharacterRange: range, actualRange: nil)
        if rect == .zero, let window = textView.window {
            rect = window.frame
        }
        panel.setFrameTopLeftPoint(NSPoint(x: rect.minX, y: rect.minY - 6))
    }
}

final class CompletionRowView: NSTableCellView {
    private let title = NSTextField(labelWithString: "")
    private let detail = NSTextField(labelWithString: "")

    override init(frame frameRect: NSRect) {
        super.init(frame: frameRect)
        title.font = .systemFont(ofSize: 12, weight: .semibold)
        detail.font = .systemFont(ofSize: 10)
        detail.textColor = .secondaryLabelColor
        addSubview(title)
        addSubview(detail)
    }

    required init?(coder: NSCoder) {
        super.init(coder: coder)
    }

    func configure(_ item: BifrostCompletionItem) {
        title.stringValue = item.label
        detail.stringValue = item.detail
        needsLayout = true
    }

    override func layout() {
        super.layout()
        title.frame = NSRect(x: 9, y: bounds.height - 18, width: bounds.width - 18, height: 14)
        detail.frame = NSRect(x: 9, y: 4, width: bounds.width - 18, height: 12)
    }
}

final class BifrostLineNumberRulerView: NSRulerView {
    weak var textView: NSTextView?

    init(textView: NSTextView) {
        self.textView = textView
        super.init(scrollView: textView.enclosingScrollView, orientation: .verticalRuler)
        clientView = textView
        ruleThickness = 44
    }

    required init(coder: NSCoder) {
        super.init(coder: coder)
    }

    override func drawHashMarksAndLabels(in rect: NSRect) {
        guard let textView,
              let layoutManager = textView.layoutManager,
              let textContainer = textView.textContainer else {
            return
        }
        BifrostRuleEditorTheme(appearance: effectiveAppearance).rulerBackground.setFill()
        rect.fill()

        let visible = textView.visibleRect
        let glyphRange = layoutManager.glyphRange(forBoundingRect: visible, in: textContainer)
        var lineNumber = lineNumberForCharacter(at: layoutManager.characterIndexForGlyph(at: glyphRange.location), in: textView.string)
        var glyphIndex = glyphRange.location
        let attrs: [NSAttributedString.Key: Any] = [
            .font: NSFont.monospacedDigitSystemFont(ofSize: 10, weight: .regular),
            .foregroundColor: NSColor.secondaryLabelColor,
        ]
        while glyphIndex < NSMaxRange(glyphRange) {
            var lineRange = NSRange(location: 0, length: 0)
            let lineRect = layoutManager.lineFragmentRect(forGlyphAt: glyphIndex, effectiveRange: &lineRange)
            let y = lineRect.minY + textView.textContainerOrigin.y
            NSString(string: "\(lineNumber)").draw(
                in: NSRect(x: 4, y: y + 1, width: ruleThickness - 10, height: 14),
                withAttributes: attrs
            )
            glyphIndex = NSMaxRange(lineRange)
            lineNumber += 1
        }
    }

    private func lineNumberForCharacter(at index: Int, in string: String) -> Int {
        let nsString = string as NSString
        guard index > 0, index <= nsString.length else {
            return 1
        }
        return nsString.substring(to: index).filter { $0 == "\n" }.count + 1
    }
}

private extension NSRange {
    func clamped(to length: Int) -> NSRange? {
        guard location <= length else {
            return nil
        }
        return NSRange(location: location, length: min(self.length, length - location))
    }
}
