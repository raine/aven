import Darwin
import Foundation
import UIKit

@main
final class AppDelegate: UIResponder, UIApplicationDelegate {
    func application(
        _: UIApplication,
        didFinishLaunchingWithOptions _: [
            UIApplication.LaunchOptionsKey: Any
        ]? = nil
    ) -> Bool {
        let isXCTestHost = ProcessInfo.processInfo.environment[
            "XCTestConfigurationFilePath"
        ] != nil
        let explicitlyRequested = CommandLine.arguments.contains(
            "--aven-host-proof"
        )
        guard !isXCTestHost || explicitlyRequested else {
            return true
        }

        Task { @MainActor in
            do {
                if CommandLine.arguments.contains("--aven-resource-proof") {
                    let configuration = try IOSSyncProofConfiguration.fromEnvironment()
                    let result = try await LifecycleResourceProof().measureResources(
                        configuration: configuration
                    )
                    ProofMarker.writeResourcePass(result)
                } else if CommandLine.arguments.contains("--aven-background-local") {
                    let result = try await LifecycleResourceProof()
                        .backgroundLocalOperation()
                    ProofMarker.writeBackgroundLocalPass(result)
                } else if CommandLine.arguments.contains("--aven-background-network") {
                    let configuration = try IOSSyncProofConfiguration.fromEnvironment()
                    let result = try await LifecycleResourceProof()
                        .backgroundNetworkWait(configuration: configuration)
                    ProofMarker.writeBackgroundNetworkPass(result)
                } else if CommandLine.arguments.contains("--aven-locked-device") {
                    let result = try await LifecycleResourceProof()
                        .lockedDeviceBehavior()
                    ProofMarker.writeLockedDevicePass(result)
                } else if CommandLine.arguments.contains("--aven-termination-committed") {
                    try await LifecycleResourceProof().prepareCommittedTermination()
                } else if CommandLine.arguments.contains("--aven-termination-committed-verify") {
                    let recovered = try await LifecycleResourceProof()
                        .verifyCommittedTermination()
                    ProofMarker.writeCommittedTerminationPass(recovered: recovered)
                } else if CommandLine.arguments.contains("--aven-termination-network") {
                    let configuration = try IOSSyncProofConfiguration.fromEnvironment()
                    try await LifecycleResourceProof().prepareNetworkWaitTermination(
                        configuration: configuration
                    )
                } else if CommandLine.arguments.contains("--aven-termination-network-recover") {
                    let configuration = try IOSSyncProofConfiguration.fromEnvironment()
                    let recovered = try await LifecycleResourceProof()
                        .recoverNetworkWaitTermination(configuration: configuration)
                    ProofMarker.writeNetworkTerminationPass(recovered: recovered)
                } else if CommandLine.arguments.contains("--aven-sync-seed") {
                    let configuration = try IOSSyncProofConfiguration.fromEnvironment()
                    let result = try await IOSSyncProof().seed(
                        configuration: configuration
                    )
                    ProofMarker.writeSyncSeedPass(result)
                } else if CommandLine.arguments.contains("--aven-sync-conflict") {
                    let configuration = try IOSSyncProofConfiguration.fromEnvironment()
                    let result = try await IOSSyncProof().convergeConflict(
                        configuration: configuration
                    )
                    ProofMarker.writeSyncConflictPass(result)
                } else {
                    _ = try await HostSmokeProof().run()
                    _ = try await PersistenceProof().run()
                    ProofMarker.writePass()
                }
                exit(EXIT_SUCCESS)
            } catch {
                ProofMarker.writeFailure()
                exit(EXIT_FAILURE)
            }
        }
        return true
    }
}

private enum ProofMarker {
    static func writePass() {
        write(
            "AVEN_IOS_HOST_PROOF status=pass facade=typed " +
                "worker=serial heartbeat=progressing persistence=reopen " +
                "types=complete storage=application_support wal_shm=reopen " +
                "protection=complete_until_first_authentication\n"
        )
    }

    static func writeSyncSeedPass(_ result: IOSSyncSeedResult) {
        guard result.malformedResponseAtomic,
              result.cancellationFailRequestCount == 1,
              result.cancellationStateAtomic,
              result.freshSessionCompleted,
              result.heartbeatTickCount > 0,
              result.attachmentTransferCount == 0
        else {
            writeFailure()
            return
        }
        write(
            "AVEN_IOS_SYNC_SEED status=pass transport=urlsession " +
                "auth=accepted metadata=push malformed=atomic " +
                "cancellation=single_fail recovery=fresh_session " +
                "heartbeat=progressing attachments=zero\n"
        )
    }

    static func writeSyncConflictPass(_ result: IOSSyncConflictResult) {
        guard result.conflictVariantsTyped,
              result.selectedRemoteVariant,
              result.unresolvedConflictCount == 0,
              result.attachmentTransferCount == 0
        else {
            writeFailure()
            return
        }
        write(
            "AVEN_IOS_SYNC_CONFLICT status=pass variants=typed " +
                "resolution=remote metadata=push_pull convergence=local " +
                "unresolved=zero attachments=zero\n"
        )
    }

    static func writeResourcePass(_ result: ResourceProofResult) {
        guard result.taskCount == LifecycleResourceProof.taskCount,
              result.lifetimeIterations == LifecycleResourceProof.lifetimeIterations,
              result.latencySampleCount == LifecycleResourceProof.latencySampleCount,
              result.heartbeatTickCount > 0,
              !result.monotonicGrowthObserved
        else {
            writeFailure()
            return
        }
        write(
            "AVEN_IOS_RESOURCE_PROOF status=pass configuration=release " +
                "tasks=\(result.taskCount) database_bytes=\(result.databaseBytes) " +
                "sync_request_bytes=\(result.syncRequestBytes) " +
                "sync_response_bytes=\(result.syncResponseBytes) " +
                "iterations=\(result.lifetimeIterations) " +
                "samples=\(result.latencySampleCount) " +
                "cold_open_us=\(result.coldOpenMicroseconds) " +
                "warm_open_median_us=\(result.warmOpenMedianMicroseconds) " +
                "warm_open_max_us=\(result.warmOpenMaximumMicroseconds) " +
                "task_median_us=\(result.taskOperationMedianMicroseconds) " +
                "task_max_us=\(result.taskOperationMaximumMicroseconds) " +
                "sync_median_us=\(result.syncMedianMicroseconds) " +
                "sync_max_us=\(result.syncMaximumMicroseconds) " +
                "lifetime_median_us=\(result.lifetimeMedianMicroseconds) " +
                "lifetime_max_us=\(result.lifetimeMaximumMicroseconds) " +
                "memory_baseline_bytes=\(result.baselineResidentBytes) " +
                "memory_initialized_bytes=\(result.initializedResidentBytes) " +
                "memory_peak_bytes=\(result.peakResidentBytes) " +
                "memory_post_bytes=\(result.postOperationResidentBytes) " +
                "lifetime_memory_first_bytes=\(result.lifetimeMemoryFirstBytes) " +
                "lifetime_memory_median_bytes=\(result.lifetimeMemoryMedianBytes) " +
                "lifetime_memory_max_bytes=\(result.lifetimeMemoryMaximumBytes) " +
                "lifetime_memory_last_bytes=\(result.lifetimeMemoryLastBytes) " +
                "monotonic_growth=false heartbeat=progressing " +
                "thermal=\(result.thermalState) attachments=zero\n"
        )
    }

    static func writeBackgroundLocalPass(_ result: BackgroundLocalResult) {
        guard result.databaseReopened,
              result.behavior == "completed" || result.behavior == "suspended"
        else {
            writeFailure()
            return
        }
        write(
            "AVEN_IOS_BACKGROUND_LOCAL status=pass behavior=\(result.behavior) " +
                "database=reopened\n"
        )
    }

    static func writeBackgroundNetworkPass(_ result: CancelledRequestResult) {
        guard result.failRequestCount == 1,
              result.cursorBefore == result.cursorAfter
        else {
            writeFailure()
            return
        }
        write(
            "AVEN_IOS_BACKGROUND_NETWORK status=pass transport=urlsession " +
                "cancellation=single_fail recovery=fresh_session\n"
        )
    }

    static func writeLockedDevicePass(_ result: LockedDeviceResult) {
        guard result.protectedDataUnavailable,
              result.databaseAccessible,
              result.walAccessible,
              result.shmAccessible,
              result.storageRootAccessible,
              result.protectionMatched
        else {
            writeFailure()
            return
        }
        write(
            "AVEN_IOS_LOCKED_DEVICE status=pass protection=" +
                "complete_until_first_authentication database=accessible " +
                "wal=accessible shm=accessible storage_root=accessible\n"
        )
    }

    static func writeCommittedTerminationPass(recovered: Bool) {
        guard recovered else {
            writeFailure()
            return
        }
        write(
            "AVEN_IOS_TERMINATION_COMMITTED status=pass database=reopened " +
                "mutation=observed\n"
        )
    }

    static func writeNetworkTerminationPass(recovered: Bool) {
        guard recovered else {
            writeFailure()
            return
        }
        write(
            "AVEN_IOS_TERMINATION_NETWORK status=pass database=reopened " +
                "session=fresh recovery=complete\n"
        )
    }

    static func writeFailure() {
        write("AVEN_IOS_HOST_PROOF status=fail code=host_smoke\n")
    }

    private static func write(_ marker: String) {
        FileHandle.standardOutput.write(Data(marker.utf8))
    }
}
