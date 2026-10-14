import Foundation

enum GatewayFailure: Equatable {
    case missingBinary
    case launchFailed(String)
    case timedOut
    case loadFailed(String)
}

/// Owns the gateway child process (only if this app started it).
@MainActor
final class GatewayController {
    nonisolated static let baseURL = "http://127.0.0.1:4170"
    nonisolated static let appURL = URL(string: baseURL + "/app/")!
    nonisolated static let healthURL = URL(string: baseURL + "/api/health")!

    let repoURL: URL
    let binaryURL: URL
    private var process: Process?
    private var logHandle: FileHandle?
    /// Write end of the child's stdin. Held for the app's lifetime; when this process dies for any reason
    /// (quit, crash, Force Quit / SIGKILL) the kernel closes it, and the wrapper's watcher kills the gateway.
    private var lifelinePipe: Pipe?

    /// Runs the gateway as a background child of /bin/sh. The shell forwards TERM/INT/HUP to the gateway,
    /// exits with its status, and a watcher blocked on stdin kills the gateway on EOF (parent death).
    private static let wrapperScript = """
    exec 3<&0 0</dev/null
    "$1" 3<&- &
    g=$!
    ( cat <&3 >/dev/null 2>&1; kill -TERM "$g" 2>/dev/null ) &
    w=$!
    exec 3<&-
    trap 'kill -TERM "$g" 2>/dev/null' TERM INT HUP
    s=0
    while kill -0 "$g" 2>/dev/null; do wait "$g"; s=$?; done
    kill "$w" 2>/dev/null
    exit "$s"
    """

    init() {
        let env = ProcessInfo.processInfo.environment
        let repoPath: String
        // AGAVE_REPO wins; otherwise build-app.sh records the checkout it was built from.
        if let custom = env["AGAVE_REPO"], !custom.isEmpty {
            repoPath = (custom as NSString).expandingTildeInPath
        } else if let built = Bundle.main.object(forInfoDictionaryKey: "AgaveRepo") as? String, !built.isEmpty {
            repoPath = built
        } else {
            repoPath = FileManager.default.currentDirectoryPath
        }
        repoURL = URL(fileURLWithPath: repoPath, isDirectory: true)
        binaryURL = repoURL.appendingPathComponent("gateway/target/release/agave-studio")
    }

    var isRunning: Bool { process?.isRunning ?? false }

    nonisolated static func isHealthy() async -> Bool {
        var request = URLRequest(url: healthURL, cachePolicy: .reloadIgnoringLocalAndRemoteCacheData)
        request.timeoutInterval = 1.0
        do {
            let (data, response) = try await URLSession.shared.data(for: request)
            guard let http = response as? HTTPURLResponse, http.statusCode == 200 else { return false }
            let object = try JSONSerialization.jsonObject(with: data) as? [String: Any]
            return (object?["ok"] as? Bool) ?? false
        } catch {
            return false
        }
    }

    func launch() throws {
        // Never leave a previous child running behind a new one.
        stop()

        let logURL = try logFileURL()
        let handle = try FileHandle(forWritingTo: logURL)
        try handle.seekToEnd()

        let lifeline = Pipe()
        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/bin/sh")
        process.arguments = ["-c", Self.wrapperScript, "agave-studio-gateway", binaryURL.path]
        process.currentDirectoryURL = repoURL
        process.standardInput = lifeline
        process.standardOutput = handle
        process.standardError = handle
        do {
            try process.run()
        } catch {
            try? handle.close()
            throw error
        }
        self.process = process
        self.logHandle = handle
        self.lifelinePipe = lifeline
    }

    func stop() {
        defer {
            process = nil
            try? logHandle?.close()
            logHandle = nil
            // Closing the lifeline also makes the watcher signal the gateway if it is somehow still alive.
            try? lifelinePipe?.fileHandleForWriting.close()
            lifelinePipe = nil
        }
        guard let process, process.isRunning else { return }
        let pid = process.processIdentifier
        process.terminate()
        let deadline = Date().addingTimeInterval(3)
        while process.isRunning && Date() < deadline {
            Thread.sleep(forTimeInterval: 0.05)
        }
        if process.isRunning {
            // Process puts the wrapper in its own process group; kill the whole group (wrapper, gateway, watcher).
            if getpgid(pid) == pid {
                killpg(pid, SIGKILL)
            } else {
                kill(pid, SIGKILL)
            }
        }
    }

    private func logFileURL() throws -> URL {
        let dir = FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent("Library/Logs/AgaveStudio", isDirectory: true)
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        let url = dir.appendingPathComponent("gateway.log")
        if !FileManager.default.fileExists(atPath: url.path) {
            FileManager.default.createFile(atPath: url.path, contents: nil)
        }
        return url
    }
}
