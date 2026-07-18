import AvenLocalProofCore
import Foundation

final class ProofServer: @unchecked Sendable {
    private final class StartupState: @unchecked Sendable {
        private let lock = NSLock()
        private let signal = DispatchSemaphore(value: 0)
        private var buffer = Data()
        private var listeningURL: String?

        func consume(_ data: Data) {
            guard !data.isEmpty else { return }
            lock.lock()
            defer { lock.unlock() }
            buffer.append(data)
            while let newline = buffer.firstIndex(of: 0x0A) {
                let lineData = buffer[..<newline]
                buffer.removeSubrange(...newline)
                let line = String(decoding: lineData, as: UTF8.self)
                if let value = line.split(separator: " ").first(where: {
                    $0.hasPrefix("url=")
                }), line.hasPrefix("listening ") {
                    listeningURL = String(value.dropFirst("url=".count))
                    signal.signal()
                }
            }
        }

        func processExited() {
            signal.signal()
        }

        func waitForURL(timeout: TimeInterval) -> String? {
            _ = signal.wait(timeout: .now() + timeout)
            lock.lock()
            defer { lock.unlock() }
            return listeningURL
        }
    }

    let url: String

    private let process: Process
    private let stdoutPipe: Pipe
    private let stderrPipe: Pipe

    private init(
        url: String,
        process: Process,
        stdoutPipe: Pipe,
        stderrPipe: Pipe
    ) {
        self.url = url
        self.process = process
        self.stdoutPipe = stdoutPipe
        self.stderrPipe = stderrPipe
    }

    static func start(
        binaryPath: String,
        directory: URL,
        authToken: String
    ) throws -> ProofServer {
        let configDirectory = directory.appendingPathComponent("server-config")
        try FileManager.default.createDirectory(
            at: configDirectory,
            withIntermediateDirectories: true
        )
        let quotedToken = "'" + authToken.replacingOccurrences(of: "'", with: "''") + "'"
        try "sync:\n  auth_token: \(quotedToken)\n".write(
            to: configDirectory.appendingPathComponent("config.yaml"),
            atomically: true,
            encoding: .utf8
        )

        let stdoutPipe = Pipe()
        let stderrPipe = Pipe()
        let state = StartupState()
        stdoutPipe.fileHandleForReading.readabilityHandler = { handle in
            state.consume(handle.availableData)
        }
        stderrPipe.fileHandleForReading.readabilityHandler = { handle in
            _ = handle.availableData
        }

        let process = Process()
        process.executableURL = URL(fileURLWithPath: binaryPath)
        process.arguments = [
            "server",
            "--bind", "127.0.0.1:0",
            "--data", directory.appendingPathComponent("server.sqlite").path,
        ]
        var environment = ProcessInfo.processInfo.environment
        environment["AVEN_CONFIG_DIR"] = configDirectory.path
        environment.removeValue(forKey: "AVEN_DB")
        environment.removeValue(forKey: "AVEN_SYNC_SERVER")
        process.environment = environment
        process.standardOutput = stdoutPipe
        process.standardError = stderrPipe
        process.terminationHandler = { _ in state.processExited() }
        try process.run()

        guard let url = state.waitForURL(timeout: 10), process.isRunning else {
            if process.isRunning {
                process.terminate()
            }
            process.waitUntilExit()
            stdoutPipe.fileHandleForReading.readabilityHandler = nil
            stderrPipe.fileHandleForReading.readabilityHandler = nil
            throw ProofFailure.invariant("sync server did not become ready")
        }
        return ProofServer(
            url: url,
            process: process,
            stdoutPipe: stdoutPipe,
            stderrPipe: stderrPipe
        )
    }

    func stop() {
        if process.isRunning {
            process.terminate()
        }
        process.waitUntilExit()
        stdoutPipe.fileHandleForReading.readabilityHandler = nil
        stderrPipe.fileHandleForReading.readabilityHandler = nil
    }
}
