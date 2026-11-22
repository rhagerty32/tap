import AppKit
import QuartzCore

/// Media glyphs the glass HUD can show instead of a level bar.
enum MediaSymbol {
    case playPause
    case next
    case previous

    var symbolName: String {
        switch self {
        case .playPause: return "playpause.fill"
        case .next: return "forward.fill"
        case .previous: return "backward.fill"
        }
    }
}

/// Proportions of the SlideSense on-screen display: a fixed capsule, an icon,
/// and a level bar with equal margins.
private enum HUDMetrics {
    static let panelWidth: CGFloat = 210
    static let panelHeight: CGFloat = 60
    static var panelSize: NSSize { NSSize(width: panelWidth, height: panelHeight) }
    static let cornerRadius: CGFloat = 16
    static let contentInset: CGFloat = 14
    static let iconBoxWidth: CGFloat = 22
    static let iconBoxHeight: CGFloat = 22
    static let gap: CGFloat = 12
    static let barHeight: CGFloat = 8
    static let barCornerRadius: CGFloat = 4
    static let minimumVisibleLevel: Double = 0.002
    static let mediaIconWidth: CGFloat = 40
    static let mediaIconHeight: CGFloat = 30
    static let levelSymbolPointSize: CGFloat = 17
    static let mediaSymbolPointSize: CGFloat = 22
    static let fillAnimationDuration: CFTimeInterval = 0.15
    static var barOriginX: CGFloat { contentInset + iconBoxWidth + gap }
}

/// Liquid glass on macOS 26, frosted HUD material before that.
private final class LevelHUDView: NSView {
    static let preferredSize = HUDMetrics.panelSize

    let backgroundView: NSView
    let overlay = NSView()
    let trackLayer = CALayer()
    let fillLayer = CALayer()
    let iconView = NSImageView(frame: .zero)

    private(set) var showsLevel = true
    private(set) var level: Double = 0
    private(set) var isMuted = false
    private var iconKey = ""
    private var currentContentColor: NSColor?
    private var currentMuted = false

    override init(frame frameRect: NSRect) {
        backgroundView = LevelHUDView.makeBackground()
        super.init(frame: frameRect)
        setUp()
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        fatalError("LevelHUDView is created in code only")
    }

    override var isOpaque: Bool { false }

    private static func makeBackground() -> NSView {
        #if compiler(>=6.2)
        if #available(macOS 26.0, *) {
            let glass = NSGlassEffectView()
            glass.cornerRadius = HUDMetrics.cornerRadius
            glass.style = .regular
            return glass
        }
        #endif
        let effect = NSVisualEffectView()
        effect.material = .hudWindow
        effect.blendingMode = .behindWindow
        effect.state = .active
        effect.isEmphasized = true
        effect.wantsLayer = true
        effect.layer?.cornerRadius = HUDMetrics.cornerRadius
        effect.layer?.cornerCurve = .continuous
        effect.layer?.masksToBounds = true
        return effect
    }

    private func setUp() {
        addSubview(backgroundView)
        overlay.wantsLayer = true

        var contentLivesInBackground = false
        #if compiler(>=6.2)
        if #available(macOS 26.0, *), let glass = backgroundView as? NSGlassEffectView {
            glass.contentView = overlay
            contentLivesInBackground = true
        }
        #endif
        if !contentLivesInBackground {
            addSubview(overlay)
        }

        trackLayer.cornerRadius = HUDMetrics.barCornerRadius
        overlay.layer?.addSublayer(trackLayer)
        fillLayer.anchorPoint = CGPoint(x: 0, y: 0.5)
        fillLayer.cornerRadius = HUDMetrics.barCornerRadius
        overlay.layer?.addSublayer(fillLayer)

        iconView.imageScaling = .scaleProportionallyUpOrDown
        iconView.imageAlignment = .alignCenter
        iconView.imageFrameStyle = .none
        overlay.addSubview(iconView)
        updateContentsScale()
    }

    func configure(edge: EdgeAction, level: Double, muted: Bool, animated: Bool) {
        showsLevel = true
        self.level = level
        isMuted = muted
        let color = resolvedContentColor()
        applyColors(color: color)
        applyIcon(name: muted ? "speaker.slash.fill" : edge.symbolName, pointSize: HUDMetrics.levelSymbolPointSize, color: color)
        withoutAnimations {
            trackLayer.isHidden = false
            fillLayer.isHidden = false
        }
        applyFill(animated: animated)
        needsLayout = true
    }

    func configure(media: MediaSymbol) {
        showsLevel = false
        let color = resolvedContentColor()
        applyColors(color: color)
        applyIcon(name: media.symbolName, pointSize: HUDMetrics.mediaSymbolPointSize, color: color)
        withoutAnimations {
            trackLayer.isHidden = true
            fillLayer.isHidden = true
        }
        needsLayout = true
    }

    private var barRect: CGRect {
        let x = HUDMetrics.barOriginX
        let width = max(0, bounds.width - x - HUDMetrics.contentInset)
        return CGRect(x: x, y: (bounds.height - HUDMetrics.barHeight) / 2, width: width, height: HUDMetrics.barHeight)
    }

    private var iconFrame: CGRect {
        if showsLevel {
            return CGRect(
                x: HUDMetrics.contentInset,
                y: (bounds.height - HUDMetrics.iconBoxHeight) / 2,
                width: HUDMetrics.iconBoxWidth,
                height: HUDMetrics.iconBoxHeight
            )
        }
        return CGRect(
            x: (bounds.width - HUDMetrics.mediaIconWidth) / 2,
            y: (bounds.height - HUDMetrics.mediaIconHeight) / 2,
            width: HUDMetrics.mediaIconWidth,
            height: HUDMetrics.mediaIconHeight
        )
    }

    private var fillWidth: CGFloat {
        guard showsLevel else { return 0 }
        let clamped = level.isFinite ? min(max(level, 0), 1) : 0
        guard clamped > HUDMetrics.minimumVisibleLevel else { return 0 }
        let width = barRect.width * clamped
        guard width > 0 else { return 0 }
        return max(HUDMetrics.barHeight, width)
    }

    override func layout() {
        super.layout()
        backgroundView.frame = bounds
        overlay.frame = bounds
        withoutAnimations {
            let bar = barRect
            trackLayer.frame = bar
            fillLayer.position = CGPoint(x: bar.minX, y: bar.midY)
            fillLayer.bounds = CGRect(x: 0, y: 0, width: fillWidth, height: HUDMetrics.barHeight)
            fillLayer.isHidden = !showsLevel || fillWidth <= 0
            iconView.frame = iconFrame
        }
    }

    override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        updateContentsScale()
    }

    override func viewDidChangeBackingProperties() {
        super.viewDidChangeBackingProperties()
        updateContentsScale()
    }

    private func updateContentsScale() {
        let scale = window?.backingScaleFactor ?? 2
        trackLayer.contentsScale = scale
        fillLayer.contentsScale = scale
    }

    private func applyFill(animated: Bool) {
        guard showsLevel else { return }
        let target = fillWidth
        let newBounds = CGRect(x: 0, y: 0, width: target, height: HUDMetrics.barHeight)
        let current = fillLayer.presentation()?.bounds.size.width ?? fillLayer.bounds.size.width
        fillLayer.removeAnimation(forKey: "fillWidth")
        withoutAnimations { fillLayer.bounds = newBounds }
        guard animated,
              !NSWorkspace.shared.accessibilityDisplayShouldReduceMotion,
              abs(current - target) > 0.5
        else { return }
        let animation = CABasicAnimation(keyPath: "bounds")
        animation.fromValue = NSValue(rect: CGRect(x: 0, y: 0, width: current, height: HUDMetrics.barHeight))
        animation.toValue = NSValue(rect: newBounds)
        animation.duration = HUDMetrics.fillAnimationDuration
        animation.timingFunction = CAMediaTimingFunction(name: .easeOut)
        animation.isRemovedOnCompletion = true
        fillLayer.add(animation, forKey: "fillWidth")
    }

    private func applyIcon(name: String, pointSize: CGFloat, color: NSColor) {
        let key = "\(name)|\(pointSize)|\(color.description)"
        guard key != iconKey else { return }
        iconKey = key
        iconView.contentTintColor = color
        guard let base = NSImage(systemSymbolName: name, accessibilityDescription: nil) else {
            iconView.image = nil
            return
        }
        let configuration = NSImage.SymbolConfiguration(pointSize: pointSize, weight: .medium)
            .applying(NSImage.SymbolConfiguration(paletteColors: [color]))
        iconView.image = base.withSymbolConfiguration(configuration) ?? base
    }

    private func resolvedContentColor() -> NSColor {
        var resolved = NSColor.labelColor
        backgroundView.effectiveAppearance.performAsCurrentDrawingAppearance {
            resolved = NSColor(cgColor: NSColor.labelColor.cgColor) ?? NSColor.labelColor
        }
        return resolved
    }

    private func applyColors(color: NSColor) {
        guard color != currentContentColor || isMuted != currentMuted else { return }
        currentContentColor = color
        currentMuted = isMuted
        let track = color.withAlphaComponent(0.18)
        let fill = isMuted ? color.withAlphaComponent(0.40) : color
        withoutAnimations {
            trackLayer.backgroundColor = track.cgColor
            fillLayer.backgroundColor = fill.cgColor
        }
    }

    private func withoutAnimations(_ body: () -> Void) {
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        body()
        CATransaction.commit()
    }
}

private final class HUDPanel: NSPanel {
    override var canBecomeKey: Bool { false }
    override var canBecomeMain: Bool { false }
}

/// The SlideSense capsule, drawn by AppKit so the background is real liquid glass.
final class LevelHUD {
    static let shared = LevelHUD()

    private var panel: HUDPanel?
    private var view: LevelHUDView?
    private var hideWorkItem: DispatchWorkItem?
    private var presentationToken: UInt = 0

    private init() {}

    func show(edge: EdgeAction, level: Double, muted: Bool) {
        onMain { [weak self] in
            guard let self else { return }
            let panel = self.ensurePanel()
            let visible = panel.isVisible
            self.view?.configure(edge: edge, level: level, muted: muted, animated: visible)
            self.view?.layoutSubtreeIfNeeded()
            self.position(panel)
            self.fadeIn(panel)
            self.scheduleAutoHide()
        }
    }

    func show(media: MediaSymbol) {
        onMain { [weak self] in
            guard let self else { return }
            let panel = self.ensurePanel()
            self.view?.configure(media: media)
            self.view?.layoutSubtreeIfNeeded()
            self.position(panel)
            self.fadeIn(panel)
            self.scheduleAutoHide()
        }
    }

    func dismiss() {
        onMain { [weak self] in
            self?.hide(immediately: true)
        }
    }

    private func ensurePanel() -> HUDPanel {
        if let panel { return panel }
        let size = LevelHUDView.preferredSize
        let panel = HUDPanel(
            contentRect: NSRect(origin: .zero, size: size),
            styleMask: [.borderless, .nonactivatingPanel],
            backing: .buffered,
            defer: false
        )
        panel.isFloatingPanel = true
        panel.becomesKeyOnlyIfNeeded = true
        panel.hidesOnDeactivate = false
        panel.worksWhenModal = true
        panel.level = .statusBar
        panel.collectionBehavior = [.canJoinAllSpaces, .stationary, .fullScreenAuxiliary, .ignoresCycle]
        panel.isOpaque = false
        panel.backgroundColor = .clear
        panel.hasShadow = true
        panel.ignoresMouseEvents = true
        panel.isMovable = false
        panel.isReleasedWhenClosed = false
        panel.isRestorable = false
        panel.isExcludedFromWindowsMenu = true
        panel.animationBehavior = .none
        let view = LevelHUDView(frame: NSRect(origin: .zero, size: size))
        panel.contentView = view
        self.panel = panel
        self.view = view
        return panel
    }

    private func position(_ panel: NSPanel) {
        let mouse = NSEvent.mouseLocation
        guard let screen = NSScreen.screens.first(where: { $0.frame.contains(mouse) }) ?? NSScreen.main else { return }
        let visible = screen.visibleFrame
        let size = LevelHUDView.preferredSize
        let target = NSRect(
            x: visible.maxX - size.width - 16,
            y: visible.maxY - size.height - 16,
            width: size.width,
            height: size.height
        )
        guard panel.frame != target else { return }
        panel.setFrame(target, display: false)
    }

    private func fadeIn(_ panel: NSPanel) {
        presentationToken &+= 1
        if !panel.isVisible {
            panel.alphaValue = 0
            panel.orderFrontRegardless()
        }
        guard panel.alphaValue < 1 else { return }
        if NSWorkspace.shared.accessibilityDisplayShouldReduceMotion {
            panel.alphaValue = 1
            return
        }
        NSAnimationContext.runAnimationGroup { context in
            context.duration = 0.15
            context.timingFunction = CAMediaTimingFunction(name: .easeOut)
            panel.animator().alphaValue = 1
        }
    }

    private func scheduleAutoHide() {
        hideWorkItem?.cancel()
        let item = DispatchWorkItem { [weak self] in
            self?.hideWorkItem = nil
            self?.hide(immediately: false)
        }
        hideWorkItem = item
        DispatchQueue.main.asyncAfter(deadline: .now() + 1.0, execute: item)
    }

    private func hide(immediately: Bool) {
        hideWorkItem?.cancel()
        hideWorkItem = nil
        presentationToken &+= 1
        guard let panel else { return }
        guard !immediately, panel.isVisible else {
            panel.orderOut(nil)
            panel.alphaValue = 0
            return
        }
        let token = presentationToken
        if NSWorkspace.shared.accessibilityDisplayShouldReduceMotion {
            panel.alphaValue = 0
            panel.orderOut(nil)
            return
        }
        NSAnimationContext.runAnimationGroup { context in
            context.duration = 0.20
            context.timingFunction = CAMediaTimingFunction(name: .easeIn)
            panel.animator().alphaValue = 0
        } completionHandler: { [weak self, weak panel] in
            guard let self, let panel, self.presentationToken == token else { return }
            panel.orderOut(nil)
            panel.alphaValue = 0
        }
    }

    private func onMain(_ body: @escaping () -> Void) {
        if Thread.isMainThread {
            body()
        } else {
            DispatchQueue.main.async(execute: body)
        }
    }
}
