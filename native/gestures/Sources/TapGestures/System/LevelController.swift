import Foundation

/// Anything that can report and change a 0…1 system level.
public protocol LevelController: AnyObject {
    /// Current level, 0…1.
    var level: Double { get }
    /// Whether the underlying output is muted (brightness is never muted).
    var isMuted: Bool { get }
    /// Applies a level, clamped to 0…1.
    func setLevel(_ level: Double)
    /// Toggles mute where supported.
    func toggleMute()
    /// Re-reads the level from the system.
    func refresh()
}
