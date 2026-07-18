import AvenUniFFI
import Foundation

public enum ProofFailure: Error, Equatable, Sendable {
    case invariant(String)
    case unexpectedErrorCode(ErrorCode)
}

public enum ProofValueMapping {
    public static func statusName(_ status: TaskStatus) -> String {
        switch status {
        case .inbox: "inbox"
        case .backlog: "backlog"
        case .todo: "todo"
        case .active: "active"
        case .done: "done"
        case .canceled: "canceled"
        }
    }

    public static func priorityName(_ priority: TaskPriority) -> String {
        switch priority {
        case .none: "none"
        case .low: "low"
        case .medium: "medium"
        case .high: "high"
        case .urgent: "urgent"
        }
    }
}

public struct LocalProofResult: Equatable, Sendable {
    public let workspaceCount: Int
    public let taskCount: Int
    public let statusName: String
    public let priorityName: String
    public let requestByteCount: Int
    public let validationErrorMatched: Bool
    public let notFoundErrorMatched: Bool

    public init(
        workspaceCount: Int,
        taskCount: Int,
        statusName: String,
        priorityName: String,
        requestByteCount: Int,
        validationErrorMatched: Bool,
        notFoundErrorMatched: Bool
    ) {
        self.workspaceCount = workspaceCount
        self.taskCount = taskCount
        self.statusName = statusName
        self.priorityName = priorityName
        self.requestByteCount = requestByteCount
        self.validationErrorMatched = validationErrorMatched
        self.notFoundErrorMatched = notFoundErrorMatched
    }
}

public struct LocalProof: Sendable {
    private struct InitialState: Sendable {
        let workspaceCount: Int
        let workspaceId: String
        let taskId: String
        let requestByteCount: Int
        let validationErrorMatched: Bool
        let notFoundErrorMatched: Bool
    }

    private let worker: RustWorker

    public init(worker: RustWorker = RustWorker()) {
        self.worker = worker
    }

    public func run(databasePath: String) async throws -> LocalProofResult {
        let initial = try await worker.withClient(at: databasePath) { client in
            let workspaces = try client.listWorkspaces()
            let workspace = try client.resolveWorkspace(nameOrKey: "default")
            guard workspaces.contains(where: { $0.id == workspace.id }) else {
                throw ProofFailure.invariant("resolved workspace was not listed")
            }

            let created = try client.createTask(
                workspaceId: workspace.id,
                input: CreateTask(
                    title: "Swift local proof",
                    description: "Rust owns the local task data",
                    project: "swift-proof",
                    status: .todo,
                    priority: .high,
                    availableAt: nil,
                    dueOn: "2026-07-20"
                )
            )
            guard created.availableAt == nil, created.dueOn == "2026-07-20" else {
                throw ProofFailure.invariant("optional date mapping failed")
            }

            let updated = try client.updateTask(
                workspaceId: workspace.id,
                taskId: created.id,
                input: UpdateTask(
                    title: nil,
                    description: nil,
                    project: nil,
                    status: .active,
                    priority: nil,
                    availableAt: .unchanged,
                    dueOn: .unchanged
                )
            )
            guard updated.changed, updated.task.status == .active else {
                throw ProofFailure.invariant("scalar task update failed")
            }
            guard try client.fetchTask(
                workspaceId: workspace.id,
                taskId: created.id
            ) == updated.task else {
                throw ProofFailure.invariant("task fetch did not match update")
            }
            guard try client.listTasks(workspaceId: workspace.id).count == 1 else {
                throw ProofFailure.invariant("task list did not contain one task")
            }

            let session = try client.startSyncSession(
                server: "https://sync.invalid",
                authToken: nil,
                pageBudget: nil
            )
            guard let request = try session.prepareRequest(), !request.body.isEmpty else {
                throw ProofFailure.invariant("sync request did not transfer bytes")
            }
            try session.failRequest(
                context: request.context,
                message: "transport is outside the local proof"
            )

            let validationErrorMatched = try matchErrorCode(.validation) {
                _ = try client.listTasks(workspaceId: "bad")
            }
            let notFoundErrorMatched = try matchErrorCode(.notFound) {
                _ = try client.fetchTask(
                    workspaceId: workspace.id,
                    taskId: "0000000000000000"
                )
            }

            return InitialState(
                workspaceCount: workspaces.count,
                workspaceId: workspace.id,
                taskId: created.id,
                requestByteCount: request.body.count,
                validationErrorMatched: validationErrorMatched,
                notFoundErrorMatched: notFoundErrorMatched
            )
        }

        return try await worker.withClient(at: databasePath) { client in
            let workspace = try client.resolveWorkspace(nameOrKey: "default")
            guard workspace.id == initial.workspaceId else {
                throw ProofFailure.invariant("workspace changed after reopen")
            }
            let tasks = try client.listTasks(workspaceId: workspace.id)
            let task = try client.fetchTask(
                workspaceId: workspace.id,
                taskId: initial.taskId
            )
            guard tasks.count == 1, tasks[0] == task else {
                throw ProofFailure.invariant("reopen changed task persistence")
            }

            let statusName = ProofValueMapping.statusName(task.status)
            let priorityName = ProofValueMapping.priorityName(task.priority)
            guard statusName == "active", priorityName == "high" else {
                throw ProofFailure.invariant("typed enum mapping failed")
            }

            return LocalProofResult(
                workspaceCount: initial.workspaceCount,
                taskCount: tasks.count,
                statusName: statusName,
                priorityName: priorityName,
                requestByteCount: initial.requestByteCount,
                validationErrorMatched: initial.validationErrorMatched,
                notFoundErrorMatched: initial.notFoundErrorMatched
            )
        }
    }
}

private func matchErrorCode(
    _ expected: ErrorCode,
    operation: () throws -> Void
) throws -> Bool {
    do {
        try operation()
        throw ProofFailure.invariant("expected a typed facade error")
    } catch let AvenError.Failure(code, message) {
        guard !message.isEmpty else {
            throw ProofFailure.invariant("typed facade error had no message")
        }
        guard code == expected else {
            throw ProofFailure.unexpectedErrorCode(code)
        }
        return true
    }
}
