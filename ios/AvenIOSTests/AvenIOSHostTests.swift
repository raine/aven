@testable import AvenIOS
import AvenUniFFI
import Foundation
import XCTest

final class AvenIOSHostTests: XCTestCase {
    @MainActor
    func testTypedFacadeCallsKeepMainActorHeartbeatProgressing() async throws {
        let result = try await HostSmokeProof().run()

        XCTAssertGreaterThanOrEqual(result.facadeCallCount, 100)
        XCTAssertLessThanOrEqual(result.facadeCallCount, 10000)
        XCTAssertGreaterThan(result.heartbeatTickCount, 0)
        print(
            "AVEN_IOS_HOST_TEST status=pass facade=typed " +
                "worker=serial heartbeat=progressing"
        )
    }

    @MainActor
    func testSandboxPersistenceAndTypedTaskMappings() async throws {
        let result = try await PersistenceProof().run()

        XCTAssertGreaterThanOrEqual(result.workspaceCount, 1)
        XCTAssertEqual(result.taskCount, 1)
        XCTAssertEqual(
            result.statusNames,
            ["inbox", "backlog", "todo", "active", "done", "canceled"]
        )
        XCTAssertEqual(
            result.priorityNames,
            ["none", "low", "medium", "high", "urgent"]
        )
        XCTAssertEqual(result.validationErrorCount, 2)
        XCTAssertEqual(result.notFoundErrorCount, 1)
        XCTAssertTrue(result.workspaceMismatchMatched)
        XCTAssertTrue(result.walObservedBeforeRelease)
        XCTAssertTrue(result.shmObservedBeforeRelease)
        XCTAssertTrue(result.walObservedAfterReopen)
        XCTAssertTrue(result.shmObservedAfterReopen)
        XCTAssertTrue(result.dataProtectionConfigured)
        XCTAssertEqual(result.storagePathCount, 5)
        print(
            "AVEN_IOS_PERSISTENCE_TEST status=pass persistence=reopen " +
                "types=complete storage=application_support attachments=none " +
                "wal_shm=reopen protection=complete_until_first_authentication"
        )
    }

    func testCancellingSwiftWaitDoesNotCancelRustWorkerOperation() async throws {
        let paths = try ApplicationSupportPath.prepareSyncProof(reset: true)
        let worker = RustWorker(label: "com.raine.aven.ios-proof.cancellation-test")
        let started = DispatchSemaphore(value: 0)
        let release = DispatchSemaphore(value: 0)
        let operation = Task {
            try await worker.run {
                let client = try AvenClient.open(path: paths.databaseURL.path)
                started.signal()
                release.wait()
                return try client.listWorkspaces().count
            }
        }

        XCTAssertEqual(started.wait(timeout: .now() + 5), .success)
        operation.cancel()
        release.signal()
        let workspaceCount = try await operation.value
        XCTAssertGreaterThanOrEqual(workspaceCount, 1)
        print(
            "AVEN_IOS_WORKER_CANCELLATION_TEST status=pass " +
                "swift_task=cancelled rust_call=completed"
        )
    }

    func testApplicationSupportPathIsStableAndDurable() throws {
        let first = try ApplicationSupportPath.hostDatabaseURL()
        let second = try ApplicationSupportPath.hostDatabaseURL()

        XCTAssertEqual(first, second)
        XCTAssertEqual(first.lastPathComponent, "host-smoke.sqlite")
        XCTAssertEqual(first.deletingLastPathComponent().lastPathComponent, "AvenHostProof")
        XCTAssertTrue(
            FileManager.default.fileExists(
                atPath: first.deletingLastPathComponent().path
            )
        )
    }

    func testRetryableHTTPStatusReplaysOpaqueRequestUntilExhaustion() async throws {
        let state = ScriptedTransportState(mode: .httpStatus(503))
        let driver = SyncDriver(
            worker: RustWorker(label: "com.raine.aven.ios-proof.http-retry-test"),
            transport: scriptedTransport(state: state)
        )
        let paths = try ApplicationSupportPath.prepareSyncProof(reset: true)

        do {
            _ = try await driver.run(
                databasePath: paths.databaseURL.path,
                server: "https://sync.example.test",
                authToken: "test-token"
            )
            XCTFail("retry exhaustion must fail")
        } catch {}

        let requests = state.capturedRequests
        XCTAssertEqual(requests.count, 4)
        let first = try XCTUnwrap(requests.first)
        for request in requests.dropFirst() {
            XCTAssertEqual(request.httpMethod, first.httpMethod)
            XCTAssertEqual(request.url, first.url)
            XCTAssertEqual(request.httpBody, first.httpBody)
            XCTAssertEqual(
                request.value(forHTTPHeaderField: "Authorization"),
                "Bearer test-token"
            )
            XCTAssertEqual(request.timeoutInterval, first.timeoutInterval)
        }
        XCTAssertEqual(first.timeoutInterval, 10)
    }

    func testTransientTransportFailureUsesBoundedAttempts() async throws {
        let state = ScriptedTransportState(mode: .transportFailure)
        let driver = SyncDriver(
            worker: RustWorker(label: "com.raine.aven.ios-proof.transport-retry-test"),
            transport: scriptedTransport(state: state)
        )
        let paths = try ApplicationSupportPath.prepareSyncProof(reset: true)

        do {
            _ = try await driver.run(
                databasePath: paths.databaseURL.path,
                server: "https://sync.example.test",
                authToken: "test-token"
            )
            XCTFail("transport retry exhaustion must fail")
        } catch {}

        let requests = state.capturedRequests
        XCTAssertEqual(requests.count, 4)
        XCTAssertTrue(requests.dropFirst().allSatisfy { request in
            request.httpMethod == requests[0].httpMethod &&
                request.url == requests[0].url &&
                request.httpBody == requests[0].httpBody
        })
    }

    func testPreparedAttemptTimeoutCancelsStalledURLSessionTask() async throws {
        let state = ScriptedTransportState(mode: .stall)
        let transport = scriptedTransport(state: state)
        var prepared = try await preparedRequest()
        prepared.timeout = SyncRequestTimeout(
            attemptMs: 20,
            inactivityMs: 20
        )

        do {
            _ = try await transport.send(prepared)
            XCTFail("stalled request must time out")
        } catch URLSessionTransportError.attemptTimedOut {}

        try await Task.sleep(for: .milliseconds(100))
        XCTAssertEqual(state.capturedRequests.count, 1)
        XCTAssertEqual(state.stopCount, 1)
    }

    private func preparedRequest() async throws -> PreparedSyncRequest {
        let paths = try ApplicationSupportPath.prepareSyncProof(reset: true)
        let worker = RustWorker(label: "com.raine.aven.ios-proof.timeout-test")
        let session = try await worker.withClient(at: paths.databaseURL.path) { client in
            try client.startSyncSession(
                server: "https://sync.example.test",
                authToken: "test-token",
                pageBudget: nil
            )
        }
        return try await worker.run {
            try XCTUnwrap(session.prepareRequest())
        }
    }

    private func scriptedTransport(
        state: ScriptedTransportState
    ) -> URLSessionTransport {
        ScriptedURLProtocol.state = state
        let configuration = URLSessionConfiguration.ephemeral
        configuration.protocolClasses = [ScriptedURLProtocol.self]
        return URLSessionTransport(session: URLSession(configuration: configuration))
    }
}

private enum ScriptedTransportMode {
    case httpStatus(Int)
    case transportFailure
    case stall
}

private final class ScriptedTransportState: @unchecked Sendable {
    private let lock = NSLock()
    let mode: ScriptedTransportMode
    private var requests: [URLRequest] = []
    private var stops = 0

    init(mode: ScriptedTransportMode) {
        self.mode = mode
    }

    var capturedRequests: [URLRequest] {
        lock.withLock { requests }
    }

    var stopCount: Int {
        lock.withLock { stops }
    }

    func capture(_ request: URLRequest) {
        lock.withLock { requests.append(request) }
    }

    func recordStop() {
        lock.withLock { stops += 1 }
    }
}

private final class ScriptedURLProtocol: URLProtocol, @unchecked Sendable {
    nonisolated(unsafe) static var state = ScriptedTransportState(mode: .stall)

    override class func canInit(with _: URLRequest) -> Bool {
        true
    }

    override class func canonicalRequest(for request: URLRequest) -> URLRequest {
        request
    }

    override func startLoading() {
        let state = Self.state
        state.capture(request)
        switch state.mode {
        case let .httpStatus(status):
            let response = HTTPURLResponse(
                url: request.url!,
                statusCode: status,
                httpVersion: "HTTP/1.1",
                headerFields: ["Retry-After": "0"]
            )!
            client?.urlProtocol(
                self,
                didReceive: response,
                cacheStoragePolicy: .notAllowed
            )
            client?.urlProtocolDidFinishLoading(self)
        case .transportFailure:
            client?.urlProtocol(
                self,
                didFailWithError: URLError(.networkConnectionLost)
            )
        case .stall:
            break
        }
    }

    override func stopLoading() {
        Self.state.recordStop()
    }
}
