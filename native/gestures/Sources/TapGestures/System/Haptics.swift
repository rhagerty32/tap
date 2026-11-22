import AppKit
import Foundation

/// Trackpad haptics through the public AppKit feedback API.
public final class Haptics {
    public static let shared = Haptics()

    private let performer = NSHapticFeedbackManager.defaultPerformer

    private init() {}

    /// A single step tick, used while a slide crosses a level boundary.
    public func tick(_ strength: HapticStrength) {
        let pattern: NSHapticFeedbackManager.FeedbackPattern
        switch strength {
        case .light: pattern = .alignment
        case .medium: pattern = .generic
        case .strong: pattern = .levelChange
        }
        performer.perform(pattern, performanceTime: .now)
    }

    /// A heavier buzz, used for mute.
    public func buzz(_ strength: HapticStrength) {
        tick(.strong)
        if strength == .strong {
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.06) { [performer] in
                performer.perform(.levelChange, performanceTime: .now)
            }
        }
    }
}
