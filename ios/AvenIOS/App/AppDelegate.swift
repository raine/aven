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
                _ = try await HostSmokeProof().run()
                _ = try await PersistenceProof().run()
                ProofMarker.writePass()
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
                "types=complete storage=application_support\n"
        )
    }

    static func writeFailure() {
        write("AVEN_IOS_HOST_PROOF status=fail code=host_smoke\n")
    }

    private static func write(_ marker: String) {
        FileHandle.standardOutput.write(Data(marker.utf8))
    }
}
