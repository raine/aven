import AvenUniFFI
import Foundation

public enum PersistenceProofFailure: Error, Equatable, Sendable {
    case invariant(String)
    case unexpectedErrorCode(ErrorCode)
}

public struct PersistenceProofResult: Equatable, Sendable {
    public let workspaceCount: Int
    public let taskCount: Int
    public let statusNames: [String]
    public let priorityNames: [String]
    public let validationErrorCount: Int
    public let notFoundErrorCount: Int
    public let workspaceMismatchMatched: Bool
    public let walObservedBeforeRelease: Bool
    public let shmObservedBeforeRelease: Bool
    public let walObservedAfterReopen: Bool
    public let shmObservedAfterReopen: Bool
    public let dataProtectionConfigured: Bool
    public let storagePathCount: Int
}

public struct PersistenceProof: Sendable {
    private struct InitialState: Sendable {
        let workspaceCount: Int
        let workspaceId: String
        let taskId: String
        let statusNames: [String]
        let priorityNames: [String]
        let validationErrorCount: Int
        let notFoundErrorCount: Int
        let workspaceMismatchMatched: Bool
        let walObserved: Bool
        let shmObserved: Bool
        let storagePaths: [String]
    }

    private let worker: RustWorker

    public init(worker: RustWorker = RustWorker()) {
        self.worker = worker
    }

    public func run() async throws -> PersistenceProofResult {
        let paths = try ApplicationSupportPath.preparePersistenceProof()
        let databasePath = paths.databaseURL.path
        let initial = try await worker.withClient(at: databasePath) { client in
            let storage = try client.initializeStorage()
            let storagePaths = [
                storage.root,
                storage.objects,
                storage.staging,
                storage.trash,
                storage.previews,
            ]
            try validateStoragePaths(
                storagePaths,
                root: storage.root,
                objects: storage.objects,
                staging: storage.staging,
                sandboxDirectory: paths.directoryURL.path
            )

            let workspaces = try client.listWorkspaces()
            let workspace = try client.resolveWorkspace(nameOrKey: "default")
            guard workspaces.contains(where: { $0.id == workspace.id }) else {
                throw PersistenceProofFailure.invariant(
                    "resolved workspace was not listed"
                )
            }

            let created = try client.createTask(
                workspaceId: workspace.id,
                input: CreateTask(
                    title: "iOS persistence proof",
                    description: "Synthetic attachment-free task data",
                    project: "ios-proof",
                    status: .inbox,
                    priority: .none,
                    availableAt: nil,
                    dueOn: nil
                )
            )
            guard created.availableAt == nil, created.dueOn == nil else {
                throw PersistenceProofFailure.invariant(
                    "absent optional dates did not map"
                )
            }

            var statusNames = [statusName(created.status)]
            var priorityNames = [priorityName(created.priority)]
            let setDates = try client.updateTask(
                workspaceId: workspace.id,
                taskId: created.id,
                input: UpdateTask(
                    title: "Updated iOS persistence proof",
                    description: "Deterministic synthetic task data",
                    project: "ios-persistence",
                    status: .backlog,
                    priority: .low,
                    availableAt: .set(value: "2026-07-20T10:30:00Z"),
                    dueOn: .set(value: "2026-07-21")
                )
            )
            guard setDates.changed,
                  setDates.task.availableAt == "2026-07-20T10:30:00Z",
                  setDates.task.dueOn == "2026-07-21"
            else {
                throw PersistenceProofFailure.invariant(
                    "set optional dates did not map"
                )
            }
            statusNames.append(statusName(setDates.task.status))
            priorityNames.append(priorityName(setDates.task.priority))

            let clearDates = try update(
                client: client,
                workspaceId: workspace.id,
                taskId: created.id,
                status: .todo,
                priority: .medium,
                availableAt: .clear,
                dueOn: .clear
            )
            guard clearDates.task.availableAt == nil,
                  clearDates.task.dueOn == nil
            else {
                throw PersistenceProofFailure.invariant(
                    "cleared optional dates did not map"
                )
            }
            statusNames.append(statusName(clearDates.task.status))
            priorityNames.append(priorityName(clearDates.task.priority))

            let active = try update(
                client: client,
                workspaceId: workspace.id,
                taskId: created.id,
                status: .active,
                priority: .high
            )
            statusNames.append(statusName(active.task.status))
            priorityNames.append(priorityName(active.task.priority))

            let done = try update(
                client: client,
                workspaceId: workspace.id,
                taskId: created.id,
                status: .done,
                priority: .urgent
            )
            statusNames.append(statusName(done.task.status))
            priorityNames.append(priorityName(done.task.priority))

            let canceled = try update(
                client: client,
                workspaceId: workspace.id,
                taskId: created.id,
                status: .canceled,
                priority: nil
            )
            statusNames.append(statusName(canceled.task.status))

            guard statusNames == [
                "inbox", "backlog", "todo", "active", "done", "canceled",
            ], priorityNames == [
                "none", "low", "medium", "high", "urgent",
            ] else {
                throw PersistenceProofFailure.invariant(
                    "typed enum coverage was incomplete"
                )
            }
            guard try client.fetchTask(
                workspaceId: workspace.id,
                taskId: created.id
            ) == canceled.task else {
                throw PersistenceProofFailure.invariant(
                    "task fetch did not match the final update"
                )
            }
            guard try client.listTasks(workspaceId: workspace.id) == [canceled.task] else {
                throw PersistenceProofFailure.invariant(
                    "task list did not contain exactly one task"
                )
            }

            var validationErrorCount = 0
            if try matchErrorCode(.validation, operation: {
                _ = try client.listTasks(workspaceId: "bad")
            }) {
                validationErrorCount += 1
            }
            if try matchErrorCode(.validation, operation: {
                _ = try client.fetchTask(
                    workspaceId: workspace.id,
                    taskId: "bad"
                )
            }) {
                validationErrorCount += 1
            }
            let notFoundErrorCount = try matchErrorCode(.notFound, operation: {
                _ = try client.fetchTask(
                    workspaceId: workspace.id,
                    taskId: "0000000000000000"
                )
            }) ? 1 : 0
            let workspaceMismatchMatched = try matchErrorCode(
                .notFound,
                operation: {
                    _ = try client.fetchTask(
                        workspaceId: "1111111111111111",
                        taskId: created.id
                    )
                }
            )

            return InitialState(
                workspaceCount: workspaces.count,
                workspaceId: workspace.id,
                taskId: created.id,
                statusNames: statusNames,
                priorityNames: priorityNames,
                validationErrorCount: validationErrorCount,
                notFoundErrorCount: notFoundErrorCount,
                workspaceMismatchMatched: workspaceMismatchMatched,
                walObserved: FileManager.default.fileExists(
                    atPath: databasePath + "-wal"
                ),
                shmObserved: FileManager.default.fileExists(
                    atPath: databasePath + "-shm"
                ),
                storagePaths: storagePaths
            )
        }

        let protectedURLs = [paths.directoryURL, paths.databaseURL] +
            initial.storagePaths.map { URL(fileURLWithPath: $0) }
        try ApplicationSupportPath.applyDataProtection(to: protectedURLs)
        let dataProtectionConfigured =
            ApplicationSupportPath.dataProtectionClass ==
            .completeUntilFirstUserAuthentication

        return try await worker.withClient(at: databasePath) { client in
            let workspace = try client.resolveWorkspace(nameOrKey: "default")
            guard workspace.id == initial.workspaceId else {
                throw PersistenceProofFailure.invariant(
                    "workspace changed after reopen"
                )
            }
            let tasks = try client.listTasks(workspaceId: workspace.id)
            let task = try client.fetchTask(
                workspaceId: workspace.id,
                taskId: initial.taskId
            )
            guard tasks == [task],
                  task.title == "Updated iOS persistence proof",
                  task.description == "Deterministic synthetic task data",
                  task.projectKey == "ios-persistence",
                  task.status == .canceled,
                  task.priority == .urgent,
                  task.availableAt == nil,
                  task.dueOn == nil
            else {
                throw PersistenceProofFailure.invariant(
                    "reopen changed persisted task fields"
                )
            }

            return PersistenceProofResult(
                workspaceCount: initial.workspaceCount,
                taskCount: tasks.count,
                statusNames: initial.statusNames,
                priorityNames: initial.priorityNames,
                validationErrorCount: initial.validationErrorCount,
                notFoundErrorCount: initial.notFoundErrorCount,
                workspaceMismatchMatched: initial.workspaceMismatchMatched,
                walObservedBeforeRelease: initial.walObserved,
                shmObservedBeforeRelease: initial.shmObserved,
                walObservedAfterReopen: FileManager.default.fileExists(
                    atPath: databasePath + "-wal"
                ),
                shmObservedAfterReopen: FileManager.default.fileExists(
                    atPath: databasePath + "-shm"
                ),
                dataProtectionConfigured: dataProtectionConfigured,
                storagePathCount: initial.storagePaths.count
            )
        }
    }
}

private func update(
    client: AvenClient,
    workspaceId: String,
    taskId: String,
    status: TaskStatus,
    priority: TaskPriority?,
    availableAt: OptionalDateUpdate = .unchanged,
    dueOn: OptionalDateUpdate = .unchanged
) throws -> TaskUpdateResult {
    let result = try client.updateTask(
        workspaceId: workspaceId,
        taskId: taskId,
        input: UpdateTask(
            title: nil,
            description: nil,
            project: nil,
            status: status,
            priority: priority,
            availableAt: availableAt,
            dueOn: dueOn
        )
    )
    guard result.changed else {
        throw PersistenceProofFailure.invariant("task update reported no change")
    }
    return result
}

private func validateStoragePaths(
    _ paths: [String],
    root: String,
    objects: String,
    staging: String,
    sandboxDirectory: String
) throws {
    guard staging == objects else {
        throw PersistenceProofFailure.invariant(
            "staging did not use the atomic object directory"
        )
    }
    let sandboxPrefix = URL(fileURLWithPath: sandboxDirectory)
        .standardizedFileURL.path + "/"
    for path in paths {
        let url = URL(fileURLWithPath: path).standardizedFileURL
        guard url.path.hasPrefix("/"),
              url.path.hasPrefix(sandboxPrefix),
              FileManager.default.fileExists(atPath: url.path)
        else {
            throw PersistenceProofFailure.invariant(
                "storage path escaped Application Support"
            )
        }
    }
    guard root.hasSuffix("persistence.sqlite.blobs") else {
        throw PersistenceProofFailure.invariant(
            "storage root was not derived from the database path"
        )
    }
}

private func matchErrorCode(
    _ expected: ErrorCode,
    operation: () throws -> Void
) throws -> Bool {
    do {
        try operation()
        throw PersistenceProofFailure.invariant("expected a typed facade error")
    } catch let AvenError.Failure(code, message) {
        guard !message.isEmpty else {
            throw PersistenceProofFailure.invariant(
                "typed facade error had no message"
            )
        }
        guard code == expected else {
            throw PersistenceProofFailure.unexpectedErrorCode(code)
        }
        return true
    }
}

private func statusName(_ status: TaskStatus) -> String {
    switch status {
    case .inbox: "inbox"
    case .backlog: "backlog"
    case .todo: "todo"
    case .active: "active"
    case .done: "done"
    case .canceled: "canceled"
    }
}

private func priorityName(_ priority: TaskPriority) -> String {
    switch priority {
    case .none: "none"
    case .low: "low"
    case .medium: "medium"
    case .high: "high"
    case .urgent: "urgent"
    }
}
