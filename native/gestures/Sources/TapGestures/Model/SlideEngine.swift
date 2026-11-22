import CoreGraphics
import Foundation

/// A snapshot of everything the gesture engine needs for one frame.
public struct EngineConfiguration: Equatable, Sendable {
    public var edges: EdgeMap = .factory
    public var corners: CornerMap = .factory
    public var sensitivity: Double = 1.0
    public var currentBrightness: Double = 0.5
    public var currentVolume: Double = 0.5
    public var mediaStripReverse: Bool = false
    public var requireModifier: ModifierRequirement = .none
    public var fineControlWithModifiers: Bool = true
    public var preciseSliding: Bool = false
    /// Seconds after a key during which a new touch cannot start a gesture. 0 turns this off.
    public var typingPause: Double = 0.4
    /// Largest major axis still treated as a fingertip. A wider contact is ignored. 0 turns this off.
    public var maxTouchSize: Double = 18

    public init() {}
}

/// Things the engine can ask the app to do.
public enum MediaCommand: Sendable, Equatable {
    case playPause
    case next
    case previous
    case mute
}

public protocol SlideEngineDelegate: AnyObject {
    func slideEngine(_ engine: SlideEngine, didBeginSlide target: EdgeAction)
    func slideEngine(_ engine: SlideEngine, didUpdateSlide target: EdgeAction, level: Double)
    func slideEngine(_ engine: SlideEngine, didEndSlide target: EdgeAction, level: Double)
    /// Fired once every time the level crosses a haptic step.
    func slideEngine(_ engine: SlideEngine, didTick target: EdgeAction, level: Double)
    func slideEngine(_ engine: SlideEngine, didRecognize command: MediaCommand)
    /// A corner click should open a Raycast command.
    func slideEngine(_ engine: SlideEngine, didOpenRaycast link: String)
    /// A single click is waiting out the double-click window.
    func slideEngineDidArmSingleTap(_ engine: SlideEngine)
    /// A finger has rested in a corner long enough to tap a person.
    func slideEngine(_ engine: SlideEngine, didBeginHold corner: Corner)
    /// The finger left the corner, or the hold ended.
    func slideEngine(_ engine: SlideEngine, didEndHold corner: Corner)
}

/// Turns raw contact frames into edge-slide gestures.
///
/// The engine is deliberately free of AppKit: it is fed `TouchFrame`s and
/// emits semantic events, which makes the messy part unit-testable.
public final class SlideEngine {
    public weak var delegate: SlideEngineDelegate?

    /// Live configuration (levels are read when a slide begins).
    public var configuration: () -> EngineConfiguration = { EngineConfiguration() }
    /// Live modifier state, used for the "require modifier" and fine-control options.
    public var modifiers: () -> ModifierState = { .current }
    /// Seconds since the last key-down. Idle keyboards report a large value.
    public var keyAge: () -> TimeInterval = { .infinity }

    private enum Kind {
        case slider(target: EdgeAction, edge: Edge, startLevel: Double)
        case media(Edge)
        case corner(Corner)
        case ignored
    }

    private struct Gesture {
        var kind: Kind
        var startX: Double
        var startY: Double
        var startTime: TimeInterval
        var lastX: Double
        var lastY: Double
        var lastSeen: TimeInterval
        var moved: Bool
        var emittedLevel: Double
        var lastStep: Int
        var consumed: Bool
        var lostFrames: Int
        var holdStarted: Bool
        var fingerID: Int32
    }

    private var gesture: Gesture?
    /// Contacts that landed as a palm, or while typing. Ignored until they lift.
    private var suppressed: Set<Int32> = []
    private var lastCornerTap: [Corner: TimeInterval] = [:]
    private var pendingSingle: ClickAction?
    private var pendingRaycastLink = ""

    public init() {}

    /// True while a finger is being tracked (used to keep the cursor frozen).
    public var isTrackingSlide: Bool {
        guard let gesture else { return false }
        if case .slider = gesture.kind { return gesture.moved }
        return false
    }

    // MARK: - Frame processing

    public func process(_ frame: TouchFrame) {
        let config = configuration()
        let active = frame.activeContacts
        suppressed.formIntersection(Set(active.map(\.id)))
        for contact in active where gesture?.fingerID != contact.id && isPalm(contact, config: config) {
            suppressed.insert(contact.id)
        }

        if var current = gesture {
            guard let point = active.first(where: { $0.id == current.fingerID }) else {
                current.lostFrames += 1
                // A tap ends the moment the finger lifts; a slide survives a
                // few dropped frames so it does not stop halfway (1.8 fix).
                if !current.moved || current.lostFrames > 5 || frame.timestamp - current.lastSeen > 0.12 {
                    finish(current, at: frame.timestamp, config: config)
                    gesture = nil
                }
                return
            }

            if shouldAbortForTyping(current, config: config) {
                for contact in active { suppressed.insert(contact.id) }
                cancel(current)
                gesture = nil
                return
            }

            let extras = active.filter { $0.id != current.fingerID && !suppressed.contains($0.id) }
            guard extras.isEmpty else {
                // Extra fingers mean the user wants a system gesture.
                cancel(current)
                gesture = nil
                return
            }

            current.lostFrames = 0
            current.lastX = point.x
            current.lastY = point.y
            current.lastSeen = frame.timestamp
            update(&current, point: CGPoint(x: point.x, y: point.y), now: frame.timestamp, config: config)
            gesture = current
            return
        }

        if isTyping(config) {
            for contact in active { suppressed.insert(contact.id) }
            return
        }

        let fingers = active.filter { !suppressed.contains($0.id) }
        guard fingers.count == 1, let point = fingers.first else { return }
        begin(contact: point, now: frame.timestamp, config: config)
    }

    /// Advances a resting corner hold when the trackpad stops sending frames.
    public func poke(at now: TimeInterval) {
        guard var current = gesture, case .corner = current.kind, !current.holdStarted else { return }
        update(&current, point: CGPoint(x: current.lastX, y: current.lastY), now: now, config: configuration())
        gesture = current
    }

    /// Drops any in-flight gesture (e.g. the app was disabled).
    public func reset() {
        if let current = gesture { cancel(current) }
        gesture = nil
        pendingSingle = nil
        pendingRaycastLink = ""
        suppressed.removeAll()
    }

    /// Fires a single click once the double-click window has passed.
    /// Returns nil when a second tap already consumed it.
    public func takePendingSingle() -> (ClickAction, String)? {
        defer {
            pendingSingle = nil
            pendingRaycastLink = ""
        }
        guard let action = pendingSingle else { return nil }
        return (action, pendingRaycastLink)
    }

    // MARK: - Gesture lifecycle

    private func begin(contact: TouchContact, now: TimeInterval, config: EngineConfiguration) {
        let point = CGPoint(x: contact.x, y: contact.y)
        guard ModifierState.modifierRequirementSatisfied(config.requireModifier, by: modifiers()) else {
            gesture = makeGesture(kind: .ignored, contact: contact, point: point, now: now, level: 0, fine: false, config: config)
            return
        }

        let geometry = EdgeGeometry(edges: config.edges)
        let fine = isFine(config: config)

        if let corner = geometry.corner(at: point),
           config.corners[corner].hasAction || config.corners[corner].holdsSomeone {
            gesture = makeGesture(kind: .corner(corner), contact: contact, point: point, now: now, level: 0, fine: fine, config: config)
            return
        }

        guard let edge = geometry.edge(at: point) else {
            gesture = makeGesture(kind: .ignored, contact: contact, point: point, now: now, level: 0, fine: false, config: config)
            return
        }

        gesture = makeGesture(
            kind: kind(for: edge, config: config),
            contact: contact,
            point: point,
            now: now,
            level: level(for: edge, config: config),
            fine: fine,
            config: config
        )
    }

    private func kind(for edge: Edge, config: EngineConfiguration) -> Kind {
        let sensor = config.edges[edge]
        switch sensor.action {
        case .media:
            return .media(edge)
        case .volume, .brightness:
            guard let target = sensor.action.edgeAction else { return .ignored }
            return .slider(target: target, edge: edge, startLevel: level(for: target, config: config))
        }
    }

    private func level(for edge: Edge, config: EngineConfiguration) -> Double {
        guard let target = config.edges[edge].action.edgeAction else { return 0 }
        return level(for: target, config: config)
    }

    private func level(for target: EdgeAction, config: EngineConfiguration) -> Double {
        target == .volume ? config.currentVolume : config.currentBrightness
    }

    private func update(_ gesture: inout Gesture, point: CGPoint, now: TimeInterval, config: EngineConfiguration) {
        let dx = point.x - gesture.startX
        let dy = point.y - gesture.startY

        // A click that starts moving becomes the slide underneath the corner,
        // but only when the finger is actually inside that band. A narrowed
        // band leaves the corner alone, so the tap or hold still happens.
        if case .corner = gesture.kind, !gesture.holdStarted, hypot(dx, dy) > 0.012,
           let edge = edgeForSlide(from: point, dx: dx, dy: dy, config: config) {
            gesture.kind = kind(for: edge, config: config)
            let level = level(for: edge, config: config)
            let steps = isFine(config: config) ? 64.0 : 16.0
            gesture.emittedLevel = (level * steps).rounded() / steps
            gesture.lastStep = Int((level * steps).rounded())
            update(&gesture, point: point, now: now, config: config)
            return
        }

        switch gesture.kind {
        case let .slider(target, edge, startLevel):
            let delta = edge.isVertical ? dy : dx
            guard abs(delta) > 0.006 else { return }
            if !gesture.moved {
                gesture.moved = true
                delegate?.slideEngine(self, didBeginSlide: target)
            }

            let geometry = EdgeGeometry(edges: config.edges)
            let span = geometry.travelSpan(for: edge)
            let raw = startLevel + (delta / span) * config.sensitivity
            let clamped = min(max(raw, 0), 1)

            let steps = isFine(config: config) ? 64.0 : 16.0
            let snapped = (clamped * steps).rounded() / steps

            if snapped != gesture.emittedLevel {
                gesture.emittedLevel = snapped
                delegate?.slideEngine(self, didUpdateSlide: target, level: snapped)
            }

            let step = Int((snapped * steps).rounded())
            if step != gesture.lastStep {
                gesture.lastStep = step
                delegate?.slideEngine(self, didTick: target, level: snapped)
            }

        case let .media(edge):
            guard config.edges[edge].enabled, config.edges[edge].action == .media else { return }
            let delta = edge.isVertical ? dy : dx
            if abs(delta) > 0.02 { gesture.moved = true }
            guard abs(delta) > 0.12, !gesture.consumed else { return }
            gesture.consumed = true
            // Swiping toward the left, or up a side edge, means "next".
            var forward = edge.isVertical ? delta > 0 : delta < 0
            if config.mediaStripReverse { forward.toggle() }
            delegate?.slideEngine(self, didRecognize: forward ? .next : .previous)

        case let .corner(corner):
            let travel = hypot(dx, dy)
            let elapsed = now - gesture.startTime
            if !gesture.holdStarted, elapsed >= 0.30, travel < 0.03 {
                gesture.holdStarted = true
                delegate?.slideEngine(self, didBeginHold: corner)
            }

        case .ignored:
            break
        }
    }

    /// The band a corner drag should become. The stronger axis wins when the
    /// corner sits in two bands. A band the finger is outside of does not count.
    private func edgeForSlide(from point: CGPoint, dx: Double, dy: Double, config: EngineConfiguration) -> Edge? {
        let geometry = EdgeGeometry(edges: config.edges)
        let vertical: Edge = point.x < 0.5 ? .left : .right
        let horizontal: Edge = point.y < 0.5 ? .bottom : .top
        let ordered = abs(dy) >= abs(dx) ? [vertical, horizontal] : [horizontal, vertical]
        return ordered.first { geometry.contains(point, edge: $0) }
    }

    private func finish(_ gesture: Gesture, at now: TimeInterval, config: EngineConfiguration) {
        let duration = now - gesture.startTime
        let travel = hypot(gesture.lastX - gesture.startX, gesture.lastY - gesture.startY)
        let isTap = !gesture.moved && duration < 0.4 && travel < 0.03

        switch gesture.kind {
        case let .slider(target, _, _):
            if gesture.moved {
                delegate?.slideEngine(self, didEndSlide: target, level: gesture.emittedLevel)
            }

        case .media:
            break

        case let .corner(corner):
            if gesture.holdStarted {
                delegate?.slideEngine(self, didEndHold: corner)
                break
            }
            guard isTap else { break }
            let actions = config.corners[corner]
            if let previous = lastCornerTap[corner], now - previous < 0.45 {
                lastCornerTap[corner] = -10
                pendingSingle = nil
                pendingRaycastLink = ""
                if actions.double == .raycast {
                    let link = actions.raycastDouble
                    if link.hasPrefix("raycast://") {
                        delegate?.slideEngine(self, didOpenRaycast: link)
                    }
                } else if let command = actions.double.command {
                    delegate?.slideEngine(self, didRecognize: command)
                }
            } else {
                lastCornerTap[corner] = now
                if actions.single == .raycast {
                    let link = actions.raycastSingle
                    if link.hasPrefix("raycast://") {
                        pendingSingle = .raycast
                        pendingRaycastLink = link
                        delegate?.slideEngineDidArmSingleTap(self)
                    }
                } else if actions.single != .none {
                    pendingSingle = actions.single
                    pendingRaycastLink = ""
                    delegate?.slideEngineDidArmSingleTap(self)
                }
            }

        case .ignored:
            break
        }
    }

    private func cancel(_ gesture: Gesture) {
        if case let .slider(target, _, _) = gesture.kind, gesture.moved {
            delegate?.slideEngine(self, didEndSlide: target, level: gesture.emittedLevel)
        }
        if case let .corner(corner) = gesture.kind, gesture.holdStarted {
            delegate?.slideEngine(self, didEndHold: corner)
        }
    }

    // MARK: - Helpers

    private func isTyping(_ config: EngineConfiguration) -> Bool {
        config.typingPause > 0 && keyAge() < config.typingPause
    }

    /// A slide that has already moved keeps going. A resting touch does not become a click or a hold.
    private func shouldAbortForTyping(_ gesture: Gesture, config: EngineConfiguration) -> Bool {
        isTyping(config) && !gesture.moved && !gesture.holdStarted
    }

    private func isPalm(_ contact: TouchContact, config: EngineConfiguration) -> Bool {
        config.maxTouchSize > 0 && contact.majorAxis > config.maxTouchSize
    }

    private func makeGesture(
        kind: Kind,
        contact: TouchContact,
        point: CGPoint,
        now: TimeInterval,
        level: Double,
        fine: Bool,
        config: EngineConfiguration
    ) -> Gesture {
        let steps = fine ? 64.0 : 16.0
        return Gesture(
            kind: kind,
            startX: point.x,
            startY: point.y,
            startTime: now,
            lastX: point.x,
            lastY: point.y,
            lastSeen: now,
            moved: false,
            emittedLevel: (level * steps).rounded() / steps,
            lastStep: Int((level * steps).rounded()),
            consumed: false,
            lostFrames: 0,
            holdStarted: false,
            fingerID: contact.id
        )
    }

    private func isFine(config: EngineConfiguration) -> Bool {
        if config.preciseSliding { return true }
        guard config.fineControlWithModifiers else { return false }
        let state = modifiers()
        return state.contains(.shift) && state.contains(.option)
    }
}

private extension ModifierState {
    /// Whether the user is holding whatever the setting asks for.
    static func modifierRequirementSatisfied(_ requirement: ModifierRequirement, by available: ModifierState) -> Bool {
        switch requirement {
        case .none: return true
        case .fn: return available.contains(.fn)
        case .option: return available.contains(.option)
        case .control: return available.contains(.control)
        }
    }
}
