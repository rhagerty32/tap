import CoreGraphics

/// The four edges of the trackpad, in normalised coordinates where
/// `x` goes left → right and `y` goes bottom → top (see `TrackpadMonitor`).
public enum Edge: String, Codable, CaseIterable, Sendable {
    case left
    case right
    case top
    case bottom

    public var label: String {
        switch self {
        case .left: return "Left edge"
        case .right: return "Right edge"
        case .top: return "Top edge"
        case .bottom: return "Bottom edge"
        }
    }

    /// Sides run up and down. The top and bottom run left and right.
    public var isVertical: Bool {
        switch self {
        case .left, .right: return true
        case .top, .bottom: return false
        }
    }
}

/// Pure geometry helpers. All input points are normalised to 0…1.
public struct EdgeGeometry: Sendable {
    public var edges: EdgeMap
    /// Size of each corner used for click actions.
    public var cornerSize: Double

    public init(edges: EdgeMap = .factory, cornerSize: Double = 0.16) {
        self.edges = edges
        self.cornerSize = cornerSize
    }

    /// Builds the previous uniform-band layout, used by tests that predate per-edge spans.
    public init(edgeWidth: Double = 0.18, bottomQuarterOnly: Bool = false, cornerSize: Double = 0.16) {
        var edges = EdgeMap.factory
        let width = min(max(edgeWidth, 0.05), 0.45)
        for edge in Edge.allCases {
            edges[edge].thickness = width
        }
        if bottomQuarterOnly {
            edges.left.end = 0.25
            edges.right.end = 0.25
        }
        edges.top.enabled = false
        self.init(edges: edges, cornerSize: cornerSize)
    }

    public init(settings: Settings) {
        self.init(edges: settings.edges)
    }

    /// Which enabled band contains the point. The closest outer edge wins a tie.
    public func edge(at point: CGPoint) -> Edge? {
        let x = Double(point.x)
        let y = Double(point.y)
        guard (0...1).contains(x), (0...1).contains(y) else { return nil }

        var best: (edge: Edge, distance: Double)?
        for edge in Edge.allCases {
            let sensor = edges[edge]
            guard sensor.enabled, contains(point, edge: edge, sensor: sensor) else { continue }
            let distance = distanceToOuterEdge(point, edge: edge)
            if best == nil || distance < best!.distance {
                best = (edge, distance)
            }
        }
        return best?.edge
    }

    /// A corner square, independent of the slide bands.
    public func corner(at point: CGPoint) -> Corner? {
        let x = Double(point.x)
        let y = Double(point.y)
        guard (0...1).contains(x), (0...1).contains(y) else { return nil }
        let left = x <= cornerSize
        let right = x >= 1 - cornerSize
        let bottom = y <= cornerSize
        let top = y >= 1 - cornerSize
        if left && top { return .topLeft }
        if right && top { return .topRight }
        if left && bottom { return .bottomLeft }
        if right && bottom { return .bottomRight }
        return nil
    }

    /// The top-right corner region used for Quick Mute.
    public func isQuickMuteCorner(_ point: CGPoint) -> Bool {
        corner(at: point) == .topRight
    }

    /// The span of trackpad travel that maps onto a full 0…1 change.
    public func travelSpan(for edge: Edge) -> Double {
        let sensor = edges[edge]
        let length = max(sensor.end - sensor.start, 0.2)
        return 0.75 * length
    }

    /// Position along an edge, 0 at the near/left end and 1 at the far/right end.
    public func along(_ point: CGPoint, edge: Edge) -> Double {
        switch edge {
        case .left, .right: return Double(point.y)
        case .top, .bottom: return Double(point.x)
        }
    }

    private func contains(_ point: CGPoint, edge: Edge, sensor: EdgeSensor) -> Bool {
        let position = along(point, edge: edge)
        guard position + 0.0001 >= sensor.start, position - 0.0001 <= sensor.end else { return false }
        let x = Double(point.x)
        let y = Double(point.y)
        switch edge {
        case .left: return x <= sensor.thickness
        case .right: return x >= 1 - sensor.thickness
        case .bottom: return y <= sensor.thickness
        case .top: return y >= 1 - sensor.thickness
        }
    }

    private func distanceToOuterEdge(_ point: CGPoint, edge: Edge) -> Double {
        switch edge {
        case .left: return Double(point.x)
        case .right: return 1 - Double(point.x)
        case .bottom: return Double(point.y)
        case .top: return 1 - Double(point.y)
        }
    }
}
