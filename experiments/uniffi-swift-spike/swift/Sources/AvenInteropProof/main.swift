import AvenInterop
import Foundation

@main
struct AvenInteropProof {
    static func main() async throws {
        let store = try ProbeStore()

        let ownedRuntimeSnapshot = try store.snapshotViaOwnedRuntime()
        precondition(ownedRuntimeSnapshot.rowCount == 1)
        precondition(ownedRuntimeSnapshot.name == "aven")
        precondition(ownedRuntimeSnapshot.nickname == nil)
        precondition(ownedRuntimeSnapshot.payload == Data([0, 1, 2, 0xFF]))
        precondition(ownedRuntimeSnapshot.kind == .binary)

        let nativeAsyncSnapshot = try await store.snapshotViaNativeAsync(delayMs: 1)
        precondition(nativeAsyncSnapshot == ownedRuntimeSnapshot)

        let finalReferenceSnapshot = try await Task {
            let temporaryStore = try ProbeStore()
            return try await temporaryStore.snapshotViaNativeAsync(delayMs: 1)
        }.value
        precondition(finalReferenceSnapshot == ownedRuntimeSnapshot)

        let cancellationProbe = Task {
            try await store.snapshotViaNativeAsync(delayMs: 100)
        }
        cancellationProbe.cancel()
        let cancellationResult = try await cancellationProbe.value
        precondition(cancellationResult == ownedRuntimeSnapshot)

        do {
            try store.failTyped()
            preconditionFailure("typed error was not thrown")
        } catch ProbeError.Intentional {}

        print("opaque_object=pass")
        print("owned_runtime_sqlx=pass")
        print("native_async_sqlx=pass")
        print("async_final_reference_drop=pass")
        print("record_enum_optional_bytes=pass")
        print("typed_error=pass")
        print("swift_task_cancellation_propagated=false")
    }
}
