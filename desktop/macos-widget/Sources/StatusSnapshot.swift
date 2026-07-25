import Foundation
import OSLog

let bifrostAppGroupIdentifier = "group.com.bifrost.desktop"
let bifrostWidgetSnapshotFileName = "status.json"
let bifrostWidgetApplicationSupportDirectory = "Bifrost"
let bifrostWidgetSnapshotSchemaVersion = 1
let bifrostWidgetReloadInterval: TimeInterval = 5
let bifrostWidgetStaleInterval: TimeInterval = 30 * 60
let bifrostWidgetTimelineDiagnosticFileName = "timeline.log"

enum ProxyStatus: String, Codable {
    case on
    case off
    case checking
    case unsupported
}

struct StatusSnapshot: Codable, Equatable {
    let schemaVersion: Int
    let sampledAtMs: UInt64
    let cpuPercent: Double?
    let memoryPercent: Double?
    let diskPercent: Double?
    let proxyStatus: ProxyStatus

    var sampledAt: Date {
        Date(timeIntervalSince1970: TimeInterval(sampledAtMs) / 1_000)
    }

    func isStale(at date: Date) -> Bool {
        date.timeIntervalSince(sampledAt) >= bifrostWidgetStaleInterval
    }

    static let placeholder = StatusSnapshot(
        schemaVersion: bifrostWidgetSnapshotSchemaVersion,
        sampledAtMs: UInt64(Date().timeIntervalSince1970 * 1_000),
        cpuPercent: 24,
        memoryPercent: 67,
        diskPercent: 53,
        proxyStatus: .on
    )
}

enum StatusSnapshotStore {
    private static let logger = Logger(
        subsystem: "com.bifrost.desktop.status-widget",
        category: "snapshot"
    )

    static func load(
        fileManager: FileManager = .default,
        decoder: JSONDecoder = JSONDecoder()
    ) -> StatusSnapshot? {
        var candidates = [URL]()
        if let applicationSupport = fileManager.urls(
            for: .applicationSupportDirectory,
            in: .userDomainMask
        ).first {
            candidates.append(
                applicationSupport
                    .appendingPathComponent(bifrostWidgetApplicationSupportDirectory)
                    .appendingPathComponent(bifrostWidgetSnapshotFileName)
            )
        }
        if let container = fileManager.containerURL(
            forSecurityApplicationGroupIdentifier: bifrostAppGroupIdentifier
        ) {
            candidates.append(container.appendingPathComponent(bifrostWidgetSnapshotFileName))
        } else {
            logger.error("App Group container is unavailable")
        }
        return load(from: candidates, decoder: decoder)
    }

    static func load(from candidates: [URL], decoder: JSONDecoder = JSONDecoder()) -> StatusSnapshot? {
        for url in candidates {
            logger.debug("Loading widget snapshot from \(url.path, privacy: .public)")
            if let snapshot = load(from: url, decoder: decoder) {
                return snapshot
            }
        }
        return nil
    }

    static func load(from url: URL, decoder: JSONDecoder = JSONDecoder()) -> StatusSnapshot? {
        let data: Data
        do {
            data = try Data(contentsOf: url)
        } catch {
            logger.error("Unable to read widget snapshot: \(error.localizedDescription, privacy: .public)")
            return nil
        }

        let snapshot: StatusSnapshot
        do {
            snapshot = try decoder.decode(StatusSnapshot.self, from: data)
        } catch {
            logger.error("Unable to decode widget snapshot: \(error.localizedDescription, privacy: .public)")
            return nil
        }

        guard snapshot.schemaVersion == bifrostWidgetSnapshotSchemaVersion else {
            logger.error("Unsupported widget snapshot schema: \(snapshot.schemaVersion)")
            return nil
        }
        return snapshot
    }
}

enum WidgetTimelineDiagnostics {
    private static let maximumBytes: UInt64 = 64 * 1_024

    static func record(
        event: String,
        snapshot: StatusSnapshot?,
        fileManager: FileManager = .default
    ) {
        guard let applicationSupport = fileManager.urls(
            for: .applicationSupportDirectory,
            in: .userDomainMask
        ).first else {
            return
        }
        let directory = applicationSupport
            .appendingPathComponent(bifrostWidgetApplicationSupportDirectory)
        let url = directory.appendingPathComponent(bifrostWidgetTimelineDiagnosticFileName)
        do {
            try fileManager.createDirectory(
                at: directory,
                withIntermediateDirectories: true
            )
            if let attributes = try? fileManager.attributesOfItem(atPath: url.path),
               let size = attributes[.size] as? NSNumber,
               size.uint64Value >= maximumBytes {
                try? fileManager.removeItem(at: url)
            }
            let timestamp = ISO8601DateFormatter().string(from: .now)
            let sampledAt = snapshot.map {
                ISO8601DateFormatter().string(from: $0.sampledAt)
            } ?? "none"
            let proxy = snapshot?.proxyStatus.rawValue ?? "none"
            let line = "\(timestamp) event=\(event) sampledAt=\(sampledAt) proxy=\(proxy)\n"
            let data = Data(line.utf8)
            if !fileManager.fileExists(atPath: url.path) {
                try data.write(to: url, options: .atomic)
                return
            }
            let handle = try FileHandle(forWritingTo: url)
            defer { try? handle.close() }
            try handle.seekToEnd()
            try handle.write(contentsOf: data)
        } catch {
            return
        }
    }
}
