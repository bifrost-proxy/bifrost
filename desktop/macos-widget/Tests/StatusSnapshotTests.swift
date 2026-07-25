import Foundation

private func expect(
    _ condition: @autoclosure () -> Bool,
    _ message: String
) {
    guard condition() else {
        FileHandle.standardError.write(Data("FAIL: \(message)\n".utf8))
        exit(1)
    }
}

@main
struct StatusSnapshotTests {
    static func main() throws {
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent("bifrost-widget-tests-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: root) }

        let validURL = root.appendingPathComponent("valid.json")
        try Data(
            """
            {
              "schemaVersion": 1,
              "sampledAtMs": 1780000000000,
              "cpuPercent": 24.5,
              "memoryPercent": 67.0,
              "diskPercent": null,
              "proxyStatus": "on"
            }
            """.utf8
        ).write(to: validURL)

        let snapshot = StatusSnapshotStore.load(from: validURL)
        expect(snapshot?.cpuPercent == 24.5, "valid CPU percent should decode")
        expect(snapshot?.memoryPercent == 67, "valid memory percent should decode")
        expect(snapshot?.diskPercent == nil, "null disk percent should remain unavailable")
        expect(snapshot?.proxyStatus == .on, "proxy state should decode")

        let missingURL = root.appendingPathComponent("missing.json")
        expect(
            StatusSnapshotStore.load(from: [missingURL, validURL])?.cpuPercent == 24.5,
            "snapshot loading should fall back to the extension container"
        )

        let staleAt = snapshot!.sampledAt.addingTimeInterval(bifrostWidgetStaleInterval)
        expect(!snapshot!.isStale(at: staleAt.addingTimeInterval(-0.001)), "snapshot should be fresh before thirty minutes")
        expect(snapshot!.isStale(at: staleAt), "snapshot should become stale at thirty minutes")

        let unsupportedSchemaURL = root.appendingPathComponent("unsupported.json")
        try Data(
            """
            {
              "schemaVersion": 2,
              "sampledAtMs": 1780000000000,
              "cpuPercent": 1,
              "memoryPercent": 2,
              "diskPercent": 3,
              "proxyStatus": "off"
            }
            """.utf8
        ).write(to: unsupportedSchemaURL)
        expect(
            StatusSnapshotStore.load(from: unsupportedSchemaURL) == nil,
            "unsupported schemas must degrade to missing data"
        )

        let corruptURL = root.appendingPathComponent("corrupt.json")
        try Data("{not-json".utf8).write(to: corruptURL)
        expect(
            StatusSnapshotStore.load(from: corruptURL) == nil,
            "corrupt snapshots must degrade to missing data"
        )

        print("PASS: Swift widget snapshot decoding and staleness")
    }
}
