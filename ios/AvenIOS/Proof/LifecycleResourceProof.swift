import AvenUniFFI
import Darwin
import Foundation
import UIKit

public enum LifecycleResourceProofFailure: Error, Equatable, Sendable {
    case invariant(String)
    case memoryReadFailed(kern_return_t)
}

public struct ResourceProofResult: Equatable, Sendable {
    public let taskCount: Int
    public let databaseBytes: UInt64
    public let syncRequestBytes: Int
    public let syncResponseBytes: Int
    public let lifetimeIterations: Int
    public let latencySampleCount: Int
    public let coldOpenMicroseconds: UInt64
    public let warmOpenMedianMicroseconds: UInt64
    public let warmOpenMaximumMicroseconds: UInt64
    public let taskOperationMedianMicroseconds: UInt64
    public let taskOperationMaximumMicroseconds: UInt64
    public let syncMedianMicroseconds: UInt64
    public let syncMaximumMicroseconds: UInt64
    public let lifetimeMedianMicroseconds: UInt64
    public let lifetimeMaximumMicroseconds: UInt64
    public let baselineResidentBytes: UInt64
    public let initializedResidentBytes: UInt64
    public let peakResidentBytes: UInt64
    public let postOperationResidentBytes: UInt64
    public let lifetimeMemoryFirstBytes: UInt64
    public let lifetimeMemoryMedianBytes: UInt64
    public let lifetimeMemoryMaximumBytes: UInt64
    public let lifetimeMemoryLastBytes: UInt64
    public let monotonicGrowthObserved: Bool
    public let heartbeatTickCount: Int
    public let thermalState: String
}

public struct BackgroundLocalResult: Equatable, Sendable {
    public let behavior: String
    public let databaseReopened: Bool
}

public struct LockedDeviceResult: Equatable, Sendable {
    public let protectedDataUnavailable: Bool
    public let databaseAccessible: Bool
    public let walAccessible: Bool
    public let shmAccessible: Bool
    public let storageRootAccessible: Bool
    public let protectionMatched: Bool
}

private final class LockedFlag: @unchecked Sendable {
    private let lock = NSLock()
    private var storedValue = false

    var value: Bool {
        lock.withLock { storedValue }
    }

    func set() {
        lock.withLock { storedValue = true }
    }
}

private final class ProofHeartbeat: @unchecked Sendable {
    private let lock = NSLock()
    private var ticks = 0

    var value: Int {
        lock.withLock { ticks }
    }

    func increment() {
        lock.withLock { ticks += 1 }
    }
}

public struct LifecycleResourceProof: Sendable {
    public static let taskCount = 100
    public static let latencySampleCount = 21
    public static let lifetimeIterations = 30

    private let worker: RustWorker
    private let driver: SyncDriver
    private let clock = ContinuousClock()

    public init(
        worker: RustWorker = RustWorker(label: "com.raine.aven.ios-proof.lifecycle")
    ) {
        self.worker = worker
        driver = SyncDriver(worker: worker)
    }

    @MainActor
    public func measureResources(
        configuration: IOSSyncProofConfiguration
    ) async throws -> ResourceProofResult {
        let paths = try ApplicationSupportPath.prepareLifecycleProof(reset: true)
        let databasePath = paths.databaseURL.path
        let heartbeatCounter = ProofHeartbeat()
        let heartbeat = Task { @MainActor in
            while !Task.isCancelled {
                heartbeatCounter.increment()
                try? await Task.sleep(for: .milliseconds(1))
            }
        }
        await Task.yield()
        let initialHeartbeat = heartbeatCounter.value

        do {
            let baselineResidentBytes = try residentMemoryBytes()
            let coldOpenMicroseconds = try await measureMicroseconds {
                try await worker.withClient(at: databasePath) { client in
                    _ = try client.resolveWorkspace(nameOrKey: "default")
                }
            }
            let initializedResidentBytes = try residentMemoryBytes()
            var peakResidentBytes = max(
                baselineResidentBytes,
                initializedResidentBytes
            )

            var warmOpenSamples = [UInt64]()
            for _ in 0 ..< Self.latencySampleCount {
                try await warmOpenSamples.append(measureMicroseconds {
                    try await worker.withClient(at: databasePath) { client in
                        _ = try client.listWorkspaces()
                    }
                })
                peakResidentBytes = try max(peakResidentBytes, residentMemoryBytes())
            }

            let taskIdentity = try await worker.withClient(at: databasePath) { client in
                let workspace = try client.resolveWorkspace(nameOrKey: "default")
                var firstTaskId = ""
                for index in 0 ..< Self.taskCount {
                    let task = try client.createTask(
                        workspaceId: workspace.id,
                        input: CreateTask(
                            title: "Lifecycle resource proof \(index)",
                            description: "Synthetic attachment-free measurement data",
                            project: "ios-lifecycle-proof",
                            status: .todo,
                            priority: .medium,
                            availableAt: nil,
                            dueOn: nil
                        )
                    )
                    if index == 0 {
                        firstTaskId = task.id
                    }
                }
                return (workspace.id, firstTaskId)
            }
            peakResidentBytes = try max(peakResidentBytes, residentMemoryBytes())

            let taskOperationSamples = try await worker.run {
                let client = try AvenClient.open(path: databasePath)
                var samples = [UInt64]()
                for index in 0 ..< Self.latencySampleCount {
                    let start = clock.now
                    let result = try client.updateTask(
                        workspaceId: taskIdentity.0,
                        taskId: taskIdentity.1,
                        input: UpdateTask(
                            title: nil,
                            description: nil,
                            project: nil,
                            status: nil,
                            priority: index.isMultiple(of: 2) ? .high : .medium,
                            availableAt: .unchanged,
                            dueOn: .unchanged
                        )
                    )
                    samples.append(
                        microseconds(from: start.duration(to: clock.now))
                    )
                    guard result.changed else {
                        throw LifecycleResourceProofFailure.invariant(
                            "measured task mutation did not change state"
                        )
                    }
                }
                return samples
            }
            peakResidentBytes = try max(peakResidentBytes, residentMemoryBytes())

            let initialSync = try await driver.runMeasured(
                databasePath: databasePath,
                server: configuration.server,
                authToken: configuration.authToken
            )
            guard initialSync.summary.complete,
                  initialSync.summary.pushed >= UInt64(Self.taskCount)
            else {
                throw LifecycleResourceProofFailure.invariant(
                    "fixed task set did not complete its initial sync"
                )
            }

            var syncSamples = [UInt64]()
            var syncRequestBytes = 0
            var syncResponseBytes = 0
            for index in 0 ..< Self.latencySampleCount {
                try await worker.withClient(at: databasePath) { client in
                    let result = try client.updateTask(
                        workspaceId: taskIdentity.0,
                        taskId: taskIdentity.1,
                        input: UpdateTask(
                            title: nil,
                            description: nil,
                            project: nil,
                            status: index.isMultiple(of: 2) ? .active : .todo,
                            priority: nil,
                            availableAt: .unchanged,
                            dueOn: .unchanged
                        )
                    )
                    guard result.changed else {
                        throw LifecycleResourceProofFailure.invariant(
                            "measured sync mutation did not change state"
                        )
                    }
                }
                let start = clock.now
                let measured = try await driver.runMeasured(
                    databasePath: databasePath,
                    server: configuration.server,
                    authToken: configuration.authToken
                )
                syncSamples.append(microseconds(from: start.duration(to: clock.now)))
                guard measured.summary.complete,
                      measured.summary.pushed == 1,
                      measured.summary.blobUploaded == 0,
                      measured.summary.blobDownloaded == 0
                else {
                    throw LifecycleResourceProofFailure.invariant(
                        "measured metadata round trip violated the fixed payload"
                    )
                }
                syncRequestBytes = max(syncRequestBytes, measured.requestBodyBytes)
                syncResponseBytes = max(syncResponseBytes, measured.responseBodyBytes)
                peakResidentBytes = try max(peakResidentBytes, residentMemoryBytes())
            }

            var lifetimeSamples = [UInt64]()
            var lifetimeMemorySamples = [UInt64]()
            for _ in 0 ..< Self.lifetimeIterations {
                let start = clock.now
                let measured = try await driver.runMeasured(
                    databasePath: databasePath,
                    server: configuration.server,
                    authToken: configuration.authToken
                )
                lifetimeSamples.append(microseconds(from: start.duration(to: clock.now)))
                guard measured.summary.complete else {
                    throw LifecycleResourceProofFailure.invariant(
                        "repeated client and session cycle did not complete"
                    )
                }
                let resident = try residentMemoryBytes()
                lifetimeMemorySamples.append(resident)
                peakResidentBytes = max(peakResidentBytes, resident)
            }

            try await Task.sleep(for: .milliseconds(250))
            let postOperationResidentBytes = try residentMemoryBytes()
            peakResidentBytes = max(peakResidentBytes, postOperationResidentBytes)
            let heartbeatTickCount = heartbeatCounter.value - initialHeartbeat
            heartbeat.cancel()
            guard heartbeatTickCount > 0 else {
                throw LifecycleResourceProofFailure.invariant(
                    "main-actor heartbeat did not progress"
                )
            }

            return try ResourceProofResult(
                taskCount: Self.taskCount,
                databaseBytes: databaseFootprintBytes(paths.databaseURL),
                syncRequestBytes: syncRequestBytes,
                syncResponseBytes: syncResponseBytes,
                lifetimeIterations: Self.lifetimeIterations,
                latencySampleCount: Self.latencySampleCount,
                coldOpenMicroseconds: coldOpenMicroseconds,
                warmOpenMedianMicroseconds: median(warmOpenSamples),
                warmOpenMaximumMicroseconds: warmOpenSamples.max() ?? 0,
                taskOperationMedianMicroseconds: median(taskOperationSamples),
                taskOperationMaximumMicroseconds: taskOperationSamples.max() ?? 0,
                syncMedianMicroseconds: median(syncSamples),
                syncMaximumMicroseconds: syncSamples.max() ?? 0,
                lifetimeMedianMicroseconds: median(lifetimeSamples),
                lifetimeMaximumMicroseconds: lifetimeSamples.max() ?? 0,
                baselineResidentBytes: baselineResidentBytes,
                initializedResidentBytes: initializedResidentBytes,
                peakResidentBytes: peakResidentBytes,
                postOperationResidentBytes: postOperationResidentBytes,
                lifetimeMemoryFirstBytes: lifetimeMemorySamples.first ?? 0,
                lifetimeMemoryMedianBytes: median(lifetimeMemorySamples),
                lifetimeMemoryMaximumBytes: lifetimeMemorySamples.max() ?? 0,
                lifetimeMemoryLastBytes: lifetimeMemorySamples.last ?? 0,
                monotonicGrowthObserved: isMonotonicGrowth(lifetimeMemorySamples),
                heartbeatTickCount: heartbeatTickCount,
                thermalState: thermalStateName(ProcessInfo.processInfo.thermalState)
            )
        } catch {
            heartbeat.cancel()
            throw error
        }
    }

    @MainActor
    public func backgroundLocalOperation() async throws -> BackgroundLocalResult {
        let paths = try ApplicationSupportPath.prepareLifecycleProof(reset: true)
        let databasePath = paths.databaseURL.path
        try await worker.withClient(at: databasePath) { client in
            _ = try client.resolveWorkspace(nameOrKey: "default")
        }
        ProofOutput.write("AVEN_IOS_BACKGROUND_LOCAL status=ready\n")
        await notification(UIApplication.didEnterBackgroundNotification)

        let completed = LockedFlag()
        let operation = Task {
            try await worker.withClient(at: databasePath) { client in
                for _ in 0 ..< 100_000 {
                    _ = try client.listWorkspaces()
                }
                completed.set()
            }
        }
        await notification(UIApplication.willEnterForegroundNotification)
        let behavior = completed.value ? "completed" : "suspended"
        try await operation.value
        let reopened = try await worker.withClient(at: databasePath) { client in
            try client.listWorkspaces().isEmpty == false
        }
        return BackgroundLocalResult(
            behavior: behavior,
            databaseReopened: reopened
        )
    }

    @MainActor
    public func backgroundNetworkWait(
        configuration: IOSSyncProofConfiguration
    ) async throws -> CancelledRequestResult {
        let paths = try ApplicationSupportPath.prepareLifecycleProof(reset: true)
        let databasePath = paths.databaseURL.path
        try await createOneTask(databasePath: databasePath)
        let cancellation = try await driver.cancelOneDelayedRequest(
            databasePath: databasePath,
            server: configuration.server,
            authToken: configuration.authToken,
            beforeCancellation: {
                ProofOutput.write("AVEN_IOS_BACKGROUND_NETWORK status=ready\n")
                await notification(UIApplication.didEnterBackgroundNotification)
            }
        )
        _ = try await driver.run(
            databasePath: databasePath,
            server: configuration.server,
            authToken: configuration.authToken
        )
        return cancellation
    }

    @MainActor
    public func lockedDeviceBehavior() async throws -> LockedDeviceResult {
        let paths = try ApplicationSupportPath.prepareLifecycleProof(reset: true)
        let databasePath = paths.databaseURL.path
        let started = LockedFlag()
        let protectedDataUnavailable = LockedFlag()
        let release = DispatchSemaphore(value: 0)
        let operation = Task {
            try await worker.withClient(at: databasePath) { client in
                let storage = try client.initializeStorage()
                let workspace = try client.resolveWorkspace(nameOrKey: "default")
                _ = try client.createTask(
                    workspaceId: workspace.id,
                    input: CreateTask(
                        title: "Locked device proof",
                        description: "Synthetic attachment-free locked-state data",
                        project: "ios-lifecycle-proof",
                        status: .todo,
                        priority: .medium,
                        availableAt: nil,
                        dueOn: nil
                    )
                )
                let databaseURL = paths.databaseURL
                let walURL = URL(fileURLWithPath: databasePath + "-wal")
                let shmURL = URL(fileURLWithPath: databasePath + "-shm")
                let storageRootURL = URL(fileURLWithPath: storage.root)
                let protectedURLs = [
                    paths.directoryURL,
                    databaseURL,
                    walURL,
                    shmURL,
                    storageRootURL,
                ]
                guard protectedURLs.allSatisfy({
                    FileManager.default.fileExists(atPath: $0.path)
                }) else {
                    throw LifecycleResourceProofFailure.invariant(
                        "locked-device proof paths were incomplete"
                    )
                }
                try ApplicationSupportPath.applyDataProtection(to: protectedURLs)
                started.set()
                release.wait()

                let taskAccessible = try client.listTasks(
                    workspaceId: workspace.id
                ).count == 1
                let protectionMatched = try protectedURLs.allSatisfy { url in
                    let attributes = try FileManager.default.attributesOfItem(
                        atPath: url.path
                    )
                    return attributes[.protectionKey] as? FileProtectionType ==
                        ApplicationSupportPath.dataProtectionClass
                }
                return LockedDeviceResult(
                    protectedDataUnavailable: protectedDataUnavailable.value,
                    databaseAccessible: taskAccessible && FileManager.default
                        .isReadableFile(atPath: databaseURL.path),
                    walAccessible: FileManager.default.isReadableFile(
                        atPath: walURL.path
                    ),
                    shmAccessible: FileManager.default.isReadableFile(
                        atPath: shmURL.path
                    ),
                    storageRootAccessible: FileManager.default.isReadableFile(
                        atPath: storageRootURL.path
                    ),
                    protectionMatched: protectionMatched
                )
            }
        }
        for _ in 0 ..< 5000 where !started.value {
            try await Task.sleep(for: .milliseconds(1))
        }
        guard started.value else {
            operation.cancel()
            release.signal()
            throw LifecycleResourceProofFailure.invariant(
                "locked-device proof did not become ready"
            )
        }
        ProofOutput.write("AVEN_IOS_LOCKED_DEVICE status=ready\n")
        await notification(UIApplication.protectedDataWillBecomeUnavailableNotification)
        if !UIApplication.shared.isProtectedDataAvailable {
            protectedDataUnavailable.set()
        }
        release.signal()
        return try await operation.value
    }

    public func prepareCommittedTermination() async throws -> Never {
        let paths = try ApplicationSupportPath.prepareLifecycleProof(reset: true)
        try await createOneTask(databasePath: paths.databaseURL.path)
        ProofOutput.write("AVEN_IOS_TERMINATION_COMMITTED status=ready\n")
        while true {
            try await Task.sleep(for: .seconds(60))
        }
    }

    public func verifyCommittedTermination() async throws -> Bool {
        let paths = try ApplicationSupportPath.prepareLifecycleProof(reset: false)
        return try await worker.withClient(at: paths.databaseURL.path) { client in
            let workspace = try client.resolveWorkspace(nameOrKey: "default")
            return try client.listTasks(workspaceId: workspace.id).count == 1
        }
    }

    public func prepareNetworkWaitTermination(
        configuration: IOSSyncProofConfiguration
    ) async throws -> Never {
        let paths = try ApplicationSupportPath.prepareLifecycleProof(reset: true)
        let databasePath = paths.databaseURL.path
        try await createOneTask(databasePath: databasePath)
        let prepared = try await worker.withClient(at: databasePath) { client in
            let session = try client.startSyncSession(
                server: configuration.server,
                authToken: configuration.authToken,
                pageBudget: nil
            )
            guard let request = try session.prepareRequest() else {
                throw LifecycleResourceProofFailure.invariant(
                    "termination network wait had no request"
                )
            }
            return request
        }
        try await driver.waitForProcessTermination(prepared)
    }

    public func recoverNetworkWaitTermination(
        configuration: IOSSyncProofConfiguration
    ) async throws -> Bool {
        let paths = try ApplicationSupportPath.prepareLifecycleProof(reset: false)
        let databasePath = paths.databaseURL.path
        let reopened = try await worker.withClient(at: databasePath) { client in
            let workspace = try client.resolveWorkspace(nameOrKey: "default")
            return try client.listTasks(workspaceId: workspace.id).count == 1
        }
        let summary = try await driver.run(
            databasePath: databasePath,
            server: configuration.server,
            authToken: configuration.authToken
        )
        return reopened && summary.complete && summary.pushed > 0
    }

    private func createOneTask(databasePath: String) async throws {
        try await worker.withClient(at: databasePath) { client in
            let workspace = try client.resolveWorkspace(nameOrKey: "default")
            _ = try client.createTask(
                workspaceId: workspace.id,
                input: CreateTask(
                    title: "Lifecycle recovery proof",
                    description: "Synthetic attachment-free recovery data",
                    project: "ios-lifecycle-proof",
                    status: .todo,
                    priority: .medium,
                    availableAt: nil,
                    dueOn: nil
                )
            )
        }
    }

    @MainActor
    private func measureMicroseconds(
        _ operation: @MainActor () async throws -> Void
    ) async throws -> UInt64 {
        let start = clock.now
        try await operation()
        return microseconds(from: start.duration(to: clock.now))
    }
}

public enum ProofOutput {
    public static func write(_ marker: String) {
        FileHandle.standardOutput.write(Data(marker.utf8))
    }
}

@MainActor
private func notification(_ name: Notification.Name) async {
    for await _ in NotificationCenter.default.notifications(named: name).prefix(1) {
        return
    }
}

private func residentMemoryBytes() throws -> UInt64 {
    var info = mach_task_basic_info()
    var count = mach_msg_type_number_t(
        MemoryLayout<mach_task_basic_info>.size /
            MemoryLayout<natural_t>.size
    )
    let status = withUnsafeMutablePointer(to: &info) { pointer in
        pointer.withMemoryRebound(to: integer_t.self, capacity: Int(count)) {
            task_info(
                mach_task_self_,
                task_flavor_t(MACH_TASK_BASIC_INFO),
                $0,
                &count
            )
        }
    }
    guard status == KERN_SUCCESS else {
        throw LifecycleResourceProofFailure.memoryReadFailed(status)
    }
    return UInt64(info.resident_size)
}

private func databaseFootprintBytes(_ databaseURL: URL) throws -> UInt64 {
    let fileManager = FileManager.default
    return try [databaseURL.path, databaseURL.path + "-wal", databaseURL.path + "-shm"]
        .reduce(into: UInt64(0)) { total, path in
            guard fileManager.fileExists(atPath: path) else { return }
            let attributes = try fileManager.attributesOfItem(atPath: path)
            total += (attributes[.size] as? NSNumber)?.uint64Value ?? 0
        }
}

private func microseconds(from duration: Duration) -> UInt64 {
    let components = duration.components
    let seconds = UInt64(max(components.seconds, 0))
    let attoseconds = UInt64(max(components.attoseconds, 0))
    return seconds * 1_000_000 + attoseconds / 1_000_000_000_000
}

private func median(_ samples: [UInt64]) -> UInt64 {
    let sorted = samples.sorted()
    return sorted[sorted.count / 2]
}

private func isMonotonicGrowth(_ samples: [UInt64]) -> Bool {
    guard let first = samples.first, let last = samples.last, last > first else {
        return false
    }
    return zip(samples, samples.dropFirst()).allSatisfy { $0 <= $1 }
}

private func thermalStateName(_ state: ProcessInfo.ThermalState) -> String {
    switch state {
    case .nominal: "nominal"
    case .fair: "fair"
    case .serious: "serious"
    case .critical: "critical"
    @unknown default: "unknown"
    }
}
