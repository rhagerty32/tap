import CoreGraphics
import Foundation

/// One finger reported by the trackpad.
public struct TouchContact: Sendable, Equatable {
    /// Stable for the lifetime of a touch.
    public var id: Int32
    /// 0 = left, 1 = right.
    public var x: Double
    /// 0 = closest to you, 1 = closest to the screen.
    public var y: Double
    public var isTouching: Bool
    /// Contact area reported by the trackpad. Unused by the gesture rules.
    public var size: Double
    /// Long axis of the contact. A fingertip is much smaller than a palm.
    public var majorAxis: Double
    public var minorAxis: Double
    public var density: Double

    public init(
        id: Int32,
        x: Double,
        y: Double,
        isTouching: Bool,
        size: Double = 0,
        majorAxis: Double = 0,
        minorAxis: Double = 0,
        density: Double = 0
    ) {
        self.id = id
        self.x = x
        self.y = y
        self.isTouching = isTouching
        self.size = size
        self.majorAxis = majorAxis
        self.minorAxis = minorAxis
        self.density = density
    }
}

/// All contacts seen in one device frame.
public struct TouchFrame: Sendable {
    public var timestamp: TimeInterval
    public var contacts: [TouchContact]

    public init(timestamp: TimeInterval, contacts: [TouchContact]) {
        self.timestamp = timestamp
        self.contacts = contacts
    }

    public var activeContacts: [TouchContact] {
        contacts.filter(\.isTouching)
    }
}

/// Keyboard modifiers, read without touching the event stream.
public struct ModifierState: OptionSet, Sendable {
    public let rawValue: UInt

    public init(rawValue: UInt) {
        self.rawValue = rawValue
    }

    public static let shift = ModifierState(rawValue: 1 << 0)
    public static let control = ModifierState(rawValue: 1 << 1)
    public static let option = ModifierState(rawValue: 1 << 2)
    public static let command = ModifierState(rawValue: 1 << 3)
    public static let fn = ModifierState(rawValue: 1 << 4)

    /// Reads the current session flags. Safe from any thread.
    public static var current: ModifierState {
        let flags = CGEventSource.flagsState(.combinedSessionState)
        var state: ModifierState = []
        if flags.contains(.maskShift) { state.insert(.shift) }
        if flags.contains(.maskControl) { state.insert(.control) }
        if flags.contains(.maskAlternate) { state.insert(.option) }
        if flags.contains(.maskCommand) { state.insert(.command) }
        if flags.contains(.maskSecondaryFn) { state.insert(.fn) }
        return state
    }
}
