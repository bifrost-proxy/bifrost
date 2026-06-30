import SwiftUI

struct PlaceholderFeatureView: View {
    let title: String

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Header(title: title, subtitle: "Reserved for the next native milestone")
            Text("This surface will stay API-backed and sidecar-controlled; it will not duplicate proxy data-plane logic in Swift.")
                .foregroundStyle(.secondary)
            Spacer()
        }
        .padding(24)
    }
}
