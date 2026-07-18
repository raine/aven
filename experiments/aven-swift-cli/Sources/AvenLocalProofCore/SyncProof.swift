import AvenUniFFI
import Foundation

public struct SyncProofResult: Equatable, Sendable {
    public let malformedResponseAtomic: Bool
    public let authorizationAccepted: Bool
    public let conflictVariantsTyped: Bool
    public let replicasConverged: Bool
    public let unresolvedConflictCount: Int

    public init(
        malformedResponseAtomic: Bool,
        authorizationAccepted: Bool,
        conflictVariantsTyped: Bool,
        replicasConverged: Bool,
        unresolvedConflictCount: Int
    ) {
        self.malformedResponseAtomic = malformedResponseAtomic
        self.authorizationAccepted = authorizationAccepted
        self.conflictVariantsTyped = conflictVariantsTyped
        self.replicasConverged = replicasConverged
        self.unresolvedConflictCount = unresolvedConflictCount
    }
}

public struct SyncProof: Sendable {
    private struct TaskIdentity: Sendable {
        let workspaceId: String
        let taskId: String
    }

    private let worker: RustWorker
    private let syncDriver: SyncDriver

    public init(
        worker: RustWorker = RustWorker(label: "dev.aven.swift-sync-proof.scenario"),
        transport: URLSessionTransport = URLSessionTransport()
    ) {
        self.worker = worker
        syncDriver = SyncDriver(worker: worker, transport: transport)
    }

    public func run(
        directory: URL,
        server: String,
        authToken: String
    ) async throws -> SyncProofResult {
        let firstDatabase = directory.appendingPathComponent("replica-a.sqlite").path
        let secondDatabase = directory.appendingPathComponent("replica-b.sqlite").path
        let firstTitle = "swift-proof-private-first"
        let secondTitle = "swift-proof-private-second"

        let identity = try await worker.withClient(at: firstDatabase) { client in
            let workspace = try client.resolveWorkspace(nameOrKey: "default")
            let task = try client.createTask(
                workspaceId: workspace.id,
                input: CreateTask(
                    title: "swift-proof-private-seed",
                    description: "swift-proof-private-body",
                    project: "swift-proof-private-project",
                    status: .todo,
                    priority: .high,
                    availableAt: nil,
                    dueOn: nil
                )
            )
            return TaskIdentity(workspaceId: workspace.id, taskId: task.id)
        }

        let malformedResponseAtomic = try await proveMalformedResponseAtomicity(
            databasePath: firstDatabase,
            server: server,
            authToken: authToken,
            identity: identity
        )

        let firstPush = try await syncDriver.run(
            databasePath: firstDatabase,
            server: server,
            authToken: authToken
        )
        let secondPull = try await syncDriver.run(
            databasePath: secondDatabase,
            server: server,
            authToken: authToken
        )
        guard firstPush.pushed > 0, secondPull.pulled > 0 else {
            throw ProofFailure.invariant("initial round trip did not exchange changes")
        }

        try await updateTitle(
            databasePath: firstDatabase,
            identity: identity,
            title: firstTitle
        )
        try await updateTitle(
            databasePath: secondDatabase,
            identity: identity,
            title: secondTitle
        )

        _ = try await syncDriver.run(
            databasePath: firstDatabase,
            server: server,
            authToken: authToken
        )
        _ = try await syncDriver.run(
            databasePath: secondDatabase,
            server: server,
            authToken: authToken
        )

        let conflictVariantsTyped = try await worker.withClient(at: secondDatabase) { client in
            let summaries = try client.listConflicts(workspaceId: identity.workspaceId)
            let conflicts = try client.inspectConflicts(
                workspaceId: identity.workspaceId,
                taskId: identity.taskId
            )
            guard summaries.count == 1,
                  summaries[0].field == .title,
                  conflicts.count == 1,
                  conflicts[0].field == .title,
                  Set([conflicts[0].localValue, conflicts[0].remoteValue])
                  == Set([firstTitle, secondTitle])
            else {
                throw ProofFailure.invariant("typed title conflict variants did not match")
            }
            let resolved = try client.resolveConflict(
                workspaceId: identity.workspaceId,
                taskId: identity.taskId,
                field: .title,
                choice: .remote
            )
            guard resolved.title == firstTitle else {
                throw ProofFailure.invariant("remote conflict choice selected the wrong title")
            }
            return true
        }

        _ = try await syncDriver.run(
            databasePath: secondDatabase,
            server: server,
            authToken: authToken
        )
        _ = try await syncDriver.run(
            databasePath: firstDatabase,
            server: server,
            authToken: authToken
        )
        _ = try await syncDriver.run(
            databasePath: secondDatabase,
            server: server,
            authToken: authToken
        )

        let firstState = try await replicaState(
            databasePath: firstDatabase,
            identity: identity
        )
        let secondState = try await replicaState(
            databasePath: secondDatabase,
            identity: identity
        )
        let unresolvedConflictCount = firstState.conflictCount + secondState.conflictCount
        guard firstState.task == secondState.task, unresolvedConflictCount == 0 else {
            throw ProofFailure.invariant("replicas did not converge after resolution")
        }

        return SyncProofResult(
            malformedResponseAtomic: malformedResponseAtomic,
            authorizationAccepted: true,
            conflictVariantsTyped: conflictVariantsTyped,
            replicasConverged: true,
            unresolvedConflictCount: unresolvedConflictCount
        )
    }

    private func proveMalformedResponseAtomicity(
        databasePath: String,
        server: String,
        authToken: String,
        identity: TaskIdentity
    ) async throws -> Bool {
        let session = try await worker.withClient(at: databasePath) { client in
            try client.startSyncSession(
                server: server,
                authToken: authToken,
                pageBudget: nil
            )
        }
        let taskBefore = try await fetchTask(databasePath: databasePath, identity: identity)
        let summaryBefore = try await worker.run { try session.summary() }
        guard let prepared = try await worker.run({ try session.prepareRequest() }) else {
            throw ProofFailure.invariant("malformed response proof had no request")
        }

        var malformedRejected = false
        do {
            _ = try await worker.run {
                try session.acceptResponse(
                    context: prepared.context,
                    response: SyncHttpResponse(
                        status: 200,
                        headers: [],
                        body: Data([0x7B])
                    )
                )
            }
        } catch {
            malformedRejected = true
        }
        guard malformedRejected else {
            throw ProofFailure.invariant("malformed response was accepted")
        }

        let taskAfter = try await fetchTask(databasePath: databasePath, identity: identity)
        let summaryAfter = try await worker.run { try session.summary() }
        guard let retry = try await worker.run({ try session.prepareRequest() }),
              taskAfter == taskBefore,
              summaryAfter.cursor == summaryBefore.cursor,
              retry.method == prepared.method,
              retry.url == prepared.url,
              retry.headers == prepared.headers,
              retry.body == prepared.body
        else {
            throw ProofFailure.invariant("malformed response changed state or cursor")
        }
        try await worker.run {
            try session.failRequest(
                context: prepared.context,
                message: "expected malformed response proof"
            )
        }
        return true
    }

    private func updateTitle(
        databasePath: String,
        identity: TaskIdentity,
        title: String
    ) async throws {
        try await worker.withClient(at: databasePath) { client in
            let result = try client.updateTask(
                workspaceId: identity.workspaceId,
                taskId: identity.taskId,
                input: UpdateTask(
                    title: title,
                    description: nil,
                    project: nil,
                    status: nil,
                    priority: nil,
                    availableAt: .unchanged,
                    dueOn: .unchanged
                )
            )
            guard result.changed, result.task.title == title else {
                throw ProofFailure.invariant("replica title update failed")
            }
        }
    }

    private func fetchTask(
        databasePath: String,
        identity: TaskIdentity
    ) async throws -> TaskRecord {
        try await worker.withClient(at: databasePath) { client in
            try client.fetchTask(
                workspaceId: identity.workspaceId,
                taskId: identity.taskId
            )
        }
    }

    private func replicaState(
        databasePath: String,
        identity: TaskIdentity
    ) async throws -> (task: TaskRecord, conflictCount: Int) {
        try await worker.withClient(at: databasePath) { client in
            let task = try client.fetchTask(
                workspaceId: identity.workspaceId,
                taskId: identity.taskId
            )
            let conflicts = try client.listConflicts(workspaceId: identity.workspaceId)
            return (task, conflicts.count)
        }
    }
}
