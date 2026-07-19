import AvenUniFFI

@main
struct AvenUniFFIPackageProbe {
    static func main() {
        do {
            _ = try AvenClient.open(path: "/tmp/aven-ios-package-probe.sqlite")
        } catch {
            // Linking this consumer proves the production facade entry point resolves.
        }
    }
}
