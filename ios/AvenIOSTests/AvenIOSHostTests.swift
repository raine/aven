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
