import AvenUniFFI
import Foundation

public enum URLSessionTransportError: Error, Equatable, Sendable {
    case invalidURL
    case nonHTTPResponse
    case invalidStatus(Int)
    case responseTooLarge(limit: Int)
    case attemptTimedOut
    case cancellationDidNotStart
    case cancellationWasNotObserved
}

public struct URLSessionTransport: Sendable {
    public static let defaultMaxResponseBytes = 64 * 1024 * 1024

    private let session: URLSession
    private let maxResponseBytes: Int

    public init(
        session: URLSession? = nil,
        maxResponseBytes: Int = Self.defaultMaxResponseBytes
    ) {
        if let session {
            self.session = session
        } else {
            let configuration = URLSessionConfiguration.ephemeral
            self.session = URLSession(
                configuration: configuration,
                delegate: NoRedirectURLSessionDelegate(),
                delegateQueue: nil
            )
        }
        self.maxResponseBytes = maxResponseBytes
    }

    public func send(_ prepared: PreparedSyncRequest) async throws -> SyncHttpResponse {
        let attempt = Task {
            try await sendOnce(prepared)
        }
        defer { attempt.cancel() }
        return try await withTaskCancellationHandler {
            try await withThrowingTaskGroup(of: SyncHttpResponse.self) { group in
                group.addTask { try await attempt.value }
                group.addTask {
                    try await Task.sleep(
                        for: .milliseconds(prepared.timeout.attemptMs)
                    )
                    throw URLSessionTransportError.attemptTimedOut
                }
                guard let response = try await group.next() else {
                    throw URLSessionTransportError.attemptTimedOut
                }
                return response
            }
        } onCancel: {
            attempt.cancel()
        }
    }

    private func sendOnce(_ prepared: PreparedSyncRequest) async throws -> SyncHttpResponse {
        guard let url = URL(string: prepared.url) else {
            throw URLSessionTransportError.invalidURL
        }

        var request = URLRequest(url: url)
        request.httpMethod = prepared.method
        request.httpBody = prepared.body
        request.timeoutInterval = TimeInterval(prepared.timeout.inactivityMs) / 1000
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

        let headers = [
            "content-encoding",
            "content-length",
            "content-type",
            "retry-after",
        ].compactMap { name in
            response.value(forHTTPHeaderField: name).map {
                SyncHttpHeader(name: name, value: $0)
            }
        }
        return SyncHttpResponse(status: status, headers: headers, body: body)
    }

    public func waitForProcessTermination(
        _ prepared: PreparedSyncRequest
    ) async throws -> Never {
        guard let url = URL(string: prepared.url) else {
            throw URLSessionTransportError.invalidURL
        }
        var request = URLRequest(url: url)
        request.httpMethod = prepared.method
        request.httpBody = prepared.body
        request.timeoutInterval = TimeInterval(prepared.timeout.inactivityMs) / 1000
        for header in prepared.headers {
            request.setValue(header.value, forHTTPHeaderField: header.name)
        }
        let configuration = URLSessionConfiguration.ephemeral
        configuration.protocolClasses = [TerminationWaitURLProtocol.self]
        let waitSession = URLSession(configuration: configuration)
        defer { waitSession.invalidateAndCancel() }
        _ = try await waitSession.data(for: request)
        throw URLSessionTransportError.cancellationWasNotObserved
    }
}

public struct CancelledRequestResult: Sendable {
    public let failRequestCount: Int
    public let cursorBefore: Int64
    public let cursorAfter: Int64
}

public struct SyncRunMeasurement: Sendable {
    public let summary: SyncSessionSummary
    public let requestBodyBytes: Int
    public let responseBodyBytes: Int
}

public struct SyncDriver: Sendable {
    private let worker: RustWorker
    private let transport: URLSessionTransport

    public init(
        worker: RustWorker = RustWorker(label: "com.raine.aven.ios-proof.sync"),
        transport: URLSessionTransport = URLSessionTransport()
    ) {
        self.worker = worker
        self.transport = transport
    }

    public func run(
        databasePath: String,
        server: String,
        authToken: String
    ) async throws -> SyncSessionSummary {
        try await runMeasured(
            databasePath: databasePath,
            server: server,
            authToken: authToken
        ).summary
    }

    public func runMeasured(
        databasePath: String,
        server: String,
        authToken: String
    ) async throws -> SyncRunMeasurement {
        let session = try await worker.withClient(at: databasePath) { client in
            try client.startSyncSession(
                server: server,
                authToken: authToken,
                pageBudget: nil
            )
        }
        var requestBodyBytes = 0
        var responseBodyBytes = 0

        while let prepared = try await worker.run({
            try session.prepareRequest()
        }) {
            requestBodyBytes += prepared.body.count
            let response: SyncHttpResponse
            do {
                response = try await sendOutstanding(
                    prepared,
                    session: session
                )
                responseBodyBytes += response.body.count
            } catch {
                let message = isCancellation(error)
                    ? "sync cancelled"
                    : "sync transport failed"
                try await worker.run {
                    try session.failRequest(
                        context: prepared.context,
                        message: message
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

        let summary = try await worker.run {
            try session.summary()
        }
        return SyncRunMeasurement(
            summary: summary,
            requestBodyBytes: requestBodyBytes,
            responseBodyBytes: responseBodyBytes
        )
    }

    private func sendOutstanding(
        _ prepared: PreparedSyncRequest,
        session: AvenSyncSession
    ) async throws -> SyncHttpResponse {
        while true {
            let response: SyncHttpResponse
            do {
                response = try await transport.send(prepared)
            } catch {
                if isCancellation(error) || !isTransientTransport(error) {
                    throw error
                }
                let decision = try await worker.run {
                    try session.registerTransportFailure(
                        context: prepared.context
                    )
                }
                switch decision {
                case let .retryAfter(delayMs):
                    try await Task.sleep(
                        for: .milliseconds(Int64(clamping: delayMs))
                    )
                    continue
                case .stop:
                    throw error
                }
            }

            guard !(200 ..< 300).contains(Int(response.status)) else {
                return response
            }
            let decision = try await worker.run {
                try session.registerHttpFailure(
                    context: prepared.context,
                    status: response.status,
                    headers: response.headers
                )
            }
            switch decision {
            case let .retryAfter(delayMs):
                try await Task.sleep(
                    for: .milliseconds(Int64(clamping: delayMs))
                )
            case .stop:
                return response
            }
        }
    }

    public func waitForProcessTermination(
        _ prepared: PreparedSyncRequest
    ) async throws -> Never {
        try await transport.waitForProcessTermination(prepared)
    }

    public func cancelOneDelayedRequest(
        databasePath: String,
        server: String,
        authToken: String,
        beforeCancellation: (@MainActor @Sendable () async -> Void)? = nil
    ) async throws -> CancelledRequestResult {
        let session = try await worker.withClient(at: databasePath) { client in
            try client.startSyncSession(
                server: server,
                authToken: authToken,
                pageBudget: nil
            )
        }
        let summaryBefore = try await worker.run { try session.summary() }
        guard let prepared = try await worker.run({ try session.prepareRequest() }) else {
            throw URLSessionTransportError.cancellationDidNotStart
        }

        let state = CancellationURLProtocol.state
        state.reset()
        let configuration = URLSessionConfiguration.ephemeral
        configuration.protocolClasses = [CancellationURLProtocol.self]
        let urlSession = URLSession(configuration: configuration)
        defer { urlSession.invalidateAndCancel() }
        let cancellationTransport = URLSessionTransport(session: urlSession)
        let requestTask = Task {
            try await cancellationTransport.send(prepared)
        }

        try await state.waitUntilStarted()
        if let beforeCancellation {
            await beforeCancellation()
        }
        try await Task.sleep(for: .milliseconds(100))
        requestTask.cancel()
        do {
            _ = try await requestTask.value
            throw URLSessionTransportError.cancellationWasNotObserved
        } catch is CancellationError {
        } catch let error as URLError where error.code == .cancelled {
        } catch {
            throw error
        }

        var failRequestCount = 0
        try await worker.run {
            try session.failRequest(
                context: prepared.context,
                message: "sync cancelled"
            )
        }
        failRequestCount += 1
        let summaryAfter = try await worker.run { try session.summary() }
        guard state.stopCount == 1 else {
            throw URLSessionTransportError.cancellationWasNotObserved
        }

        return CancelledRequestResult(
            failRequestCount: failRequestCount,
            cursorBefore: summaryBefore.cursor,
            cursorAfter: summaryAfter.cursor
        )
    }
}

private func isCancellation(_ error: Error) -> Bool {
    error is CancellationError || (error as? URLError)?.code == .cancelled
}

private func isTransientTransport(_ error: Error) -> Bool {
    if case URLSessionTransportError.attemptTimedOut = error {
        return true
    }
    guard let error = error as? URLError else {
        return false
    }
    return switch error.code {
    case .timedOut,
         .cannotFindHost,
         .cannotConnectToHost,
         .networkConnectionLost,
         .dnsLookupFailed,
         .notConnectedToInternet:
        true
    default:
        false
    }
}

private final class NoRedirectURLSessionDelegate: NSObject, URLSessionTaskDelegate,
    @unchecked Sendable
{
    func urlSession(
        _: URLSession,
        task _: URLSessionTask,
        willPerformHTTPRedirection _: HTTPURLResponse,
        newRequest _: URLRequest,
        completionHandler: @escaping (URLRequest?) -> Void
    ) {
        completionHandler(nil)
    }
}

private final class TerminationWaitURLProtocol: URLProtocol, @unchecked Sendable {
    override class func canInit(with _: URLRequest) -> Bool {
        true
    }

    override class func canonicalRequest(for request: URLRequest) -> URLRequest {
        request
    }

    override func startLoading() {
        ProofOutput.write("AVEN_IOS_TERMINATION_NETWORK status=ready\n")
    }

    override func stopLoading() {}
}

private final class CancellationURLProtocol: URLProtocol, @unchecked Sendable {
    static let state = CancellationURLProtocolState()

    override class func canInit(with _: URLRequest) -> Bool {
        true
    }

    override class func canonicalRequest(for request: URLRequest) -> URLRequest {
        request
    }

    override func startLoading() {
        Self.state.recordStart()
    }

    override func stopLoading() {
        Self.state.recordStop()
    }
}

private final class CancellationURLProtocolState: @unchecked Sendable {
    private let lock = NSLock()
    private var started = false
    private var stops = 0

    var stopCount: Int {
        lock.withLock { stops }
    }

    func reset() {
        lock.withLock {
            started = false
            stops = 0
        }
    }

    func recordStart() {
        lock.withLock {
            started = true
        }
    }

    func recordStop() {
        lock.withLock {
            stops += 1
        }
    }

    func waitUntilStarted() async throws {
        for _ in 0 ..< 2000 {
            if lock.withLock({ started }) {
                return
            }
            try await Task.sleep(for: .milliseconds(1))
        }
        throw URLSessionTransportError.cancellationDidNotStart
    }
}
