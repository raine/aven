import AvenLocalProofCore
import Foundation

@main
struct AvenLocalProofCommand {
    static func main() async throws {
        let arguments = Array(CommandLine.arguments.dropFirst())
        switch arguments.first {
        case "local" where arguments.count == 1:
            try await runLocalProof()
        case "sync" where arguments.count == 2:
            try await runSyncProof(binaryPath: arguments[1])
        default:
            throw ProofFailure.invariant(
                "usage: aven-local-proof local | sync <aven-binary>"
            )
        }
    }

    private static func runLocalProof() async throws {
        let directory = try temporaryDirectory(named: "aven-swift-local-proof")
        defer { try? FileManager.default.removeItem(at: directory) }

        let result = try await LocalProof().run(
            databasePath: directory.appendingPathComponent("local.sqlite").path
        )
        precondition(result.workspaceCount >= 1)
        precondition(result.taskCount == 1)
        precondition(result.statusName == "active")
        precondition(result.priorityName == "high")
        precondition(result.requestByteCount > 0)
        precondition(result.validationErrorMatched)
        precondition(result.notFoundErrorMatched)

        print("workspace_resolution=pass")
        print("create_fetch_list_update=pass")
        print("reopen_migrations=pass")
        print("typed_status_priority_dates=pass")
        print("typed_validation_not_found=pass")
        print("sync_byte_transfer=pass")
        print("worker_executor=pass")
    }

    private static func runSyncProof(binaryPath: String) async throws {
        let directory = try temporaryDirectory(named: "aven-swift-sync-proof")
        defer { try? FileManager.default.removeItem(at: directory) }
        let authToken = "swift-proof-private-authorization"
        let server = try ProofServer.start(
            binaryPath: binaryPath,
            directory: directory,
            authToken: authToken
        )
        defer { server.stop() }

        let result = try await SyncProof().run(
            directory: directory,
            server: server.url,
            authToken: authToken
        )
        precondition(result.malformedResponseAtomic)
        precondition(result.authorizationAccepted)
        precondition(result.conflictVariantsTyped)
        precondition(result.replicasConverged)
        precondition(result.unresolvedConflictCount == 0)

        print("urlsession_transport=pass")
        print("authorized_round_trip=pass")
        print("malformed_response_atomicity=pass")
        print("typed_conflict_variants=pass")
        print("conflict_resolution_convergence=pass")
        print("unresolved_conflicts=0")
        print("privacy_safe_output=pass")
    }

    private static func temporaryDirectory(named name: String) throws -> URL {
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent("\(name)-\(UUID().uuidString)")
        try FileManager.default.createDirectory(
            at: directory,
            withIntermediateDirectories: true
        )
        return directory
    }
}
