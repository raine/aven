import AvenUniFFI
import Foundation

public enum URLSessionTransportError: Error, Equatable, Sendable {
    case invalidURL
    case nonHTTPResponse
    case invalidStatus(Int)
}

public struct URLSessionTransport: Sendable {
    private let session: URLSession

    public init(session: URLSession = .shared) {
        self.session = session
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

        let (body, response) = try await session.data(for: request)
        guard let response = response as? HTTPURLResponse else {
            throw URLSessionTransportError.nonHTTPResponse
        }
        guard let status = UInt16(exactly: response.statusCode) else {
            throw URLSessionTransportError.invalidStatus(response.statusCode)
        }

        let headers = ["content-type"].compactMap { name in
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
            _ = try await worker.run {
                try session.acceptResponse(
                    context: prepared.context,
                    response: response
                )
            }
        }

        return try await worker.run {
            try session.summary()
        }
    }
}
