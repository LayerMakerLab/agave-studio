import AppKit
import SwiftUI

/// Graphite and silver palette. No blue anywhere.
enum Palette {
    static let background = Color(hex: 0x08080A)
    static let graphite = Color(hex: 0x1C1D21)
    static let graphiteBorder = Color(hex: 0x2C2D33)
    static let silver = Color(hex: 0xC9C6C0)
    static let silverMuted = Color(hex: 0x8A8782)
    static let ink = Color(hex: 0xE8E6E1)
}

extension Color {
    init(hex: UInt32) {
        self.init(
            .sRGB,
            red: Double((hex >> 16) & 0xFF) / 255,
            green: Double((hex >> 8) & 0xFF) / 255,
            blue: Double(hex & 0xFF) / 255,
            opacity: 1
        )
    }
}

extension NSColor {
    convenience init(hex: UInt32) {
        self.init(
            srgbRed: CGFloat((hex >> 16) & 0xFF) / 255,
            green: CGFloat((hex >> 8) & 0xFF) / 255,
            blue: CGFloat(hex & 0xFF) / 255,
            alpha: 1
        )
    }
}
