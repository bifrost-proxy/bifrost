import SwiftUI

struct NameEntrySheet: View {
    let title: String
    let prompt: String
    let initialValue: String
    let confirmTitle: String
    let onConfirm: (String) -> Void

    @Environment(\.dismiss) private var dismiss
    @State private var value: String

    init(
        title: String,
        prompt: String,
        initialValue: String,
        confirmTitle: String,
        onConfirm: @escaping (String) -> Void
    ) {
        self.title = title
        self.prompt = prompt
        self.initialValue = initialValue
        self.confirmTitle = confirmTitle
        self.onConfirm = onConfirm
        _value = State(initialValue: initialValue)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            Text(title)
                .font(.system(size: 16, weight: .semibold))
            TextField(prompt, text: $value)
                .textFieldStyle(.roundedBorder)
                .frame(width: 320)
            HStack {
                Spacer()
                Button("Cancel") { dismiss() }
                Button(confirmTitle) {
                    onConfirm(value)
                    dismiss()
                }
                .keyboardShortcut(.defaultAction)
                .disabled(value.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
            }
        }
        .padding(18)
    }
}
