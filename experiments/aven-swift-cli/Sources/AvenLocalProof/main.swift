import AvenLocalProofCore
import Foundation

@main
struct AvenLocalProofCommand {
    static func main() async throws {
        guard CommandLine.arguments.dropFirst() == ["local"] else {
            throw ProofFailure.invariant("usage: aven-local-proof local")
        }

        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent("aven-swift-local-proof-\(UUID().uuidString)")
        try FileManager.default.createDirectory(
            at: directory,
            withIntermediateDirectories: true
        )
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
}
