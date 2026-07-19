import Foundation

public struct SandboxPersistencePaths: Equatable, Sendable {
    public let directoryURL: URL
    public let databaseURL: URL
}

public enum ApplicationSupportPath {
    public static let dataProtectionClass = FileProtectionType.completeUntilFirstUserAuthentication

    public static func hostDatabaseURL(
        fileManager: FileManager = .default
    ) throws -> URL {
        let directory = try proofRootURL(fileManager: fileManager)
        return directory.appendingPathComponent(
            "host-smoke.sqlite",
            isDirectory: false
        )
    }

    public static func preparePersistenceProof(
        fileManager: FileManager = .default
    ) throws -> SandboxPersistencePaths {
        let root = try proofRootURL(fileManager: fileManager)
        let directory = root.appendingPathComponent(
            "Persistence",
            isDirectory: true
        )
        if fileManager.fileExists(atPath: directory.path) {
            try fileManager.removeItem(at: directory)
        }
        try fileManager.createDirectory(
            at: directory,
            withIntermediateDirectories: true,
            attributes: [.protectionKey: dataProtectionClass]
        )
        return SandboxPersistencePaths(
            directoryURL: directory,
            databaseURL: directory.appendingPathComponent(
                "persistence.sqlite",
                isDirectory: false
            )
        )
    }

    public static func applyDataProtection(
        to urls: [URL],
        fileManager: FileManager = .default
    ) throws {
        for url in urls {
            try fileManager.setAttributes(
                [.protectionKey: dataProtectionClass],
                ofItemAtPath: url.path
            )
        }
    }

    private static func proofRootURL(
        fileManager: FileManager
    ) throws -> URL {
        let applicationSupport = try fileManager.url(
            for: .applicationSupportDirectory,
            in: .userDomainMask,
            appropriateFor: nil,
            create: true
        )
        let directory = applicationSupport.appendingPathComponent(
            "AvenHostProof",
            isDirectory: true
        )
        try fileManager.createDirectory(
            at: directory,
            withIntermediateDirectories: true,
            attributes: [.protectionKey: dataProtectionClass]
        )
        return directory
    }
}
