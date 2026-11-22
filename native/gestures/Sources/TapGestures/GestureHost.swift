import AppKit
import Foundation
import QuartzCore

/// C ABI used by the Tap process. The library owns the trackpad, the level
/// changes, and haptics. Tap owns the window that shows the level, and the
/// network hold that a corner rest starts.
private final class GestureHost: SlideEngineDelegate {
    static let shared = GestureHost()

    private let engine = SlideEngine()
    private let monitor = TrackpadMonitor()
    private let audio = AudioController()
    private let brightness = BrightnessController()
    private let freezer = CursorFreezer()
    /// Recursive: the engine reads configuration, which takes this lock, while a
    /// frame is already holding it.
    private let lock = NSRecursiveLock()

    private var settings = Settings()
    private var frozen = false
    private var singleTapToken = 0
    private var timer: Timer?
    private var lastDeviceTime: TimeInterval = 0
    private var lastWallTime: TimeInterval = 0
    private var started = false

    private var slideCallback: (@convention(c) (Int32, Int32, Double) -> Void)?
    private var commandCallback: (@convention(c) (Int32, Double, Int32) -> Void)?
    private var holdCallback: (@convention(c) (Int32, Int32) -> Void)?

    private init() {
        engine.delegate = self
        engine.configuration = { [weak self] in
            self?.engineConfiguration() ?? EngineConfiguration()
        }
        engine.modifiers = { ModifierState.current }
        monitor.onFrame = { [weak self] frame in
            self?.ingest(frame)
        }
    }

    func start(
        slide: @escaping @convention(c) (Int32, Int32, Double) -> Void,
        command: @escaping @convention(c) (Int32, Double, Int32) -> Void,
        hold: @escaping @convention(c) (Int32, Int32) -> Void
    ) -> Int32 {
        slideCallback = slide
        commandCallback = command
        holdCallback = hold
        started = true
        ensureTimer()
        applyRunningState()
        return monitorIsAvailable ? 0 : 2
    }

    func stop() {
        started = false
        lock.lock()
        engine.reset()
        lock.unlock()
        monitor.stop()
        freezer.end()
        frozen = false
        timer?.invalidate()
        timer = nil
    }

    func apply(json: String) -> Int32 {
        guard let data = json.data(using: .utf8),
              let decoded = try? JSONDecoder().decode(Settings.self, from: data) else {
            return 1
        }
        lock.lock()
        settings = decoded
        lock.unlock()
        monitor.flipVertical = decoded.flipVerticalEdges
        if decoded.usesMediaKeys {
            AccessibilityPermission.requestIfNeeded()
        }
        applyRunningState()
        return 0
    }

    private var monitorIsAvailable: Bool { TrackpadMonitor.isAvailable }

    private func applyRunningState() {
        guard started else { return }
        lock.lock()
        let enabled = settings.enabled
        lock.unlock()
        if enabled {
            if !monitor.isRunning {
                if !monitor.start() {
                    NSLog("Tap: no multitouch device found")
                }
            }
        } else {
            lock.lock()
            engine.reset()
            lock.unlock()
            monitor.stop()
            freezer.end()
            frozen = false
        }
    }

    private func ensureTimer() {
        DispatchQueue.main.async { [weak self] in
            guard let self, self.timer == nil else { return }
            let timer = Timer(timeInterval: 0.05, repeats: true) { [weak self] _ in
                self?.poke()
            }
            RunLoop.main.add(timer, forMode: .common)
            self.timer = timer
        }
    }

    private func poke() {
        lock.lock()
        let now = lastDeviceTime + (CACurrentMediaTime() - lastWallTime)
        engine.poke(at: now)
        let tracking = engine.isTrackingSlide
        lock.unlock()
        updateFreezer(tracking)
    }

    private func ingest(_ frame: TouchFrame) {
        lock.lock()
        let refreshLevels = !engine.isTrackingSlide && frame.activeContacts.count == 1
        lock.unlock()
        if refreshLevels {
            brightness.refresh()
            audio.refresh()
        }
        lastDeviceTime = frame.timestamp
        lastWallTime = CACurrentMediaTime()
        lock.lock()
        engine.process(frame)
        let tracking = engine.isTrackingSlide
        lock.unlock()
        updateFreezer(tracking)
    }

    private func updateFreezer(_ tracking: Bool) {
        lock.lock()
        let freeze = settings.freezeCursor
        lock.unlock()
        if tracking {
            if !frozen {
                frozen = true
                if freeze { freezer.begin() }
            }
            if freeze { freezer.hold() }
        } else if frozen {
            frozen = false
            freezer.end()
        }
    }

    private func engineConfiguration() -> EngineConfiguration {
        lock.lock()
        let settings = settings
        lock.unlock()
        var config = EngineConfiguration()
        config.edges = settings.edges
        config.corners = settings.corners
        config.sensitivity = settings.sensitivity
        config.currentBrightness = brightness.level
        config.currentVolume = audio.level
        config.mediaStripReverse = settings.mediaStripReverse
        config.requireModifier = settings.requireModifier
        config.fineControlWithModifiers = settings.fineControlWithModifiers
        config.preciseSliding = settings.preciseSliding
        return config
    }

    private func showHUD() -> Bool {
        lock.lock()
        defer { lock.unlock() }
        return settings.showHUD
    }

    private func haptics() -> (Bool, HapticStrength) {
        lock.lock()
        defer { lock.unlock() }
        return (settings.hapticsEnabled, settings.hapticStrength)
    }

    // MARK: SlideEngineDelegate

    func slideEngine(_ engine: SlideEngine, didBeginSlide target: EdgeAction) {
        guard showHUD() else { return }
        slideCallback?(Self.code(for: target), 0, level(for: target))
    }

    func slideEngine(_ engine: SlideEngine, didUpdateSlide target: EdgeAction, level: Double) {
        DispatchQueue.main.async { [weak self] in
            guard let self else { return }
            switch target {
            case .volume: self.audio.setLevel(level)
            case .brightness: self.brightness.setLevel(level)
            }
            guard self.showHUD() else { return }
            let shown = target == .volume ? self.audio.level : self.brightness.level
            self.slideCallback?(Self.code(for: target), 1, shown)
        }
    }

    func slideEngine(_ engine: SlideEngine, didEndSlide target: EdgeAction, level: Double) {
        guard showHUD() else { return }
        slideCallback?(Self.code(for: target), 2, level)
    }

    func slideEngine(_ engine: SlideEngine, didTick target: EdgeAction, level: Double) {
        let (enabled, strength) = haptics()
        guard enabled else { return }
        DispatchQueue.main.async {
            Haptics.shared.tick(strength)
        }
    }

    func slideEngineDidArmSingleTap(_ engine: SlideEngine) {
        singleTapToken += 1
        let token = singleTapToken
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.45) { [weak self] in
            guard let self, token == self.singleTapToken else { return }
            self.lock.lock()
            let action = engine.takePendingSingle()
            self.lock.unlock()
            guard let command = action?.command else { return }
            self.perform(command)
        }
    }

    func slideEngine(_ engine: SlideEngine, didRecognize command: MediaCommand) {
        singleTapToken += 1
        DispatchQueue.main.async { [weak self] in
            self?.perform(command)
        }
    }

    func slideEngine(_ engine: SlideEngine, didBeginHold corner: Corner) {
        holdCallback?(Self.code(for: corner), 1)
    }

    func slideEngine(_ engine: SlideEngine, didEndHold corner: Corner) {
        holdCallback?(Self.code(for: corner), 0)
    }

    private func perform(_ command: MediaCommand) {
        switch command {
        case .playPause:
            MediaKeyController.shared.send(.playPause)
            commandCallback?(0, 0, 0)
        case .next:
            MediaKeyController.shared.send(.next)
            commandCallback?(1, 0, 0)
        case .previous:
            MediaKeyController.shared.send(.previous)
            commandCallback?(2, 0, 0)
        case .mute:
            audio.toggleMute()
            let (enabled, strength) = haptics()
            if enabled { Haptics.shared.buzz(strength) }
            commandCallback?(3, audio.level, audio.isMuted ? 1 : 0)
        }
    }

    private func level(for target: EdgeAction) -> Double {
        switch target {
        case .volume: return audio.level
        case .brightness: return brightness.level
        }
    }

    private static func code(for target: EdgeAction) -> Int32 {
        target == .volume ? 0 : 1
    }

    private static func code(for corner: Corner) -> Int32 {
        switch corner {
        case .topLeft: return 0
        case .topRight: return 1
        case .bottomLeft: return 2
        case .bottomRight: return 3
        }
    }
}

@_cdecl("tap_gestures_start")
public func tapGesturesStart(
    _ slide: @convention(c) (Int32, Int32, Double) -> Void,
    _ command: @convention(c) (Int32, Double, Int32) -> Void,
    _ hold: @convention(c) (Int32, Int32) -> Void
) -> Int32 {
    GestureHost.shared.start(slide: slide, command: command, hold: hold)
}

@_cdecl("tap_gestures_apply_config")
public func tapGesturesApplyConfig(_ json: UnsafePointer<CChar>?) -> Int32 {
    guard let json else { return 1 }
    return GestureHost.shared.apply(json: String(cString: json))
}

@_cdecl("tap_gestures_stop")
public func tapGesturesStop() {
    GestureHost.shared.stop()
}
