import AvenUniFFI
import Foundation

public enum URLSessionTransportError: Error, Equatable, Sendable {
    case invalidURL
    case nonHTTPResponse
    case invalidStatus(Int)
    case responseTooLarge(limit: Int)
}

public struct URLSessionTransport: Sendable {
    public static let defaultMaxResponseBytes = 64 * 1024 * 1024

    private let session: URLSession
    private let maxResponseBytes: Int

    public init(
        session: URLSession = .shared,
        maxResponseBytes: Int = Self.defaultMaxResponseBytes
    ) {
        self.session = session
        self.maxResponseBytes = maxResponseBytes
    }

    public func send(_ prepared: PreparedSyncRequest) async throws -> SyncHttpResponse {
        guard let url = URL(string: prepared.url) else {
            throw URLSessionTransportError.invalidURL
        }

        var request = URLRequest(url: url)
        request.httpMethod = prepared.method
        request.httpBody = prepared.body
        for header in prepared.headers {
            request.setValue(header.value, forHTTPHeaderField: header.name)
        }

        let (bytes, response) = try await session.bytes(for: request)
        guard let response = response as? HTTPURLResponse else {
            throw URLSessionTransportError.nonHTTPResponse
        }
        guard let status = UInt16(exactly: response.statusCode) else {
            throw URLSessionTransportError.invalidStatus(response.statusCode)
        }
        if response.expectedContentLength > Int64(maxResponseBytes) {
            throw URLSessionTransportError.responseTooLarge(limit: maxResponseBytes)
        }
        var body = Data()
        if response.expectedContentLength > 0 {
            body.reserveCapacity(Int(response.expectedContentLength))
        }
        for try await byte in bytes {
            guard body.count < maxResponseBytes else {
                throw URLSessionTransportError.responseTooLarge(limit: maxResponseBytes)
            }
            body.append(byte)
        }

        let headers = ["content-encoding", "content-length", "content-type"].compactMap { name in
            response.value(forHTTPHeaderField: name).map {
                SyncHttpHeader(name: name, value: $0)
            }
        }
        return SyncHttpResponse(status: status, headers: headers, body: body)
    }
}

public struct SyncDriver: Sendable {
    private let worker: RustWorker
    private let transport: URLSessionTransport

    public init(
        worker: RustWorker = RustWorker(label: "dev.aven.swift-sync-proof.rust"),
        transport: URLSessionTransport = URLSessionTransport()
    ) {
        self.worker = worker
        self.transport = transport
    }

    public func run(
        databasePath: String,
        server: String,
        authToken: String?
    ) async throws -> SyncSessionSummary {
        let session = try await worker.withClient(at: databasePath) { client in
            try client.startSyncSession(
                server: server,
                authToken: authToken,
                pageBudget: nil
            )
        }

        while let prepared = try await worker.run({
            try session.prepareRequest()
        }) {
            let response: SyncHttpResponse
            do {
                response = try await transport.send(prepared)
            } catch {
                try await worker.run {
                    try session.failRequest(
                        context: prepared.context,
                        message: "URLSession transport failed"
                    )
                }
                throw error
            }
            do {
                _ = try await worker.run {
                    try session.acceptResponse(
                        context: prepared.context,
                        response: response
                    )
                }
            } catch {
                try await worker.run {
                    try session.failRequest(
                        context: prepared.context,
                        message: "sync response rejected"
                    )
                }
                throw error
            }
        }

        return try await worker.run {
            try session.summary()
        }
    }
}
