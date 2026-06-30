import BifrostNativeCore
import Foundation
import SwiftUI

struct APIBackedFeatureView: View {
    @EnvironmentObject private var appModel: AppModel

    let title: String
    let systemImage: String
    let endpoints: [FeatureEndpoint]

    @State private var results: [FeatureEndpointResult] = []
    @State private var isLoading = false

    var body: some View {
        VStack(spacing: 0) {
            header
            Divider()
            if results.isEmpty, !isLoading {
                FeatureLoadingState(title: "No API result loaded")
            } else {
                ScrollView {
                    LazyVStack(spacing: 10) {
                        ForEach(results) { result in
                            FeatureEndpointCard(result: result)
                        }
                    }
                    .padding(14)
                }
            }
        }
        .task {
            await reload()
        }
    }

    private var header: some View {
        HStack(spacing: 10) {
            Image(systemName: systemImage)
                .font(.system(size: 18, weight: .medium))
                .foregroundStyle(Color.accentColor)
            VStack(alignment: .leading, spacing: 2) {
                Text(title)
                    .font(.system(size: 14, weight: .semibold))
                Text("Native status is loaded from Admin API")
                    .font(.system(size: 11))
                    .foregroundStyle(.secondary)
            }
            Spacer()
            if isLoading {
                ProgressView()
                    .controlSize(.small)
            }
            Button {
                Task {
                    await reload()
                }
            } label: {
                Label("Refresh", systemImage: "arrow.clockwise")
            }
            .buttonStyle(.borderless)
        }
        .padding(.horizontal, 14)
        .frame(height: 54)
        .background(.bar)
    }

    private func reload() async {
        isLoading = true
        defer { isLoading = false }
        do {
            let client = try BifrostClient(baseURL: appModel.adminURL)
            var nextResults: [FeatureEndpointResult] = []
            for endpoint in endpoints {
                do {
                    let data = try await client.request(.get, path: endpoint.path, queryItems: endpoint.queryItems)
                    nextResults.append(
                        FeatureEndpointResult(
                            title: endpoint.title,
                            path: endpoint.displayPath,
                            state: .loaded(previewJSON(data))
                        )
                    )
                } catch {
                    nextResults.append(
                        FeatureEndpointResult(
                            title: endpoint.title,
                            path: endpoint.displayPath,
                            state: .failed(error.localizedDescription)
                        )
                    )
                }
            }
            results = nextResults
        } catch {
            results = [
                FeatureEndpointResult(
                    title: "Admin API",
                    path: appModel.adminURL.absoluteString,
                    state: .failed(error.localizedDescription)
                )
            ]
        }
    }

    private func previewJSON(_ data: Data) -> String {
        let object = try? JSONSerialization.jsonObject(with: data)
        let prettyData = object.flatMap {
            try? JSONSerialization.data(withJSONObject: $0, options: [.prettyPrinted, .sortedKeys])
        } ?? data
        let text = String(data: prettyData, encoding: .utf8) ?? "\(data.count) bytes"
        if text.count > 2_400 {
            return String(text.prefix(2_400)) + "\n..."
        }
        return text
    }
}

struct FeatureEndpoint: Sendable {
    let title: String
    let path: String
    var queryItems: [URLQueryItem] = []

    var displayPath: String {
        if queryItems.isEmpty {
            return path
        }
        let query = queryItems
            .map { "\($0.name)=\($0.value ?? "")" }
            .joined(separator: "&")
        return "\(path)?\(query)"
    }
}

private struct FeatureEndpointResult: Identifiable {
    enum State {
        case loaded(String)
        case failed(String)
    }

    var id: String { path }
    let title: String
    let path: String
    let state: State
}

private struct FeatureEndpointCard: View {
    let result: FeatureEndpointResult

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack {
                VStack(alignment: .leading, spacing: 2) {
                    Text(result.title)
                        .font(.system(size: 13, weight: .semibold))
                    Text(result.path)
                        .font(.system(size: 11, design: .monospaced))
                        .foregroundStyle(.secondary)
                }
                Spacer()
            }

            switch result.state {
            case .loaded(let text):
                ScrollView(.horizontal) {
                    Text(text)
                        .font(.system(size: 11, design: .monospaced))
                        .textSelection(.enabled)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(10)
                }
                .background(.quaternary.opacity(0.22), in: RoundedRectangle(cornerRadius: 6))
            case .failed(let error):
                Label(error, systemImage: "exclamationmark.triangle")
                    .font(.system(size: 12))
                    .foregroundStyle(.orange)
            }
        }
        .padding(12)
        .background(.background, in: RoundedRectangle(cornerRadius: 8))
        .overlay(
            RoundedRectangle(cornerRadius: 8)
                .stroke(Color.secondary.opacity(0.16))
        )
    }
}

private struct FeatureLoadingState: View {
    let title: String

    var body: some View {
        VStack(spacing: 10) {
            ProgressView()
            Text(title)
                .foregroundStyle(.secondary)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }
}
