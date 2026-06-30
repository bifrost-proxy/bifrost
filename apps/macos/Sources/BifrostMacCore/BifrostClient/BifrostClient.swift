import Foundation

public actor BifrostClient {
    private let factory: AdminAPIRequestFactory
    private let session: URLSession

    public init(factory: AdminAPIRequestFactory, session: URLSession = .shared) {
        self.factory = factory
        self.session = session
    }

    public init(baseURL: URL) throws {
        self.factory = try AdminAPIRequestFactory(baseURL: baseURL)
        self.session = .shared
    }

    public func getSystemOverview() async throws -> Data {
        try await request(.get, path: "/system/overview")
    }

    public func listTraffic(query: TrafficQuery = TrafficQuery()) async throws -> Data {
        try await request(.get, path: "/traffic", queryItems: query.queryItems)
    }

    public func getTraffic(id: String) async throws -> Data {
        try await request(.get, path: "/traffic/\(id)")
    }

    public func getRequestBody(id: String) async throws -> Data {
        try await request(.get, path: "/traffic/\(id)/request-body")
    }

    public func getResponseBody(id: String) async throws -> Data {
        try await request(.get, path: "/traffic/\(id)/response-body")
    }

    public func listRules() async throws -> Data {
        try await request(.get, path: "/rules")
    }

    public func getCertInfo() async throws -> Data {
        try await request(.get, path: "/cert")
    }

    public func getProxyAddress() async throws -> Data {
        try await request(.get, path: "/proxy/address")
    }

    public func getSystemProxy() async throws -> Data {
        try await request(.get, path: "/proxy/system")
    }

    public func request(
        _ method: HTTPMethod,
        path: String,
        queryItems: [URLQueryItem] = [],
        body: Data? = nil
    ) async throws -> Data {
        let request = try factory.makeRequest(
            method: method,
            path: path,
            queryItems: queryItems,
            body: body
        )
        let (data, response) = try await session.data(for: request)
        if let httpResponse = response as? HTTPURLResponse,
           !(200..<300).contains(httpResponse.statusCode) {
            throw BifrostClientError.httpStatus(httpResponse.statusCode, data)
        }
        return data
    }
}

public enum BifrostClientError: Error, Equatable {
    case httpStatus(Int, Data)
}
