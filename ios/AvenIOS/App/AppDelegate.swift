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
                if CommandLine.arguments.contains("--aven-sync-seed") {
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

    static func writeFailure() {
        write("AVEN_IOS_HOST_PROOF status=fail code=host_smoke\n")
    }

    private static func write(_ marker: String) {
        FileHandle.standardOutput.write(Data(marker.utf8))
    }
}
