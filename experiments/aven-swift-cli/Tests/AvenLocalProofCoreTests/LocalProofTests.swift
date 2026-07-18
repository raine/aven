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
