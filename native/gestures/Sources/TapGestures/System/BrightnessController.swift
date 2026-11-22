import CoreGraphics
import Darwin
import Foundation

/// Brightness for the built-in display.
///
/// Primary path is Apple's private `DisplayServices` framework (the same one the
/// system brightness keys go through). When that is unavailable we fall back to
/// posting brightness key events and tracking an approximate level.
public final class BrightnessController: LevelController {
    public static let shared = BrightnessController()

    private typealias GetBrightnessFn = @convention(c) (CGDirectDisplayID, UnsafeMutablePointer<Float>) -> Int32
    private typealias SetBrightnessFn = @convention(c) (CGDirectDisplayID, Float) -> Int32
    private var getBrightness: GetBrightnessFn?
    private var setBrightness: SetBrightnessFn?
    private var display: CGDirectDisplayID
    private var fallbackLevel: Double = 0.5
    private var cachedLevel: Double = 0.5

    public init() {
        display = BrightnessController.builtInDisplay() ?? CGMainDisplayID()
        if let handle = dlopen(
            "/System/Library/PrivateFrameworks/DisplayServices.framework/DisplayServices",
            RTLD_NOW
        ) {
            if let symbol = dlsym(handle, "DisplayServicesGetBrightness") {
                getBrightness = unsafeBitCast(symbol, to: GetBrightnessFn.self)
            }
            if let symbol = dlsym(handle, "DisplayServicesSetBrightness") {
                setBrightness = unsafeBitCast(symbol, to: SetBrightnessFn.self)
            }
        }
        refresh()
    }

    /// Whether precise brightness control is available on this Mac.
    public var supportsPreciseControl: Bool {
        getBrightness != nil || setBrightness != nil
    }

    public var level: Double {
        cachedLevel
    }

    /// Brightness has no mute.
    public var isMuted: Bool { false }

    public func refresh() {
        display = BrightnessController.builtInDisplay() ?? CGMainDisplayID()
        guard let getBrightness else {
            cachedLevel = fallbackLevel
            return
        }
        var value = Float(0)
        if getBrightness(display, &value) == 0 {
            cachedLevel = Double(min(max(value, 0), 1))
            fallbackLevel = cachedLevel
        } else {
            cachedLevel = fallbackLevel
        }
    }

    public func setLevel(_ level: Double) {
        let clamped = Float(min(max(level, 0), 1))
        cachedLevel = Double(clamped)

        if let setBrightness {
            _ = setBrightness(display, clamped)
            fallbackLevel = Double(clamped)
            return
        }

        // Fallback: nudge with the system keys and remember roughly where we are.
        let delta = Double(clamped) - fallbackLevel
        let steps = Int((abs(delta) * 16).rounded())
        guard steps > 0 else { return }
        for _ in 0..<min(steps, 16) {
            MediaKeyController.shared.send(delta > 0 ? .brightnessUp : .brightnessDown)
        }
        fallbackLevel = Double(clamped)
    }

    public func toggleMute() {
        // No-op: brightness cannot be muted.
    }

    private static func builtInDisplay() -> CGDirectDisplayID? {
        var count = UInt32(0)
        guard CGGetActiveDisplayList(0, nil, &count) == .success, count > 0 else { return nil }
        var displays = [CGDirectDisplayID](repeating: 0, count: Int(count))
        guard CGGetActiveDisplayList(count, &displays, &count) == .success else { return nil }
        return displays.first { CGDisplayIsBuiltin($0) != 0 }
    }
}
