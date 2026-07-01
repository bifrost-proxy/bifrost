import Foundation

public enum AdminAPIError: Error, Equatable, LocalizedError {
    case invalidBaseURL
    case invalidPath(String)
    case invalidURL(String)

    public var errorDescription: String? {
        switch self {
        case .invalidBaseURL:
            return "Admin API base URL must include a scheme and host."
        case .invalidPath(let path):
            return "Admin API path must be absolute: \(path)"
        case .invalidURL(let url):
            return "Failed to build Admin API URL: \(url)"
        }
    }
}

public enum HTTPMethod: String, Sendable {
    case get = "GET"
    case post = "POST"
    case put = "PUT"
    case patch = "PATCH"
    case delete = "DELETE"

    public var isUnsafe: Bool {
        switch self {
        case .get:
            return false
        case .post, .put, .patch, .delete:
            return true
        }
    }
}

public struct AdminAPIRequestFactory: Sendable {
    public static let csrfHeaderName = "X-Bifrost-CSRF"

    public let baseURL: URL
    public let clientId: String
    public let authToken: String?
    public let csrfToken: String?

    public init(
        baseURL: URL,
        clientId: String = "bifrost-mac-native",
        authToken: String? = nil,
        csrfToken: String? = nil
    ) throws {
        guard baseURL.scheme != nil, baseURL.host != nil else {
            throw AdminAPIError.invalidBaseURL
        }
        self.baseURL = baseURL
        self.clientId = clientId
        self.authToken = authToken
        self.csrfToken = csrfToken
    }

    public func makeURL(path: String, queryItems: [URLQueryItem] = []) throws -> URL {
        guard path.hasPrefix("/") else {
            throw AdminAPIError.invalidPath(path)
        }

        var components = URLComponents(url: baseURL, resolvingAgainstBaseURL: false)
        let basePath = components?.path.trimmingCharacters(in: CharacterSet(charactersIn: "/")) ?? ""
        let apiPath = path.trimmingCharacters(in: CharacterSet(charactersIn: "/"))
        components?.path = "/" + [basePath, "_bifrost/api", apiPath]
            .filter { !$0.isEmpty }
            .joined(separator: "/")
        if !queryItems.isEmpty {
            components?.queryItems = queryItems
        }

        guard let url = components?.url else {
            throw AdminAPIError.invalidURL("\(baseURL)\(path)")
        }
        return url
    }

    public func makeRequest(
        method: HTTPMethod,
        path: String,
        queryItems: [URLQueryItem] = [],
        body: Data? = nil
    ) throws -> URLRequest {
        let url = try makeURL(path: path, queryItems: queryItems)
        var request = URLRequest(url: url)
        request.httpMethod = method.rawValue
        request.httpBody = body
        request.setValue(clientId, forHTTPHeaderField: "X-Client-Id")

        if let authToken, !authToken.isEmpty {
            request.setValue("Bearer \(authToken)", forHTTPHeaderField: "Authorization")
        }
        if method.isUnsafe, let csrfToken, !csrfToken.isEmpty {
            request.setValue(csrfToken, forHTTPHeaderField: Self.csrfHeaderName)
        }
        if body != nil {
            request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        }
        return request
    }
}
