import AvenUniFFI
import Foundation

public enum IOSSyncProofFailure: Error, Equatable, Sendable {
    case missingConfiguration
    case invariant(String)
}

public struct IOSSyncSeedResult: Equatable, Sendable {
    public let malformedResponseAtomic: Bool
    public let cancellationFailRequestCount: Int
    public let cancellationStateAtomic: Bool
    public let freshSessionCompleted: Bool
    public let heartbeatTickCount: Int
    public let attachmentTransferCount: UInt64
}

public struct IOSSyncConflictResult: Equatable, Sendable {
    public let conflictVariantsTyped: Bool
    public let selectedRemoteVariant: Bool
    public let unresolvedConflictCount: Int
    public let attachmentTransferCount: UInt64
}

public struct IOSSyncProofConfiguration: Sendable {
    public let server: String
    public let authToken: String

    public static func fromEnvironment(
        environment: [String: String] = ProcessInfo.processInfo.environment
    ) throws -> IOSSyncProofConfiguration {
        guard let server = environment["AVEN_IOS_SYNC_SERVER"],
              !server.isEmpty,
              let authToken = environment["AVEN_IOS_SYNC_AUTH_TOKEN"],
              !authToken.isEmpty
        else {
            throw IOSSyncProofFailure.missingConfiguration
        }
        return IOSSyncProofConfiguration(server: server, authToken: authToken)
    }
}

private final class SyncHeartbeatCounter: @unchecked Sendable {
    private let lock = NSLock()
    private var ticks = 0

    var value: Int {
        lock.withLock { ticks }
    }

    func increment() {
        lock.withLock {
            ticks += 1
        }
    }
}

public struct IOSSyncProof: Sendable {
    private static let seedTitle = "iOS sync proof seed"
    private static let localTitle = "iOS sync proof local variant"
    private static let remoteTitle = "iOS sync proof host variant"
    private static let taskDescription = "Synthetic attachment-free sync data"
    private static let project = "ios-sync-proof"

    private let worker: RustWorker
    private let driver: SyncDriver

    public init(
        worker: RustWorker = RustWorker(label: "com.raine.aven.ios-proof.scenario")
    ) {
        self.worker = worker
        driver = SyncDriver(worker: worker)
    }

    @MainActor
    public func seed(
        configuration: IOSSyncProofConfiguration
    ) async throws -> IOSSyncSeedResult {
        let paths = try ApplicationSupportPath.prepareSyncProof(reset: true)
        let databasePath = paths.databaseURL.path
        let identity = try await worker.withClient(at: databasePath) { client in
            let workspace = try client.resolveWorkspace(nameOrKey: "default")
            let task = try client.createTask(
                workspaceId: workspace.id,
                input: CreateTask(
                    title: Self.seedTitle,
                    description: Self.taskDescription,
                    project: Self.project,
                    status: .todo,
                    priority: .high,
                    availableAt: nil,
                    dueOn: nil
                )
            )
            return SyncTaskIdentity(workspaceId: workspace.id, taskId: task.id)
        }

        let malformedResponseAtomic = try await proveMalformedResponseAtomicity(
            databasePath: databasePath,
            configuration: configuration,
            identity: identity
        )
        let taskBeforeCancellation = try await fetchTask(
            databasePath: databasePath,
            identity: identity
        )
        let heartbeatCounter = SyncHeartbeatCounter()
        let heartbeat = Task { @MainActor in
            while !Task.isCancelled {
                heartbeatCounter.increment()
                try? await Task.sleep(for: .milliseconds(1))
            }
        }
        await Task.yield()
        let initialTicks = heartbeatCounter.value
        let cancelled: CancelledRequestResult
        do {
            cancelled = try await driver.cancelOneDelayedRequest(
                databasePath: databasePath,
                server: configuration.server,
                authToken: configuration.authToken
            )
        } catch {
            heartbeat.cancel()
            throw error
        }
        let heartbeatTickCount = heartbeatCounter.value - initialTicks
        heartbeat.cancel()

        let taskAfterCancellation = try await fetchTask(
            databasePath: databasePath,
            identity: identity
        )
        let cancellationStateAtomic = taskAfterCancellation == taskBeforeCancellation &&
            cancelled.cursorAfter == cancelled.cursorBefore
        guard cancelled.failRequestCount == 1,
              cancellationStateAtomic,
              heartbeatTickCount > 0
        else {
            throw IOSSyncProofFailure.invariant(
                "cancelled transport request changed Rust state"
            )
        }

        let freshSummary = try await driver.run(
            databasePath: databasePath,
            server: configuration.server,
            authToken: configuration.authToken
        )
        guard freshSummary.complete,
              freshSummary.pushed > 0,
              zeroAttachmentTransfers(freshSummary)
        else {
            throw IOSSyncProofFailure.invariant(
                "fresh sync session did not complete the pending metadata push"
            )
        }

        return IOSSyncSeedResult(
            malformedResponseAtomic: malformedResponseAtomic,
            cancellationFailRequestCount: cancelled.failRequestCount,
            cancellationStateAtomic: cancellationStateAtomic,
            freshSessionCompleted: true,
            heartbeatTickCount: heartbeatTickCount,
            attachmentTransferCount: attachmentTransferCount(freshSummary)
        )
    }

    public func convergeConflict(
        configuration: IOSSyncProofConfiguration
    ) async throws -> IOSSyncConflictResult {
        let paths = try ApplicationSupportPath.prepareSyncProof(reset: false)
        let databasePath = paths.databaseURL.path
        let identity = try await worker.withClient(at: databasePath) { client in
            let workspace = try client.resolveWorkspace(nameOrKey: "default")
            let tasks = try client.listTasks(workspaceId: workspace.id)
            guard tasks.count == 1 else {
                throw IOSSyncProofFailure.invariant(
                    "iOS sync replica did not contain exactly one task"
                )
            }
            let task = tasks[0]
            guard task.title == Self.seedTitle,
                  task.description == Self.taskDescription,
                  task.projectKey == Self.project,
                  task.status == .todo,
                  task.priority == .high,
                  task.availableAt == nil,
                  task.dueOn == nil
            else {
                throw IOSSyncProofFailure.invariant(
                    "iOS sync seed fields changed before divergence"
                )
            }
            let updated = try client.updateTask(
                workspaceId: workspace.id,
                taskId: task.id,
                input: UpdateTask(
                    title: Self.localTitle,
                    description: nil,
                    project: nil,
                    status: nil,
                    priority: nil,
                    availableAt: .unchanged,
                    dueOn: .unchanged
                )
            )
            guard updated.changed, updated.task.title == Self.localTitle else {
                throw IOSSyncProofFailure.invariant(
                    "iOS divergent title update failed"
                )
            }
            return SyncTaskIdentity(workspaceId: workspace.id, taskId: task.id)
        }

        let conflictSummary = try await driver.run(
            databasePath: databasePath,
            server: configuration.server,
            authToken: configuration.authToken
        )
        let conflictVariantsTyped = try await worker.withClient(at: databasePath) { client in
            let summaries = try client.listConflicts(workspaceId: identity.workspaceId)
            let conflicts = try client.inspectConflicts(
                workspaceId: identity.workspaceId,
                taskId: identity.taskId
            )
            guard summaries.count == 1,
                  summaries[0].field == .title,
                  conflicts.count == 1,
                  conflicts[0].field == .title,
                  conflicts[0].localValue == Self.localTitle,
                  conflicts[0].remoteValue == Self.remoteTitle
            else {
                throw IOSSyncProofFailure.invariant(
                    "typed title conflict variants did not match"
                )
            }
            let resolved = try client.resolveConflict(
                workspaceId: identity.workspaceId,
                taskId: identity.taskId,
                field: .title,
                choice: .remote
            )
            guard resolved.title == Self.remoteTitle else {
                throw IOSSyncProofFailure.invariant(
                    "remote conflict choice selected the wrong title"
                )
            }
            return true
        }

        let resolutionSummary = try await driver.run(
            databasePath: databasePath,
            server: configuration.server,
            authToken: configuration.authToken
        )
        let finalSummary = try await driver.run(
            databasePath: databasePath,
            server: configuration.server,
            authToken: configuration.authToken
        )
        let finalState = try await worker.withClient(at: databasePath) { client in
            let task = try client.fetchTask(
                workspaceId: identity.workspaceId,
                taskId: identity.taskId
            )
            let conflicts = try client.listConflicts(workspaceId: identity.workspaceId)
            return (task, conflicts.count)
        }
        guard finalState.0.title == Self.remoteTitle,
              finalState.0.description == Self.taskDescription,
              finalState.0.projectKey == Self.project,
              finalState.0.status == .todo,
              finalState.0.priority == .high,
              finalState.0.availableAt == nil,
              finalState.0.dueOn == nil,
              finalState.1 == 0,
              conflictSummary.complete,
              resolutionSummary.complete,
              finalSummary.complete,
              zeroAttachmentTransfers(conflictSummary),
              zeroAttachmentTransfers(resolutionSummary),
              zeroAttachmentTransfers(finalSummary)
        else {
            throw IOSSyncProofFailure.invariant(
                "iOS replica did not converge after conflict resolution"
            )
        }

        return IOSSyncConflictResult(
            conflictVariantsTyped: conflictVariantsTyped,
            selectedRemoteVariant: true,
            unresolvedConflictCount: finalState.1,
            attachmentTransferCount: attachmentTransferCount(conflictSummary) +
                attachmentTransferCount(resolutionSummary) +
                attachmentTransferCount(finalSummary)
        )
    }

    private func proveMalformedResponseAtomicity(
        databasePath: String,
        configuration: IOSSyncProofConfiguration,
        identity: SyncTaskIdentity
    ) async throws -> Bool {
        let session = try await worker.withClient(at: databasePath) { client in
            try client.startSyncSession(
                server: configuration.server,
                authToken: configuration.authToken,
                pageBudget: nil
            )
        }
        let taskBefore = try await fetchTask(
            databasePath: databasePath,
            identity: identity
        )
        let summaryBefore = try await worker.run { try session.summary() }
        guard let prepared = try await worker.run({ try session.prepareRequest() }) else {
            throw IOSSyncProofFailure.invariant(
                "malformed response proof had no request"
            )
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
            throw IOSSyncProofFailure.invariant("malformed response was accepted")
        }

        let taskAfter = try await fetchTask(
            databasePath: databasePath,
            identity: identity
        )
        let summaryAfter = try await worker.run { try session.summary() }
        guard let retry = try await worker.run({ try session.prepareRequest() }),
              taskAfter == taskBefore,
              summaryAfter.cursor == summaryBefore.cursor,
              retry.method == prepared.method,
              retry.url == prepared.url,
              retry.headers == prepared.headers,
              retry.body == prepared.body
        else {
            throw IOSSyncProofFailure.invariant(
                "malformed response changed task, cursor, or outstanding request"
            )
        }
        try await worker.run {
            try session.failRequest(
                context: retry.context,
                message: "expected malformed response proof"
            )
        }
        return true
    }

    private func fetchTask(
        databasePath: String,
        identity: SyncTaskIdentity
    ) async throws -> TaskRecord {
        try await worker.withClient(at: databasePath) { client in
            try client.fetchTask(
                workspaceId: identity.workspaceId,
                taskId: identity.taskId
            )
        }
    }
}

private struct SyncTaskIdentity: Sendable {
    let workspaceId: String
    let taskId: String
}

private func zeroAttachmentTransfers(_ summary: SyncSessionSummary) -> Bool {
    attachmentTransferCount(summary) == 0 &&
        summary.blobUploadRemaining == 0 &&
        summary.blobUploadRemainingBytes == 0 &&
        summary.blobDownloadRemaining == 0 &&
        summary.blobDownloadRemainingBytes == 0
}

private func attachmentTransferCount(_ summary: SyncSessionSummary) -> UInt64 {
    summary.blobUploaded + summary.blobDownloaded
}
