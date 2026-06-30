import SwiftUI

struct TrafficView: View {
    @State private var records = TrafficRecord.sampleRows

    var body: some View {
        VStack(spacing: 0) {
            Header(title: "Traffic", subtitle: "AppKit table scaffold with lazy-detail boundary")
                .padding(24)

            RequestTableView(records: records)
                .frame(minHeight: 360)

            Divider()

            HStack {
                Text("Select a request to load headers and body through Admin API endpoints.")
                    .foregroundStyle(.secondary)
                Spacer()
            }
            .padding(16)
        }
    }
}

struct TrafficRecord: Identifiable, Equatable {
    let id: String
    let method: String
    let host: String
    let path: String
    let status: String
    let duration: String

    static let sampleRows = [
        TrafficRecord(id: "REQ-preview-0001", method: "GET", host: "example.com", path: "/", status: "200", duration: "42 ms"),
        TrafficRecord(id: "REQ-preview-0002", method: "POST", host: "api.local", path: "/v1/replay", status: "201", duration: "88 ms")
    ]
}
