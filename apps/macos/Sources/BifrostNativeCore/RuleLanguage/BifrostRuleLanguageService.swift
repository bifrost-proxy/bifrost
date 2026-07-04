import Foundation

public struct BifrostTextPosition: Equatable, Sendable {
    public var utf16Offset: Int

    public init(utf16Offset: Int) {
        self.utf16Offset = utf16Offset
    }
}

public enum BifrostRuleTokenKind: String, Equatable, Sendable {
    case comment
    case keyword
    case string
    case scheme
    case urlScheme
    case regexp
    case delimiter
    case bracket
    case variable
    case localVariable
    case globalValue
    case ruleReference
    case requestScript
    case responseScript
    case parserScript
    case attributeName
    case attributeValue
    case codeFence
    case plain
    case invalid
}

public struct BifrostRuleToken: Equatable, Sendable {
    public var kind: BifrostRuleTokenKind
    public var range: NSRange

    public init(kind: BifrostRuleTokenKind, range: NSRange) {
        self.kind = kind
        self.range = range
    }
}

public struct BifrostLocalVariable: Equatable, Sendable {
    public var name: String
    public var line: Int

    public init(name: String, line: Int) {
        self.name = name
        self.line = line
    }
}

public struct BifrostRuleEditorContext: Equatable, Sendable {
    public static let empty = BifrostRuleEditorContext()

    public var currentRuleName: String?
    public var currentGroupName: String?
    public var ruleNames: [String]
    public var values: [String]
    public var requestScripts: [String]
    public var responseScripts: [String]
    public var parserScripts: [String]
    public var localVariables: [BifrostLocalVariable]

    public init(
        currentRuleName: String? = nil,
        currentGroupName: String? = nil,
        ruleNames: [String] = [],
        values: [String] = [],
        requestScripts: [String] = [],
        responseScripts: [String] = [],
        parserScripts: [String] = [],
        localVariables: [BifrostLocalVariable] = []
    ) {
        self.currentRuleName = currentRuleName
        self.currentGroupName = currentGroupName
        self.ruleNames = ruleNames
        self.values = values
        self.requestScripts = requestScripts
        self.responseScripts = responseScripts
        self.parserScripts = parserScripts
        self.localVariables = localVariables
    }
}

public enum BifrostCompletionKind: Equatable, Sendable {
    case rule
    case value
    case localVariable
    case requestScript
    case responseScript
    case parserScript
}

public struct BifrostCompletionItem: Identifiable, Equatable, Sendable {
    public var id: String
    public var label: String
    public var insertText: String
    public var detail: String
    public var kind: BifrostCompletionKind
    public var replacementRange: NSRange
    public var sortText: String

    public init(
        label: String,
        insertText: String,
        detail: String,
        kind: BifrostCompletionKind,
        replacementRange: NSRange,
        sortText: String
    ) {
        self.id = "\(kind)-\(label)-\(replacementRange.location)-\(replacementRange.length)"
        self.label = label
        self.insertText = insertText
        self.detail = detail
        self.kind = kind
        self.replacementRange = replacementRange
        self.sortText = sortText
    }
}

public enum BifrostReferenceType: Equatable, Sendable {
    case value
    case localVariable
    case requestScript
    case responseScript
    case parserScript
    case rule
}

public struct BifrostReferenceMatch: Equatable, Sendable {
    public var name: String
    public var type: BifrostReferenceType
    public var range: NSRange

    public init(name: String, type: BifrostReferenceType, range: NSRange) {
        self.name = name
        self.type = type
        self.range = range
    }
}

public enum BifrostRuleDiagnosticSeverity: String, Equatable, Sendable {
    case error
    case warning
    case info
}

public struct BifrostRuleDiagnostic: Identifiable, Equatable, Sendable {
    public var id: String
    public var severity: BifrostRuleDiagnosticSeverity
    public var message: String
    public var line: Int
    public var range: NSRange

    public init(
        severity: BifrostRuleDiagnosticSeverity,
        message: String,
        line: Int,
        range: NSRange
    ) {
        self.id = "\(severity.rawValue)-\(line)-\(range.location)-\(range.length)-\(message)"
        self.severity = severity
        self.message = message
        self.line = line
        self.range = range
    }
}

public enum BifrostNavigationTarget: Equatable, Sendable {
    case editorLine(Int)
    case value(name: String)
    case script(type: ScriptType, name: String)
    case rule(group: String?, name: String)
}

public struct BifrostRuleLanguageService: Sendable {
    public init() {}

    public func tokenize(_ text: String, context: BifrostRuleEditorContext = .empty) -> [BifrostRuleToken] {
        let nsText = text as NSString
        var tokens: [BifrostRuleToken] = []
        var inCodeFence = false
        var inLineBlock = false

        nsText.enumerateSubstrings(
            in: NSRange(location: 0, length: nsText.length),
            options: [.byLines, .substringNotRequired]
        ) { _, lineRange, _, _ in
            let line = nsText.substring(with: lineRange)
            let trimmed = line.trimmingCharacters(in: .whitespaces)

            if inCodeFence {
                tokens.append(BifrostRuleToken(kind: .string, range: lineRange))
                if trimmed.hasPrefix("```") {
                    tokens.append(BifrostRuleToken(kind: .codeFence, range: leadingRange(lineRange, length: 3)))
                    inCodeFence = false
                }
                return
            }
            if inLineBlock {
                tokens.append(BifrostRuleToken(kind: .string, range: lineRange))
                if trimmed == "`" {
                    tokens.append(BifrostRuleToken(kind: .codeFence, range: leadingRange(lineRange, length: 1)))
                    inLineBlock = false
                }
                return
            }

            if trimmed.hasPrefix("#") {
                tokens.append(BifrostRuleToken(kind: .comment, range: lineRange))
                return
            }
            if trimmed.hasPrefix("```") {
                tokens.append(contentsOf: codeFenceOpenTokens(line: line, lineRange: lineRange))
                inCodeFence = true
                return
            }
            if trimmed == "line`" {
                tokens.append(BifrostRuleToken(kind: .keyword, range: lineRange))
                inLineBlock = true
                return
            }

            tokens.append(contentsOf: regexTokens(pattern: "#.*$", in: line, lineRange: lineRange, kind: .comment))
            tokens.append(contentsOf: regexTokens(pattern: "@[A-Za-z0-9_.\\-]+", in: line, lineRange: lineRange, kind: .ruleReference))
            tokens.append(contentsOf: regexTokens(pattern: "\\$?\\{[A-Za-z0-9_\\-]+\\}", in: line, lineRange: lineRange, kind: .variable))
            tokens.append(contentsOf: regexTokens(pattern: "\\breqScript://[^#\\s]*", in: line, lineRange: lineRange, kind: .requestScript))
            tokens.append(contentsOf: regexTokens(pattern: "\\bresScript://[^#\\s]*", in: line, lineRange: lineRange, kind: .responseScript))
            tokens.append(contentsOf: regexTokens(pattern: "\\bbp://[^#\\s]*", in: line, lineRange: lineRange, kind: .parserScript))
            tokens.append(contentsOf: regexTokens(pattern: "\\b[A-Za-z][A-Za-z0-9_.\\-]*://", in: line, lineRange: lineRange, kind: .urlScheme))
            tokens.append(contentsOf: regexTokens(pattern: "/([^\\\\/]|\\\\.)+/[A-Za-z]*", in: line, lineRange: lineRange, kind: .regexp))
            tokens.append(contentsOf: keyValueTokens(in: line, lineRange: lineRange))
            tokens.append(contentsOf: regexTokens(pattern: "[{}\\[\\]()]|\\$\\{", in: line, lineRange: lineRange, kind: .bracket))
        }

        return merge(tokens: tokens)
    }

    public func completions(
        in text: String,
        cursor: BifrostTextPosition,
        context: BifrostRuleEditorContext
    ) -> [BifrostCompletionItem] {
        let nsText = text as NSString
        let safeOffset = min(max(cursor.utf16Offset, 0), nsText.length)
        let lineRange = nsText.lineRange(for: NSRange(location: safeOffset, length: 0))
        let lineStart = lineRange.location
        let before = nsText.substring(with: NSRange(location: lineStart, length: safeOffset - lineStart))

        if let match = suffixMatch(pattern: #"@([A-Za-z0-9_.\-]*)$"#, text: before) {
            let typed = match.captures[0]
            let start = safeOffset - typed.utf16.count
            return context.ruleNames
                .filter { fuzzyMatch(query: typed, candidate: $0) }
                .map { name in
                    BifrostCompletionItem(
                        label: "@\(name)",
                        insertText: name,
                        detail: "Rule Reference: \(name)",
                        kind: .rule,
                        replacementRange: NSRange(location: start, length: typed.utf16.count),
                        sortText: ruleRank(query: typed, candidate: name)
                    )
                }
                .sorted { ($0.sortText, $0.label) < ($1.sortText, $1.label) }
        }

        if let match = suffixMatch(pattern: #"reqScript://([^\s]*)$"#, text: before) {
            return scriptCompletions(
                names: context.requestScripts,
                typed: match.captures[0],
                safeOffset: safeOffset,
                kind: .requestScript,
                detailPrefix: "Request Script"
            )
        }
        if let match = suffixMatch(pattern: #"resScript://([^\s]*)$"#, text: before) {
            return scriptCompletions(
                names: context.responseScripts,
                typed: match.captures[0],
                safeOffset: safeOffset,
                kind: .responseScript,
                detailPrefix: "Response Script"
            )
        }
        if let match = suffixMatch(pattern: #"bp://([^\s]*)$"#, text: before) {
            return scriptCompletions(
                names: context.parserScripts,
                typed: match.captures[0],
                safeOffset: safeOffset,
                kind: .parserScript,
                detailPrefix: "Parser Script"
            )
        }

        if let match = suffixMatch(pattern: #"\{([^}\s]*)$"#, text: before) {
            let typed = match.captures[0]
            let range = NSRange(location: safeOffset - match.fullMatch.utf16.count, length: match.fullMatch.utf16.count)
            let locals = context.localVariables
                .filter { typed.isEmpty || fuzzyMatch(query: typed, candidate: $0.name) }
                .map { local in
                    BifrostCompletionItem(
                        label: "{\(local.name)}",
                        insertText: "{\(local.name)}",
                        detail: "Local Variable: \(local.name) (line \(local.line))",
                        kind: .localVariable,
                        replacementRange: range,
                        sortText: "0_\(local.name)"
                    )
                }
            let globals = context.values
                .filter { typed.isEmpty || fuzzyMatch(query: typed, candidate: $0) }
                .map { name in
                    BifrostCompletionItem(
                        label: "{\(name)}",
                        insertText: "{\(name)}",
                        detail: "Global Value: \(name)",
                        kind: .value,
                        replacementRange: range,
                        sortText: "1_\(name)"
                    )
                }
            return (locals + globals).sorted { ($0.sortText, $0.label) < ($1.sortText, $1.label) }
        }

        return []
    }

    public func reference(
        in text: String,
        cursor: BifrostTextPosition,
        context: BifrostRuleEditorContext
    ) -> BifrostReferenceMatch? {
        let nsText = text as NSString
        let offset = min(max(cursor.utf16Offset, 0), nsText.length)
        let candidates = referenceCandidates(in: text, context: context)
        return candidates.first { NSLocationInRange(offset, expanded($0.range)) }
    }

    public func navigationTarget(
        for reference: BifrostReferenceMatch,
        context: BifrostRuleEditorContext
    ) -> BifrostNavigationTarget? {
        switch reference.type {
        case .localVariable:
            return context.localVariables.first { $0.name == reference.name }.map { .editorLine($0.line) }
        case .value:
            return context.values.contains(reference.name) ? .value(name: reference.name) : nil
        case .requestScript:
            return context.requestScripts.contains(reference.name) ? .script(type: .request, name: reference.name) : nil
        case .responseScript:
            return context.responseScripts.contains(reference.name) ? .script(type: .response, name: reference.name) : nil
        case .parserScript:
            return context.parserScripts.contains(reference.name) ? .script(type: .parser, name: reference.name) : nil
        case .rule:
            return context.ruleNames.contains(reference.name) ? .rule(group: context.currentGroupName, name: reference.name) : nil
        }
    }

    public func localVariables(in text: String) -> [BifrostLocalVariable] {
        let nsText = text as NSString
        var result: [BifrostLocalVariable] = []
        var seen = Set<String>()
        var inCodeFence = false
        nsText.enumerateSubstrings(
            in: NSRange(location: 0, length: nsText.length),
            options: [.byLines, .substringNotRequired]
        ) { _, lineRange, _, _ in
            let line = nsText.substring(with: lineRange)
            let lineNumber = nsText.substring(to: lineRange.location).filter { $0 == "\n" }.count + 1
            let trimmed = line.trimmingCharacters(in: .whitespaces)
            if inCodeFence {
                if trimmed.hasPrefix("```") {
                    inCodeFence = false
                }
                return
            }
            if let blockName = codeFenceVariableName(in: line) {
                if seen.insert(blockName).inserted {
                    result.append(BifrostLocalVariable(name: blockName, line: lineNumber))
                }
                inCodeFence = true
                return
            }
            guard let match = firstMatch(pattern: #"^\s*([A-Za-z_][A-Za-z0-9_\-]*)\s*="#, text: line) else {
                return
            }
            let name = match.captures[0]
            guard seen.insert(name).inserted else {
                return
            }
            result.append(BifrostLocalVariable(name: name, line: lineNumber))
        }
        return result
    }

    public func diagnostics(
        in text: String,
        context: BifrostRuleEditorContext = .empty
    ) -> [BifrostRuleDiagnostic] {
        let nsText = text as NSString
        var diagnostics: [BifrostRuleDiagnostic] = []
        var inCodeFence: (line: Int, range: NSRange)?
        var inLineBlock: (line: Int, range: NSRange)?

        nsText.enumerateSubstrings(
            in: NSRange(location: 0, length: nsText.length),
            options: [.byLines, .substringNotRequired]
        ) { _, lineRange, _, _ in
            let line = nsText.substring(with: lineRange)
            let lineNumber = nsText.substring(to: lineRange.location).filter { $0 == "\n" }.count + 1
            let trimmed = line.trimmingCharacters(in: .whitespaces)

            if inCodeFence != nil {
                if trimmed.hasPrefix("```") {
                    inCodeFence = nil
                }
                return
            }
            if inLineBlock != nil {
                if trimmed == "`" {
                    inLineBlock = nil
                }
                return
            }

            if trimmed.isEmpty || trimmed.hasPrefix("#") {
                return
            }
            if trimmed.hasPrefix("```") {
                inCodeFence = (lineNumber, lineRange)
                return
            }
            if trimmed == "line`" {
                inLineBlock = (lineNumber, lineRange)
                return
            }

            diagnostics.append(contentsOf: structuralDiagnostics(in: line, lineRange: lineRange, lineNumber: lineNumber))
            diagnostics.append(contentsOf: referenceDiagnostics(
                in: line,
                lineRange: lineRange,
                lineNumber: lineNumber,
                context: context
            ))
            if lineNeedsOperationDiagnostic(line) {
                diagnostics.append(BifrostRuleDiagnostic(
                    severity: .warning,
                    message: "Rule line should include a pattern and an operation such as host://, proxy://, or passthrough://.",
                    line: lineNumber,
                    range: lineRange
                ))
            }
        }

        if let inCodeFence {
            diagnostics.append(BifrostRuleDiagnostic(
                severity: .error,
                message: "Unclosed fenced value block. Add a closing ``` line.",
                line: inCodeFence.line,
                range: inCodeFence.range
            ))
        }
        if let inLineBlock {
            diagnostics.append(BifrostRuleDiagnostic(
                severity: .error,
                message: "Unclosed line` block. Add a closing ` line.",
                line: inLineBlock.line,
                range: inLineBlock.range
            ))
        }

        return diagnostics.sorted { ($0.line, $0.range.location, $0.message) < ($1.line, $1.range.location, $1.message) }
    }
}

private struct RegexMatch {
    var fullMatch: String
    var captures: [String]
}

private func regexTokens(pattern: String, in line: String, lineRange: NSRange, kind: BifrostRuleTokenKind) -> [BifrostRuleToken] {
    guard let regex = try? NSRegularExpression(pattern: pattern) else {
        return []
    }
    let nsLine = line as NSString
    return regex.matches(in: line, range: NSRange(location: 0, length: nsLine.length)).map {
        BifrostRuleToken(
            kind: kind,
            range: NSRange(location: lineRange.location + $0.range.location, length: $0.range.length)
        )
    }
}

private func keyValueTokens(in line: String, lineRange: NSRange) -> [BifrostRuleToken] {
    guard let regex = try? NSRegularExpression(pattern: #"^\s*([^()#=\s]+)\s*(=)\s*([^#]+)"#) else {
        return []
    }
    let nsLine = line as NSString
    guard let match = regex.firstMatch(in: line, range: NSRange(location: 0, length: nsLine.length)),
          match.numberOfRanges >= 4 else {
        return []
    }
    return [
        BifrostRuleToken(kind: .attributeName, range: offset(match.range(at: 1), by: lineRange.location)),
        BifrostRuleToken(kind: .delimiter, range: offset(match.range(at: 2), by: lineRange.location)),
        BifrostRuleToken(kind: .attributeValue, range: offset(match.range(at: 3), by: lineRange.location)),
    ]
}

private func codeFenceOpenTokens(line: String, lineRange: NSRange) -> [BifrostRuleToken] {
    let nsLine = line as NSString
    guard let regex = try? NSRegularExpression(pattern: #"^(\s*)(```)\s*([A-Za-z_][A-Za-z0-9_\-.]*)?.*$"#),
          let match = regex.firstMatch(in: line, range: NSRange(location: 0, length: nsLine.length)) else {
        return [BifrostRuleToken(kind: .codeFence, range: lineRange)]
    }
    var tokens = [BifrostRuleToken(kind: .codeFence, range: offset(match.range(at: 2), by: lineRange.location))]
    if match.numberOfRanges > 3, match.range(at: 3).location != NSNotFound {
        tokens.append(BifrostRuleToken(kind: .localVariable, range: offset(match.range(at: 3), by: lineRange.location)))
    }
    return tokens
}

private func codeFenceVariableName(in line: String) -> String? {
    guard let match = firstMatch(pattern: #"^\s*```\s*([A-Za-z_][A-Za-z0-9_\-.]*)\b"#, text: line) else {
        return nil
    }
    return match.captures[0]
}

private func referenceCandidates(in text: String, context: BifrostRuleEditorContext) -> [BifrostReferenceMatch] {
    let nsText = text as NSString
    var matches: [BifrostReferenceMatch] = []
    addReferenceMatches(pattern: #"@([A-Za-z0-9_.\-]+)"#, text: text, nsText: nsText, type: .rule, output: &matches)
    addReferenceMatches(pattern: #"\$?\{([A-Za-z0-9_\-]+)\}"#, text: text, nsText: nsText, type: .value, output: &matches) { name in
        context.localVariables.contains { $0.name == name } ? .localVariable : .value
    }
    addReferenceMatches(pattern: #"\breqScript://([^#\s]+)"#, text: text, nsText: nsText, type: .requestScript, output: &matches)
    addReferenceMatches(pattern: #"\bresScript://([^#\s]+)"#, text: text, nsText: nsText, type: .responseScript, output: &matches)
    addReferenceMatches(pattern: #"\bbp://([^#\s]+)"#, text: text, nsText: nsText, type: .parserScript, output: &matches)
    return matches.sorted { $0.range.location < $1.range.location }
}

private func structuralDiagnostics(in line: String, lineRange: NSRange, lineNumber: Int) -> [BifrostRuleDiagnostic] {
    var diagnostics: [BifrostRuleDiagnostic] = []
    let nsLine = line as NSString
    let openVariablePattern = #"\$?\{[^}\s]*(?:\s|$)"#
    if let regex = try? NSRegularExpression(pattern: openVariablePattern) {
        for match in regex.matches(in: line, range: NSRange(location: 0, length: nsLine.length)) {
            let value = nsLine.substring(with: match.range)
            guard !value.contains("}") else {
                continue
            }
            diagnostics.append(BifrostRuleDiagnostic(
                severity: .error,
                message: "Unclosed value reference. Add a closing }.",
                line: lineNumber,
                range: offset(match.range, by: lineRange.location)
            ))
        }
    }

    if let regex = try? NSRegularExpression(pattern: #"(?<!:)/(?:/|$)"#) {
        for match in regex.matches(in: line, range: NSRange(location: 0, length: nsLine.length)) {
            diagnostics.append(BifrostRuleDiagnostic(
                severity: .warning,
                message: "Operation schemes use ://, for example host://127.0.0.1:3000.",
                line: lineNumber,
                range: offset(match.range, by: lineRange.location)
            ))
        }
    }
    return diagnostics
}

private func referenceDiagnostics(
    in line: String,
    lineRange: NSRange,
    lineNumber: Int,
    context: BifrostRuleEditorContext
) -> [BifrostRuleDiagnostic] {
    let nsLine = line as NSString
    let localNames = Set(context.localVariables.map(\.name))
    var diagnostics: [BifrostRuleDiagnostic] = []

    diagnostics.append(contentsOf: missingReferenceDiagnostics(
        pattern: #"@([A-Za-z0-9_.\-]+)"#,
        line: line,
        nsLine: nsLine,
        lineRange: lineRange,
        lineNumber: lineNumber,
        known: Set(context.ruleNames),
        message: { "Rule reference @\($0) does not match a loaded rule." }
    ))
    diagnostics.append(contentsOf: missingReferenceDiagnostics(
        pattern: #"\$?\{([A-Za-z0-9_\-]+)\}"#,
        line: line,
        nsLine: nsLine,
        lineRange: lineRange,
        lineNumber: lineNumber,
        known: Set(context.values).union(localNames),
        message: { "Value reference {\($0)} does not match a loaded value or local variable." }
    ))
    diagnostics.append(contentsOf: missingReferenceDiagnostics(
        pattern: #"\breqScript://([^#\s]+)"#,
        line: line,
        nsLine: nsLine,
        lineRange: lineRange,
        lineNumber: lineNumber,
        known: Set(context.requestScripts),
        message: { "Request script \($0) is not in the loaded script list." }
    ))
    diagnostics.append(contentsOf: missingReferenceDiagnostics(
        pattern: #"\bresScript://([^#\s]+)"#,
        line: line,
        nsLine: nsLine,
        lineRange: lineRange,
        lineNumber: lineNumber,
        known: Set(context.responseScripts),
        message: { "Response script \($0) is not in the loaded script list." }
    ))
    diagnostics.append(contentsOf: missingReferenceDiagnostics(
        pattern: #"\bbp://([^#\s]+)"#,
        line: line,
        nsLine: nsLine,
        lineRange: lineRange,
        lineNumber: lineNumber,
        known: Set(context.parserScripts),
        message: { "Parser script \($0) is not in the loaded script list." }
    ))

    return diagnostics
}

private func missingReferenceDiagnostics(
    pattern: String,
    line: String,
    nsLine: NSString,
    lineRange: NSRange,
    lineNumber: Int,
    known: Set<String>,
    message: (String) -> String
) -> [BifrostRuleDiagnostic] {
    guard !known.isEmpty,
          let regex = try? NSRegularExpression(pattern: pattern) else {
        return []
    }
    return regex.matches(in: line, range: NSRange(location: 0, length: nsLine.length)).compactMap { match in
        guard match.numberOfRanges >= 2 else {
            return nil
        }
        let name = nsLine.substring(with: match.range(at: 1))
        guard !known.contains(name) else {
            return nil
        }
        return BifrostRuleDiagnostic(
            severity: .warning,
            message: message(name),
            line: lineNumber,
            range: offset(match.range, by: lineRange.location)
        )
    }
}

private func lineNeedsOperationDiagnostic(_ line: String) -> Bool {
    let trimmed = line.trimmingCharacters(in: .whitespaces)
    guard !trimmed.isEmpty,
          !trimmed.hasPrefix("@"),
          !trimmed.hasPrefix("```"),
          trimmed != "line`",
          !trimmed.contains("://"),
          !trimmed.contains("=") else {
        return false
    }
    return true
}

private func addReferenceMatches(
    pattern: String,
    text: String,
    nsText: NSString,
    type: BifrostReferenceType,
    output: inout [BifrostReferenceMatch],
    typeResolver: ((String) -> BifrostReferenceType)? = nil
) {
    guard let regex = try? NSRegularExpression(pattern: pattern) else {
        return
    }
    for match in regex.matches(in: text, range: NSRange(location: 0, length: nsText.length)) where match.numberOfRanges >= 2 {
        let name = nsText.substring(with: match.range(at: 1))
        output.append(BifrostReferenceMatch(name: name, type: typeResolver?(name) ?? type, range: match.range))
    }
}

private func firstMatch(pattern: String, text: String) -> RegexMatch? {
    guard let regex = try? NSRegularExpression(pattern: pattern) else {
        return nil
    }
    let nsText = text as NSString
    guard let match = regex.firstMatch(in: text, range: NSRange(location: 0, length: nsText.length)) else {
        return nil
    }
    return RegexMatch(
        fullMatch: nsText.substring(with: match.range),
        captures: (1..<match.numberOfRanges).map { nsText.substring(with: match.range(at: $0)) }
    )
}

private func suffixMatch(pattern: String, text: String) -> RegexMatch? {
    guard let match = firstMatch(pattern: pattern, text: text) else {
        return nil
    }
    return match
}

private func scriptCompletions(
    names: [String],
    typed: String,
    safeOffset: Int,
    kind: BifrostCompletionKind,
    detailPrefix: String
) -> [BifrostCompletionItem] {
    names
        .filter { typed.isEmpty || $0.localizedCaseInsensitiveContains(typed) }
        .map { name in
            BifrostCompletionItem(
                label: name,
                insertText: name,
                detail: "\(detailPrefix): \(name)",
                kind: kind,
                replacementRange: NSRange(location: safeOffset - typed.utf16.count, length: typed.utf16.count),
                sortText: "0_\(name)"
            )
        }
        .sorted { ($0.sortText, $0.label) < ($1.sortText, $1.label) }
}

private func fuzzyMatch(query: String, candidate: String) -> Bool {
    guard !query.isEmpty else {
        return true
    }
    let needle = Array(query.lowercased())
    let haystack = candidate.lowercased()
    if haystack.contains(query.lowercased()) {
        return true
    }
    var index = needle.startIndex
    for character in haystack {
        if character == needle[index] {
            index = needle.index(after: index)
            if index == needle.endIndex {
                return true
            }
        }
    }
    return false
}

private func ruleRank(query: String, candidate: String) -> String {
    let query = query.lowercased()
    let candidateLower = candidate.lowercased()
    if query.isEmpty {
        return "2_\(candidate)"
    }
    if candidateLower.hasPrefix(query) {
        return "0_\(candidate)"
    }
    if candidateLower.contains(query) {
        return "1_\(candidate)"
    }
    return "2_\(candidate)"
}

private func merge(tokens: [BifrostRuleToken]) -> [BifrostRuleToken] {
    tokens
        .filter { $0.range.location != NSNotFound && $0.range.length > 0 }
        .sorted {
            if $0.range.location == $1.range.location {
                return $0.range.length > $1.range.length
            }
            return $0.range.location < $1.range.location
        }
}

private func offset(_ range: NSRange, by value: Int) -> NSRange {
    NSRange(location: range.location + value, length: range.length)
}

private func leadingRange(_ lineRange: NSRange, length: Int) -> NSRange {
    NSRange(location: lineRange.location, length: min(lineRange.length, length))
}

private func expanded(_ range: NSRange) -> NSRange {
    NSRange(location: range.location, length: max(range.length, 1))
}
