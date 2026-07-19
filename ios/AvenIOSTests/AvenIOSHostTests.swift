@testable import AvenIOS
import XCTest

final class AvenIOSHostTests: XCTestCase {
    @MainActor
    func testTypedFacadeCallsKeepMainActorHeartbeatProgressing() async throws {
        let result = try await HostSmokeProof().run()

        XCTAssertGreaterThanOrEqual(result.facadeCallCount, 100)
        XCTAssertLessThanOrEqual(result.facadeCallCount, 10000)
        XCTAssertGreaterThan(result.heartbeatTickCount, 0)
        print(
            "AVEN_IOS_HOST_TEST status=pass facade=typed " +
                "worker=serial heartbeat=progressing"
        )
    }

    @MainActor
    func testSandboxPersistenceAndTypedTaskMappings() async throws {
        let result = try await PersistenceProof().run()

        XCTAssertGreaterThanOrEqual(result.workspaceCount, 1)
        XCTAssertEqual(result.taskCount, 1)
        XCTAssertEqual(
            result.statusNames,
            ["inbox", "backlog", "todo", "active", "done", "canceled"]
        )
        XCTAssertEqual(
            result.priorityNames,
            ["none", "low", "medium", "high", "urgent"]
        )
        XCTAssertEqual(result.validationErrorCount, 2)
        XCTAssertEqual(result.notFoundErrorCount, 1)
        XCTAssertTrue(result.workspaceMismatchMatched)
        XCTAssertTrue(result.walObservedBeforeRelease)
        XCTAssertTrue(result.shmObservedBeforeRelease)
        XCTAssertTrue(result.walObservedAfterReopen)
        XCTAssertTrue(result.shmObservedAfterReopen)
        XCTAssertTrue(result.dataProtectionConfigured)
        XCTAssertEqual(result.storagePathCount, 5)
        print(
            "AVEN_IOS_PERSISTENCE_TEST status=pass persistence=reopen " +
                "types=complete storage=application_support attachments=none " +
                "wal_shm=reopen protection=complete_until_first_authentication"
        )
    }

    func testApplicationSupportPathIsStableAndDurable() throws {
        let first = try ApplicationSupportPath.hostDatabaseURL()
        let second = try ApplicationSupportPath.hostDatabaseURL()

        XCTAssertEqual(first, second)
        XCTAssertEqual(first.lastPathComponent, "host-smoke.sqlite")
        XCTAssertEqual(first.deletingLastPathComponent().lastPathComponent, "AvenHostProof")
        XCTAssertTrue(
            FileManager.default.fileExists(
                atPath: first.deletingLastPathComponent().path
            )
        )
    }
}
