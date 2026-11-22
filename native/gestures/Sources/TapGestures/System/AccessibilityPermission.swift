import AppKit
import ApplicationServices
import Foundation

/// Accessibility permission handling.
///
/// Reading the trackpad does not need accessibility, but posting media keys and
/// the first-run setup flow do, so the app asks for it up front.
public enum AccessibilityPermission {
    public static var isTrusted: Bool {
        AXIsProcessTrusted()
    }

    /// Shows the system prompt once per launch if the app is not yet trusted.
    @discardableResult
    public static func requestIfNeeded() -> Bool {
        if isTrusted { return true }
        let options = [kAXTrustedCheckOptionPrompt.takeUnretainedValue() as String: true] as CFDictionary
        return AXIsProcessTrustedWithOptions(options)
    }

    public static func openSystemSettings() {
        let url = URL(
            string: "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility"
        )
        if let url {
            NSWorkspace.shared.open(url)
        }
    }
}
