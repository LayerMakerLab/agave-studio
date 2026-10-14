import AppKit
import Foundation
import SwiftUI
import WebKit

enum GatewayState: Equatable {
    case checking
    case starting
    case ready
    case failed(GatewayFailure)
}

@MainActor
final class AppModel: ObservableObject {
    static let shared = AppModel()

    @Published private(set) var state: GatewayState = .checking
    let gateway = GatewayController()
    let webView: WKWebView
    private let webDelegate = WebDelegate()
    private var bootTask: Task<Void, Never>?

    private init() {
        let view = WKWebView(frame: .zero, configuration: WebDelegate.makeConfiguration())
        // Transparent until the page paints, so no white flash; the dark SwiftUI background shows through.
        view.setValue(false, forKey: "drawsBackground")
        view.underPageBackgroundColor = NSColor(hex: 0x08080A)
        view.wantsLayer = true
        view.layer?.backgroundColor = NSColor(hex: 0x08080A).cgColor
        view.allowsBackForwardNavigationGestures = false
        view.navigationDelegate = webDelegate
        view.uiDelegate = webDelegate
        webView = view
        webDelegate.onLoadFailure = { [weak self] message in
            guard let self, self.state == .ready else { return }
            self.state = .failed(.loadFailed(message))
        }
    }

    func start() {
        bootTask?.cancel()
        bootTask = Task { await boot() }
    }

    func reload() {
        if state == .ready {
            webView.reload()
        } else {
            start()
        }
    }

    func openInBrowser() {
        NSWorkspace.shared.open(GatewayController.appURL)
    }

    func shutdown() {
        bootTask?.cancel()
        gateway.stop()
    }

    private func boot() async {
        state = .checking
        if await GatewayController.isHealthy() {
            if Task.isCancelled { return }
            finishReady()
            return
        }

        if Task.isCancelled { return }

        // A child we launched earlier may still be coming up (e.g. Reload pressed while starting): keep polling it
        // rather than spawning a second gateway that would fight for the port.
        if !gateway.isRunning {
            guard FileManager.default.isExecutableFile(atPath: gateway.binaryURL.path) else {
                state = .failed(.missingBinary)
                return
            }
            do {
                try gateway.launch()
            } catch {
                state = .failed(.launchFailed(error.localizedDescription))
                return
            }
        }

        state = .starting
        let deadline = Date().addingTimeInterval(15)
        while Date() < deadline {
            if Task.isCancelled { return }
            if await GatewayController.isHealthy() {
                if Task.isCancelled { return }
                finishReady()
                return
            }
            if !gateway.isRunning { break }
            try? await Task.sleep(nanoseconds: 500_000_000)
        }

        // A superseding boot owns the child now; do not tear it down or overwrite its state.
        if Task.isCancelled { return }
        gateway.stop()
        state = .failed(.timedOut)
    }

    private func finishReady() {
        state = .ready
        webView.load(URLRequest(url: GatewayController.appURL))
    }
}
