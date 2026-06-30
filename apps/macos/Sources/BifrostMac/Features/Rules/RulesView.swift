import SwiftUI

struct RulesView: View {
    @State private var ruleDraft = """
    # Native preview rule editor scaffold
    example.com proxy://127.0.0.1:8080
    """

    var body: some View {
        VStack(spacing: 0) {
            Header(title: "Rules", subtitle: "Native NSTextView bridge for the rule DSL")
                .padding(24)

            CodeEditorView(text: $ruleDraft)
                .frame(minHeight: 420)

            Divider()
            HStack {
                Button("Validate") {}
                    .disabled(true)
                Button("Save") {}
                    .disabled(true)
                Spacer()
            }
            .padding(16)
        }
    }
}
