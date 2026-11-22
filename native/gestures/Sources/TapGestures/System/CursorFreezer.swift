import AppKit
import CoreGraphics

/// Keeps the pointer where it was while the user slides along an edge.
///
/// The trackpad still reports the finger movement to the system (which would
/// normally move the cursor), so we capture the pointer position at the start of
/// a slide and warp it back on every frame. Warping is used rather than
/// `CGAssociateMouseAndMouseCursorPosition(false)` so a crash can never leave the
/// mouse frozen.
public final class CursorFreezer {
    private var anchor: CGPoint?
    private var hidden = false

    public init() {}

    public func begin() {
        guard anchor == nil else { return }
        anchor = CGEvent(source: nil)?.location
        if CGDisplayHideCursor(CGMainDisplayID()) == .success {
            hidden = true
        }
    }

    /// Called on every touch frame while a slide is active.
    public func hold() {
        guard let anchor else { return }
        CGWarpMouseCursorPosition(anchor)
    }

    public func end() {
        guard anchor != nil else { return }
        anchor = nil
        if hidden {
            CGDisplayShowCursor(CGMainDisplayID())
            hidden = false
        }
    }
}
