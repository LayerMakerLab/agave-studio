import AppKit
import SwiftUI
import WebKit

struct ContentView: View {
    @ObservedObject var model: AppModel

    var body: some View {
        ZStack {
            Palette.background
            WebHostView(webView: model.webView)
                .opacity(model.state == .ready ? 1 : 0)
            overlay
        }
        .frame(minWidth: 1100, minHeight: 720)
        .ignoresSafeArea()
        .tint(Palette.silver)
        .preferredColorScheme(.dark)
    }

    @ViewBuilder
    private var overlay: some View {
        switch model.state {
        case .checking:
            StatusOverlay(message: "Checking gateway")
        case .starting:
            StatusOverlay(message: "Starting gateway")
        case .failed(let failure):
            GatewayErrorView(failure: failure, gateway: model.gateway) {
                model.start()
            }
        case .ready:
            EmptyView()
        }
    }
}

struct WebHostView: NSViewRepresentable {
    let webView: WKWebView

    func makeNSView(context: Context) -> WKWebView { webView }

    func updateNSView(_ nsView: WKWebView, context: Context) {}
}

struct StatusOverlay: View {
    let message: String

    var body: some View {
        VStack(spacing: 14) {
            ProgressView()
                .controlSize(.regular)
                .tint(Palette.silver)
            Text(message)
                .font(.system(size: 13, weight: .medium))
                .foregroundStyle(Palette.silverMuted)
        }
    }
}

struct GatewayErrorView: View {
    let failure: GatewayFailure
    let gateway: GatewayController
    let onRetry: () -> Void

    private var reason: String {
        switch failure {
        case .missingBinary:
            return "The gateway binary has not been built yet."
        case .launchFailed(let message):
            return "The gateway could not be started: \(message)"
        case .timedOut:
            return "The gateway did not answer on 127.0.0.1:4170 within 15 seconds."
        case .loadFailed(let message):
            return "The workspace stopped loading: \(message)"
        }
    }

    private var commands: [String] {
        let runCommand = "\"\(gateway.binaryURL.path)\""
        switch failure {
        case .missingBinary:
            return ["cd \"\(gateway.repoURL.path)/gateway\" && cargo build --release", runCommand]
        case .launchFailed, .timedOut, .loadFailed:
            return [runCommand]
        }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            Text("Gateway is not running")
                .font(.system(size: 20, weight: .semibold))
                .foregroundStyle(Palette.ink)
            Text(reason)
                .font(.system(size: 13))
                .foregroundStyle(Palette.silverMuted)
            Text("Run this in Terminal, then press Retry. Log: ~/Library/Logs/AgaveStudio/gateway.log")
                .font(.system(size: 13))
                .foregroundStyle(Palette.silverMuted)
            VStack(alignment: .leading, spacing: 8) {
                ForEach(commands, id: \.self) { command in
                    // No .textSelection: its highlight uses the system accent (blue by default). Copy instead.
                    Text(command)
                        .font(.system(size: 12, design: .monospaced))
                        .foregroundStyle(Palette.silver)
                        .frame(maxWidth: .infinity, alignment: .leading)
                }
            }
            .padding(14)
            .background(Palette.graphite, in: RoundedRectangle(cornerRadius: 10))
            .overlay(RoundedRectangle(cornerRadius: 10).stroke(Palette.graphiteBorder, lineWidth: 1))
            HStack(spacing: 10) {
                Spacer()
                Button("Copy command") {
                    NSPasteboard.general.clearContents()
                    NSPasteboard.general.setString(commands.joined(separator: "\n"), forType: .string)
                }
                .buttonStyle(GraphiteButtonStyle(prominent: false))
                // Custom style: a system default button is drawn in the accent color (blue by default).
                Button("Retry", action: onRetry)
                    .keyboardShortcut(.defaultAction)
                    .buttonStyle(GraphiteButtonStyle(prominent: true))
            }
        }
        .padding(28)
        .frame(width: 560)
        .background(Palette.graphite.opacity(0.6), in: RoundedRectangle(cornerRadius: 16))
        .overlay(RoundedRectangle(cornerRadius: 16).stroke(Palette.graphiteBorder, lineWidth: 1))
    }
}

struct GraphiteButtonStyle: ButtonStyle {
    let prominent: Bool

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(.system(size: 13, weight: .medium))
            .foregroundStyle(prominent ? Palette.background : Palette.silver)
            .padding(.horizontal, 16)
            .padding(.vertical, 7)
            .background(
                Capsule().fill(prominent ? Palette.silver : Palette.graphite)
            )
            .overlay(Capsule().stroke(Palette.graphiteBorder, lineWidth: prominent ? 0 : 1))
            .opacity(configuration.isPressed ? 0.75 : 1)
            .contentShape(Capsule())
    }
}
