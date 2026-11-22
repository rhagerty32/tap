import Foundation

/// What a trackpad edge controls.
public enum EdgeAction: String, Codable, CaseIterable, Sendable, Identifiable {
    case volume
    case brightness

    public var id: String { rawValue }

    public var label: String {
        switch self {
        case .volume: return "Volume"
        case .brightness: return "Brightness"
        }
    }

    public var symbolName: String {
        switch self {
        case .volume: return "speaker.wave.2.fill"
        case .brightness: return "sun.max.fill"
        }
    }

    public var toggled: EdgeAction {
        self == .volume ? .brightness : .volume
    }
}

public enum ModifierRequirement: String, Codable, CaseIterable, Sendable, Identifiable {
    case none
    case fn
    case option
    case control

    public var id: String { rawValue }

    public var label: String {
        switch self {
        case .none: return "None"
        case .fn: return "FN"
        case .option: return "Option"
        case .control: return "Control"
        }
    }
}

public enum HapticStrength: String, Codable, CaseIterable, Sendable, Identifiable {
    case light
    case medium
    case strong

    public var id: String { rawValue }

    public var label: String {
        switch self {
        case .light: return "Light"
        case .medium: return "Medium"
        case .strong: return "Strong"
        }
    }
}

/// What a slide along an edge does.
public enum SlideAction: String, Codable, CaseIterable, Sendable, Identifiable {
    case volume
    case brightness
    case media

    public var id: String { rawValue }

    public var label: String {
        switch self {
        case .volume: return "Volume"
        case .brightness: return "Brightness"
        case .media: return "Media"
        }
    }

    public var edgeAction: EdgeAction? {
        switch self {
        case .volume: return .volume
        case .brightness: return .brightness
        case .media: return nil
        }
    }

    public init(_ action: EdgeAction) {
        switch action {
        case .volume: self = .volume
        case .brightness: self = .brightness
        }
    }
}

/// One click in a trackpad corner.
public enum ClickAction: String, Codable, CaseIterable, Sendable, Identifiable {
    case none
    case mute
    case playPause
    case next
    case previous

    public var id: String { rawValue }

    public var label: String {
        switch self {
        case .none: return "None"
        case .mute: return "Mute"
        case .playPause: return "Play / pause"
        case .next: return "Next track"
        case .previous: return "Previous track"
        }
    }

    public var command: MediaCommand? {
        switch self {
        case .none: return nil
        case .mute: return .mute
        case .playPause: return .playPause
        case .next: return .next
        case .previous: return .previous
        }
    }
}

/// A draggable band along one edge of the trackpad.
///
/// `start` and `end` run from 0 to 1 along the edge. On the sides, 0 is the
/// edge nearest you. On the top and bottom, 0 is the left.
public struct EdgeSensor: Codable, Equatable, Sendable {
    public var enabled: Bool
    public var action: SlideAction
    public var start: Double
    public var end: Double
    public var thickness: Double

    public init(enabled: Bool, action: SlideAction, start: Double, end: Double, thickness: Double) {
        self.enabled = enabled
        self.action = action
        self.start = min(max(start, 0), 1)
        self.end = min(max(end, self.start), 1)
        self.thickness = min(max(thickness, 0.05), 0.45)
    }

    public static func full(action: SlideAction, enabled: Bool = true, thickness: Double = 0.18) -> EdgeSensor {
        EdgeSensor(enabled: enabled, action: action, start: 0, end: 1, thickness: thickness)
    }
}

public struct EdgeMap: Codable, Equatable, Sendable {
    public var left: EdgeSensor
    public var right: EdgeSensor
    public var top: EdgeSensor
    public var bottom: EdgeSensor

    public init(left: EdgeSensor, right: EdgeSensor, top: EdgeSensor, bottom: EdgeSensor) {
        self.left = left
        self.right = right
        self.top = top
        self.bottom = bottom
    }

    public static var factory: EdgeMap {
        EdgeMap(
            left: .full(action: .brightness),
            right: .full(action: .volume),
            top: .full(action: .media, enabled: false),
            bottom: .full(action: .media)
        )
    }

    public subscript(edge: Edge) -> EdgeSensor {
        get {
            switch edge {
            case .left: return left
            case .right: return right
            case .top: return top
            case .bottom: return bottom
            }
        }
        set {
            switch edge {
            case .left: left = newValue
            case .right: right = newValue
            case .top: top = newValue
            case .bottom: bottom = newValue
            }
        }
    }

    /// True when some enabled band posts media keys.
    public var usesMediaKeys: Bool {
        Edge.allCases.contains { self[$0].enabled && self[$0].action == .media }
    }
}

/// Single- and double-click actions for one corner.
public struct CornerActions: Codable, Equatable, Sendable {
    public var single: ClickAction
    public var double: ClickAction
    /// Peer id held while a finger rests in this corner. Empty means the corner does not tap anyone.
    public var holdPeer: String

    public init(single: ClickAction = .none, double: ClickAction = .none, holdPeer: String = "") {
        self.single = single
        self.double = double
        self.holdPeer = holdPeer
    }

    private enum CodingKeys: String, CodingKey {
        case single, double, holdPeer
    }

    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        single = try c.decodeIfPresent(ClickAction.self, forKey: .single) ?? .none
        double = try c.decodeIfPresent(ClickAction.self, forKey: .double) ?? .none
        holdPeer = try c.decodeIfPresent(String.self, forKey: .holdPeer) ?? ""
    }

    public func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: CodingKeys.self)
        try c.encode(single, forKey: .single)
        try c.encode(double, forKey: .double)
        try c.encode(holdPeer, forKey: .holdPeer)
    }

    public var hasAction: Bool { single != .none || double != .none }

    public var holdsSomeone: Bool { !holdPeer.isEmpty }

    public var usesMediaKeys: Bool {
        single.command.map { $0 != .mute } == true || double.command.map { $0 != .mute } == true
    }
}

public enum Corner: String, Codable, CaseIterable, Sendable, Identifiable {
    case topLeft
    case topRight
    case bottomLeft
    case bottomRight

    public var id: String { rawValue }

    public var label: String {
        switch self {
        case .topLeft: return "Top left"
        case .topRight: return "Top right"
        case .bottomLeft: return "Bottom left"
        case .bottomRight: return "Bottom right"
        }
    }
}

public struct CornerMap: Codable, Equatable, Sendable {
    public var topLeft: CornerActions
    public var topRight: CornerActions
    public var bottomLeft: CornerActions
    public var bottomRight: CornerActions

    public init(
        topLeft: CornerActions,
        topRight: CornerActions,
        bottomLeft: CornerActions,
        bottomRight: CornerActions
    ) {
        self.topLeft = topLeft
        self.topRight = topRight
        self.bottomLeft = bottomLeft
        self.bottomRight = bottomRight
    }

    public static var factory: CornerMap {
        CornerMap(
            topLeft: CornerActions(),
            topRight: CornerActions(double: .mute),
            bottomLeft: CornerActions(),
            bottomRight: CornerActions()
        )
    }

    public subscript(corner: Corner) -> CornerActions {
        get {
            switch corner {
            case .topLeft: return topLeft
            case .topRight: return topRight
            case .bottomLeft: return bottomLeft
            case .bottomRight: return bottomRight
            }
        }
        set {
            switch corner {
            case .topLeft: topLeft = newValue
            case .topRight: topRight = newValue
            case .bottomLeft: bottomLeft = newValue
            case .bottomRight: bottomRight = newValue
            }
        }
    }

    public var usesMediaKeys: Bool {
        Corner.allCases.contains { self[$0].usesMediaKeys }
    }
}

public enum AppearanceMode: String, Codable, CaseIterable, Sendable, Identifiable {
    case system
    case light
    case dark

    public var id: String { rawValue }

    public var label: String {
        switch self {
        case .system: return "System"
        case .light: return "Light"
        case .dark: return "Dark"
        }
    }
}

/// Everything the user can change. Persisted as JSON in `UserDefaults`.
public struct Settings: Codable, Equatable, Sendable {
    /// Master switch – stops the touch pipeline when off.
    public var enabled: Bool = true

    // Edges
    public var edges: EdgeMap = .factory
    public var corners: CornerMap = .factory
    /// Multiplies how far a slide has to travel to cover the full range.
    public var sensitivity: Double = 1.0
    /// Require a modifier key to be held before an edge starts responding.
    public var requireModifier: ModifierRequirement = .none
    /// Some trackpads report the vertical axis the other way round.
    public var flipVerticalEdges: Bool = false

    // Feel
    public var freezeCursor: Bool = true
    public var preciseSliding: Bool = false
    public var fineControlWithModifiers: Bool = true
    public var hapticsEnabled: Bool = true
    public var hapticStrength: HapticStrength = .medium

    /// Flips next and previous for a media slide.
    public var mediaStripReverse: Bool = false

    // Appearance
    public var showHUD: Bool = true
    public var appearance: AppearanceMode = .system
    public var openAtLogin: Bool = false

    /// Set once the onboarding assistant has been shown.
    public var hasCompletedSetup: Bool = false

    /// Poll GitHub releases once a day for a newer build.
    public var automaticallyChecksForUpdates: Bool = true

    public init() {}

    /// Codable with forgiving decoding: missing keys fall back to the default
    /// value so old settings files keep working after an upgrade.
    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        let d = Settings()
        enabled = try c.decodeIfPresent(Bool.self, forKey: .enabled) ?? d.enabled
        edges = Self.migratedEdges(from: decoder, defaults: d)
        corners = Self.migratedCorners(from: decoder, defaults: d)
        sensitivity = try c.decodeIfPresent(Double.self, forKey: .sensitivity) ?? d.sensitivity
        requireModifier = try c.decodeIfPresent(ModifierRequirement.self, forKey: .requireModifier) ?? d.requireModifier
        flipVerticalEdges = try c.decodeIfPresent(Bool.self, forKey: .flipVerticalEdges) ?? d.flipVerticalEdges
        freezeCursor = try c.decodeIfPresent(Bool.self, forKey: .freezeCursor) ?? d.freezeCursor
        preciseSliding = try c.decodeIfPresent(Bool.self, forKey: .preciseSliding) ?? d.preciseSliding
        fineControlWithModifiers = try c.decodeIfPresent(Bool.self, forKey: .fineControlWithModifiers) ?? d.fineControlWithModifiers
        hapticsEnabled = try c.decodeIfPresent(Bool.self, forKey: .hapticsEnabled) ?? d.hapticsEnabled
        hapticStrength = try c.decodeIfPresent(HapticStrength.self, forKey: .hapticStrength) ?? d.hapticStrength
        mediaStripReverse = try c.decodeIfPresent(Bool.self, forKey: .mediaStripReverse) ?? d.mediaStripReverse
        showHUD = try c.decodeIfPresent(Bool.self, forKey: .showHUD) ?? d.showHUD
        appearance = try c.decodeIfPresent(AppearanceMode.self, forKey: .appearance) ?? d.appearance
        openAtLogin = try c.decodeIfPresent(Bool.self, forKey: .openAtLogin) ?? d.openAtLogin
        hasCompletedSetup = try c.decodeIfPresent(Bool.self, forKey: .hasCompletedSetup) ?? d.hasCompletedSetup
        automaticallyChecksForUpdates = try c.decodeIfPresent(Bool.self, forKey: .automaticallyChecksForUpdates)
            ?? d.automaticallyChecksForUpdates
    }

    /// Volume or brightness for an enabled side, when that is what the edge slides.
    public func action(for edge: Edge) -> EdgeAction? {
        let sensor = edges[edge]
        guard sensor.enabled else { return nil }
        return sensor.action.edgeAction
    }

    /// Compatibility with the previous two-side layout.
    public var leftEdge: EdgeAction {
        get { edges.left.action.edgeAction ?? .brightness }
        set { edges.left.action = SlideAction(newValue) }
    }

    public var rightEdge: EdgeAction {
        get { edges.right.action.edgeAction ?? .volume }
        set { edges.right.action = SlideAction(newValue) }
    }

    public var edgeWidth: Double {
        get { edges.left.thickness }
        set {
            for edge in Edge.allCases {
                edges[edge].thickness = newValue
            }
        }
    }

    public var bottomQuarterOnly: Bool {
        get {
            [edges.left, edges.right].allSatisfy { $0.start <= 0.02 && abs($0.end - 0.25) < 0.04 }
        }
        set {
            edges.left.start = 0
            edges.right.start = 0
            edges.left.end = newValue ? 0.25 : 1
            edges.right.end = newValue ? 0.25 : 1
        }
    }

    public var quickMuteEnabled: Bool {
        get { corners.topRight.double == .mute }
        set { corners.topRight.double = newValue ? .mute : .none }
    }

    public var mediaStripEnabled: Bool {
        get { edges.bottom.enabled && edges.bottom.action == .media }
        set {
            edges.bottom.enabled = newValue
            if newValue { edges.bottom.action = .media }
        }
    }

    public var tapToPlayEnabled: Bool {
        get {
            corners.bottomLeft.double == .playPause || corners.bottomRight.double == .playPause
        }
        set {
            let action: ClickAction = newValue ? .playPause : .none
            if newValue || corners.bottomLeft.double == .playPause {
                corners.bottomLeft.double = action
            }
            if newValue || corners.bottomRight.double == .playPause {
                corners.bottomRight.double = action
            }
        }
    }

    public var swapped: Bool {
        get { leftEdge == .volume }
        set {
            leftEdge = newValue ? .volume : .brightness
            rightEdge = newValue ? .brightness : .volume
        }
    }

    /// Media keys are posted for a media slide or a corner click other than mute.
    public var usesMediaKeys: Bool {
        edges.usesMediaKeys || corners.usesMediaKeys
    }

    private static func migratedEdges(from decoder: Decoder, defaults: Settings) -> EdgeMap {
        if let current = try? decoder.container(keyedBy: CodingKeys.self),
           let edges = try? current.decodeIfPresent(EdgeMap.self, forKey: .edges) {
            return edges
        }
        guard let legacy = try? decoder.container(keyedBy: LegacyKey.self) else {
            return defaults.edges
        }
        var edges = defaults.edges
        if let width = try? legacy.decodeIfPresent(Double.self, forKey: .edgeWidth) {
            for edge in Edge.allCases {
                edges[edge].thickness = width
            }
        }
        if let left = try? legacy.decodeIfPresent(EdgeAction.self, forKey: .leftEdge) {
            edges.left.action = SlideAction(left)
        }
        if let right = try? legacy.decodeIfPresent(EdgeAction.self, forKey: .rightEdge) {
            edges.right.action = SlideAction(right)
        }
        if (try? legacy.decodeIfPresent(Bool.self, forKey: .bottomQuarterOnly)) == true {
            edges.left.start = 0
            edges.left.end = 0.25
            edges.right.start = 0
            edges.right.end = 0.25
        }
        if let media = try? legacy.decodeIfPresent(Bool.self, forKey: .mediaStripEnabled) {
            edges.bottom.enabled = media
            if media { edges.bottom.action = .media }
        }
        return edges
    }

    private static func migratedCorners(from decoder: Decoder, defaults: Settings) -> CornerMap {
        if let current = try? decoder.container(keyedBy: CodingKeys.self),
           let corners = try? current.decodeIfPresent(CornerMap.self, forKey: .corners) {
            return corners
        }
        guard let legacy = try? decoder.container(keyedBy: LegacyKey.self) else {
            return defaults.corners
        }
        var corners = defaults.corners
        if let mute = try? legacy.decodeIfPresent(Bool.self, forKey: .quickMuteEnabled) {
            corners.topRight.double = mute ? .mute : .none
        }
        if (try? legacy.decodeIfPresent(Bool.self, forKey: .tapToPlayEnabled)) == true {
            corners.bottomLeft.double = .playPause
            corners.bottomRight.double = .playPause
        }
        return corners
    }

    private enum LegacyKey: String, CodingKey {
        case leftEdge, rightEdge, edgeWidth, bottomQuarterOnly
        case quickMuteEnabled, mediaStripEnabled, tapToPlayEnabled
    }
}

