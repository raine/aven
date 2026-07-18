import AvenLocalProofCore
import AvenUniFFI
import Foundation
import XCTest

final class LocalProofTests: XCTestCase {
    func testTypedMappingsCoverEveryStatusAndPriority() {
        XCTAssertEqual(
            TaskStatus.allProofValues.map(ProofValueMapping.statusName),
            ["inbox", "backlog", "todo", "active", "done", "canceled"]
        )
        XCTAssertEqual(
            TaskPriority.allProofValues.map(ProofValueMapping.priorityName),
            ["none", "low", "medium", "high", "urgent"]
        )
    }

    func testOpaqueClientSurvivesWorkerClosureAndDatabaseReopens() async throws {
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent("aven-swift-test-\(UUID().uuidString)")
        try FileManager.default.createDirectory(
            at: directory,
            withIntermediateDirectories: true
        )
        defer { try? FileManager.default.removeItem(at: directory) }
        let databasePath = directory.appendingPathComponent("local.sqlite").path
        let worker = RustWorker(label: "dev.aven.swift-local-proof.test")

        let created = try await worker.withClient(at: databasePath) { client in
            let workspace = try client.resolveWorkspace(nameOrKey: "default")
            let task = try client.createTask(
                workspaceId: workspace.id,
                input: CreateTask(
                    title: "Lifetime proof",
                    description: "",
                    project: "swift-proof",
                    status: .todo,
                    priority: .medium,
                    availableAt: nil,
                    dueOn: nil
                )
            )
            return TaskIdentity(workspaceId: workspace.id, taskId: task.id)
        }

        let reopenedTask = try await worker.withClient(at: databasePath) { client in
            try client.fetchTask(
                workspaceId: created.workspaceId,
                taskId: created.taskId
            )
        }
        XCTAssertEqual(reopenedTask.id, created.taskId)
        XCTAssertNil(reopenedTask.availableAt)
        XCTAssertNil(reopenedTask.dueOn)
    }

    func testLocalProofRunsFacadeCallsOffMainThread() async throws {
        let worker = RustWorker(label: "dev.aven.swift-local-proof.thread-test")
        let usedMainThread = try await worker.run { Thread.isMainThread }
        XCTAssertFalse(usedMainThread)
    }

    func testURLSessionTransportForwardsOpaqueRequestWithoutBlockingSiblingTask() async throws {
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent("aven-swift-transport-test-\(UUID().uuidString)")
        try FileManager.default.createDirectory(
            at: directory,
            withIntermediateDirectories: true
        )
        defer { try? FileManager.default.removeItem(at: directory) }
        let worker = RustWorker(label: "dev.aven.swift-transport-test.rust")
        let prepared = try await worker.withClient(
            at: directory.appendingPathComponent("transport.sqlite").path
        ) { client in
            let workspace = try client.resolveWorkspace(nameOrKey: "default")
            _ = try client.createTask(
                workspaceId: workspace.id,
                input: CreateTask(
                    title: "opaque transport fixture",
                    description: "",
                    project: "swift-proof",
                    status: .todo,
                    priority: .medium,
                    availableAt: nil,
                    dueOn: nil
                )
            )
            let session = try client.startSyncSession(
                server: "https://sync.invalid",
                authToken: "private-token",
                pageBudget: nil
            )
            guard let request = try session.prepareRequest() else {
                throw ProofFailure.invariant("transport test had no request")
            }
            return request
        }

        let state = BlockingURLProtocol.state
        state.reset()
        let configuration = URLSessionConfiguration.ephemeral
        configuration.protocolClasses = [BlockingURLProtocol.self]
        let session = URLSession(configuration: configuration)
        defer { session.invalidateAndCancel() }
        let transport = URLSessionTransport(session: session)
        let sendTask = Task { try await transport.send(prepared) }

        XCTAssertTrue(state.waitUntilStarted(timeout: 2))
        let siblingTask = Task { 42 }
        let siblingValue = await siblingTask.value
        XCTAssertEqual(siblingValue, 42)
        state.releaseResponse()
        let response = try await sendTask.value

        let captured = try XCTUnwrap(state.capturedRequest())
        XCTAssertEqual(captured.method, prepared.method)
        XCTAssertEqual(captured.url, prepared.url)
        XCTAssertEqual(captured.body, prepared.body)
        XCTAssertEqual(captured.authorization, "Bearer private-token")
        XCTAssertEqual(response.status, 201)
        XCTAssertEqual(response.body, Data([0x01, 0x02, 0x03]))
        XCTAssertEqual(
            response.headers,
            [SyncHttpHeader(name: "content-type", value: "application/octet-stream")]
        )
    }
}

private final class BlockingURLProtocol: URLProtocol, @unchecked Sendable {
    static let state = BlockingURLProtocolState()

    override class func canInit(with _: URLRequest) -> Bool {
        true
    }

    override class func canonicalRequest(for request: URLRequest) -> URLRequest {
        request
    }

    override func startLoading() {
        Self.state.record(request)
        Self.state.waitForRelease()
        guard let url = request.url,
              let response = HTTPURLResponse(
                  url: url,
                  statusCode: 201,
                  httpVersion: nil,
                  headerFields: [
                      "Content-Type": "application/octet-stream",
                      "Content-Encoding": "gzip",
                      "Content-Length": "99",
                  ]
              )
        else {
            client?.urlProtocol(self, didFailWithError: URLError(.badServerResponse))
            return
        }
        client?.urlProtocol(self, didReceive: response, cacheStoragePolicy: .notAllowed)
        client?.urlProtocol(self, didLoad: Data([0x01, 0x02, 0x03]))
        client?.urlProtocolDidFinishLoading(self)
    }

    override func stopLoading() {
        Self.state.releaseResponse()
    }
}

private final class BlockingURLProtocolState: @unchecked Sendable {
    struct CapturedRequest: Sendable {
        let method: String?
        let url: String?
        let body: Data?
        let authorization: String?
    }

    private let lock = NSLock()
    private var captured: CapturedRequest?
    private var started = DispatchSemaphore(value: 0)
    private var release = DispatchSemaphore(value: 0)

    func reset() {
        lock.lock()
        captured = nil
        started = DispatchSemaphore(value: 0)
        release = DispatchSemaphore(value: 0)
        lock.unlock()
    }

    func record(_ request: URLRequest) {
        lock.lock()
        captured = CapturedRequest(
            method: request.httpMethod,
            url: request.url?.absoluteString,
            body: request.httpBody ?? readBody(from: request.httpBodyStream),
            authorization: request.value(forHTTPHeaderField: "Authorization")
        )
        lock.unlock()
        started.signal()
    }

    func waitUntilStarted(timeout: TimeInterval) -> Bool {
        started.wait(timeout: .now() + timeout) == .success
    }

    func waitForRelease() {
        release.wait()
    }

    func releaseResponse() {
        release.signal()
    }

    func capturedRequest() -> CapturedRequest? {
        lock.lock()
        defer { lock.unlock() }
        return captured
    }

    private func readBody(from stream: InputStream?) -> Data? {
        guard let stream else { return nil }
        stream.open()
        defer { stream.close() }
        var data = Data()
        var buffer = [UInt8](repeating: 0, count: 4096)
        while stream.hasBytesAvailable {
            let count = stream.read(&buffer, maxLength: buffer.count)
            guard count > 0 else { break }
            data.append(buffer, count: count)
        }
        return data
    }
}

private struct TaskIdentity: Sendable {
    let workspaceId: String
    let taskId: String
}

private extension TaskStatus {
    static let allProofValues: [TaskStatus] = [
        .inbox, .backlog, .todo, .active, .done, .canceled,
    ]
}

private extension TaskPriority {
    static let allProofValues: [TaskPriority] = [
        .none, .low, .medium, .high, .urgent,
    ]
}
