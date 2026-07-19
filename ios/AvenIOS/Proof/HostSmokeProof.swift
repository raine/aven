import AvenUniFFI
import Foundation

public enum HostSmokeProofError: Error {
    case heartbeatDidNotProgress
}

public struct HostSmokeProofResult: Equatable, Sendable {
    public let facadeCallCount: Int
    public let heartbeatTickCount: Int
}

private final class HeartbeatCounter: @unchecked Sendable {
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

public struct HostSmokeProof: Sendable {
    private let worker: RustWorker
    private let minimumFacadeCalls = 100
    private let maximumFacadeCalls = 10000

    public init(worker: RustWorker = RustWorker()) {
        self.worker = worker
    }

    @MainActor
    public func run() async throws -> HostSmokeProofResult {
        let databaseURL = try ApplicationSupportPath.hostDatabaseURL()
        let databasePath = databaseURL.path
        let heartbeatCounter = HeartbeatCounter()
        let heartbeat = Task { @MainActor in
            while !Task.isCancelled {
                heartbeatCounter.increment()
                try? await Task.sleep(for: .milliseconds(1))
            }
        }

        await Task.yield()
        let initialHeartbeatTicks = heartbeatCounter.value
        let facadeCallCount: Int
        do {
            facadeCallCount = try await worker.run {
                let client = try AvenClient.open(path: databasePath)
                var callCount = 0
                repeat {
                    _ = try client.listWorkspaces()
                    callCount += 1
                } while callCount < maximumFacadeCalls && (
                    callCount < minimumFacadeCalls ||
                        heartbeatCounter.value <= initialHeartbeatTicks
                )
                return callCount
            }
        } catch {
            heartbeat.cancel()
            throw error
        }

        let heartbeatTickCount = heartbeatCounter.value - initialHeartbeatTicks
        heartbeat.cancel()
        guard heartbeatTickCount > 0 else {
            throw HostSmokeProofError.heartbeatDidNotProgress
        }

        return HostSmokeProofResult(
            facadeCallCount: facadeCallCount,
            heartbeatTickCount: heartbeatTickCount
        )
    }
}
