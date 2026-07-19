import AvenUniFFI
import Dispatch

public final class RustWorker: @unchecked Sendable {
    private let queue: DispatchQueue

    public init(label: String = "com.raine.aven.ios-proof.rust") {
        queue = DispatchQueue(label: label)
    }

    public func run<T: Sendable>(
        _ operation: @escaping @Sendable () throws -> T
    ) async throws -> T {
        try await withCheckedThrowingContinuation { continuation in
            queue.async {
                do {
                    try continuation.resume(returning: operation())
                } catch {
                    continuation.resume(throwing: error)
                }
            }
        }
    }

    public func withClient<T: Sendable>(
        at databasePath: String,
        _ operation: @escaping @Sendable (AvenClient) throws -> T
    ) async throws -> T {
        try await run {
            let client = try AvenClient.open(path: databasePath)
            return try operation(client)
        }
    }
}
