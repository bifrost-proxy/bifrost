import BifrostNativeCore
import SwiftUI

struct RuleEditorHostView: View {
    @Binding var text: String
    var context: BifrostRuleEditorContext
    var isReadOnly = false
    var onNavigate: (BifrostNavigationTarget) -> Void
    var onSave: () -> Void
    var onTextChanged: ((String) -> Void)?

    @State private var diagnostics: [BifrostRuleDiagnostic] = []

    private let languageService = BifrostRuleLanguageService()

    var body: some View {
        VStack(spacing: 0) {
            Group {
                if RuleEditorExperiment.useCodeEditSourceEditor {
                    CodeEditRuleEditorView(
                        text: $text,
                        context: context,
                        isReadOnly: isReadOnly,
                        onSave: onSave,
                        onTextChanged: onTextChanged
                    )
                } else {
                    BifrostRuleEditorView(
                        text: $text,
                        context: context,
                        isReadOnly: isReadOnly,
                        onNavigate: onNavigate,
                        onSave: onSave,
                        onTextChanged: onTextChanged
                    )
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .accessibilityIdentifier(RuleEditorExperiment.useCodeEditSourceEditor ? "rules-editor-codeedit" : "rules-editor-native")

            RuleDiagnosticsBar(diagnostics: diagnostics)
        }
        .onAppear {
            refreshDiagnostics()
        }
        .onChange(of: text) { _ in
            refreshDiagnostics()
        }
        .onChange(of: context) { _ in
            refreshDiagnostics()
        }
    }

    private func refreshDiagnostics() {
        diagnostics = languageService.diagnostics(in: text, context: context)
    }
}

enum RuleEditorExperiment {
    static var useCodeEditSourceEditor: Bool {
        let value = ProcessInfo.processInfo.environment["BIFROST_NATIVE_RULE_EDITOR"]?.lowercased()
        guard value == "codeedit" || value == "codeedit-source-editor" else {
            return false
        }
        return RuleEditorBackendAvailability.codeEditSourceEditor
    }
}

private struct RuleDiagnosticsBar: View {
    let diagnostics: [BifrostRuleDiagnostic]

    private var errors: Int {
        diagnostics.filter { $0.severity == .error }.count
    }

    private var warnings: Int {
        diagnostics.filter { $0.severity == .warning }.count
    }

    var body: some View {
        HStack(spacing: 10) {
            Label(statusTitle, systemImage: statusIcon)
                .font(.system(size: 12, weight: .semibold))
                .foregroundStyle(statusColor)

            if let first = diagnostics.first {
                Divider()
                    .frame(height: 14)
                Text("Line \(first.line): \(first.message)")
                    .font(.system(size: 12))
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .truncationMode(.tail)
            }

            Spacer(minLength: 8)
        }
        .padding(.horizontal, 12)
        .frame(height: 30)
        .background(AppSurface.subtleFill)
        .accessibilityIdentifier("rules-editor-diagnostics")
    }

    private var statusTitle: String {
        if errors > 0 {
            return "\(errors) errors"
        }
        if warnings > 0 {
            return "\(warnings) warnings"
        }
        return "Syntax OK"
    }

    private var statusIcon: String {
        if errors > 0 {
            return "xmark.octagon.fill"
        }
        if warnings > 0 {
            return "exclamationmark.triangle.fill"
        }
        return "checkmark.circle.fill"
    }

    private var statusColor: Color {
        if errors > 0 {
            return .red
        }
        if warnings > 0 {
            return .orange
        }
        return .green
    }
}
