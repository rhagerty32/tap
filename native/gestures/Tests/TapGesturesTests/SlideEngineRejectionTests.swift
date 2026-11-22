import XCTest
@testable import TapGestures

final class SlideEngineRejectionTests: XCTestCase {
    func testCornerHoldStillFiresForAFingertipWhenTheKeyboardIsIdle() {
        let harness = Harness()
        harness.engine.process(frame(id: 1, x: 0.05, y: 0.05, major: 8, at: 1))
        harness.engine.process(frame(id: 1, x: 0.05, y: 0.05, major: 8, at: 1.35))
        XCTAssertEqual(harness.holds, [.bottomLeft])
    }

    func testTouchThatLandsWhileTypingIsIgnoredUntilItLifts() {
        let harness = Harness()
        harness.keyAge = 0.1
        harness.engine.process(frame(id: 7, x: 0.05, y: 0.05, major: 8, at: 1))
        harness.keyAge = 5
        harness.engine.process(frame(id: 7, x: 0.05, y: 0.05, major: 8, at: 1.5))
        harness.engine.process(frame(id: 7, x: 0.05, y: 0.05, major: 8, at: 1.6, touching: false))
        harness.engine.process(frame(id: 8, x: 0.05, y: 0.05, major: 8, at: 2))
        harness.engine.process(frame(id: 8, x: 0.05, y: 0.05, major: 8, at: 2.35))
        XCTAssertEqual(harness.holds, [.bottomLeft])
    }

    func testKeyDuringAnUncommittedRestCancelsItForThatTouch() {
        let harness = Harness()
        harness.engine.process(frame(id: 3, x: 0.05, y: 0.05, major: 8, at: 1))
        harness.keyAge = 0.05
        harness.engine.process(frame(id: 3, x: 0.05, y: 0.05, major: 8, at: 1.1))
        harness.keyAge = 5
        harness.engine.process(frame(id: 3, x: 0.05, y: 0.05, major: 8, at: 1.5))
        XCTAssertTrue(harness.holds.isEmpty)
        XCTAssertEqual(harness.armed, 0)
    }

    func testSlideThatAlreadyMovedKeepsGoingWhenAKeyArrives() {
        let harness = Harness()
        harness.engine.process(frame(id: 4, x: 0.05, y: 0.40, major: 8, at: 1))
        harness.engine.process(frame(id: 4, x: 0.05, y: 0.50, major: 8, at: 1.05))
        harness.keyAge = 0.02
        harness.engine.process(frame(id: 4, x: 0.05, y: 0.70, major: 8, at: 1.1))
        XCTAssertEqual(harness.began, [.brightness])
        XCTAssertGreaterThan(harness.updates, 0)
    }

    func testWideContactDoesNotHold() {
        let harness = Harness()
        harness.engine.process(frame(id: 5, x: 0.05, y: 0.05, major: 30, at: 1))
        harness.engine.process(frame(id: 5, x: 0.05, y: 0.05, major: 30, at: 1.4))
        XCTAssertTrue(harness.holds.isEmpty)
    }

    func testPalmRejectionOffAllowsAWideContact() {
        let harness = Harness()
        harness.maxTouchSize = 0
        harness.engine.process(frame(id: 6, x: 0.05, y: 0.05, major: 30, at: 1))
        harness.engine.process(frame(id: 6, x: 0.05, y: 0.05, major: 30, at: 1.4))
        XCTAssertEqual(harness.holds, [.bottomLeft])
    }

    func testTypingPauseOffAllowsATouchRightAfterAKey() {
        let harness = Harness()
        harness.typingPause = 0
        harness.keyAge = 0.05
        harness.engine.process(frame(id: 9, x: 0.05, y: 0.05, major: 8, at: 1))
        harness.engine.process(frame(id: 9, x: 0.05, y: 0.05, major: 8, at: 1.35))
        XCTAssertEqual(harness.holds, [.bottomLeft])
    }

    func testNarrowedTopBandDoesNotSlideFromTheCornerOutsideIt() {
        let harness = Harness()
        harness.edges = narrowedTopBrightness()
        harness.engine.configuration = harness.configuration
        harness.engine.process(frame(id: 11, x: 0.04, y: 0.96, major: 8, at: 1))
        harness.engine.process(frame(id: 11, x: 0.18, y: 0.96, major: 8, at: 1.08))
        XCTAssertTrue(harness.began.isEmpty)
        XCTAssertEqual(harness.updates, 0)
    }

    func testNarrowedTopBandStillSlidesFromInsideTheBand() {
        let harness = Harness()
        harness.edges = narrowedTopBrightness()
        harness.engine.configuration = harness.configuration
        harness.engine.process(frame(id: 12, x: 0.40, y: 0.96, major: 8, at: 1))
        harness.engine.process(frame(id: 12, x: 0.58, y: 0.96, major: 8, at: 1.08))
        XCTAssertEqual(harness.began, [.brightness])
    }

    func testMissingSettingsUseTheRejectionDefaults() throws {
        let settings = try JSONDecoder().decode(Settings.self, from: Data("{}".utf8))
        XCTAssertTrue(settings.typingPauseEnabled)
        XCTAssertEqual(settings.typingPause, 0.4, accuracy: 0.001)
        XCTAssertTrue(settings.palmRejectionEnabled)
        XCTAssertEqual(settings.maxTouchSize, 18, accuracy: 0.001)
    }
}

private final class Harness: SlideEngineDelegate {
    let engine = SlideEngine()
    var keyAge: TimeInterval = 10
    var typingPause: Double = 0.4
    var maxTouchSize: Double = 18
    var holds: [Corner] = []
    var armed = 0
    var began: [EdgeAction] = []
    var updates = 0
    var edges: EdgeMap?

    init() {
        engine.delegate = self
        engine.keyAge = { [unowned self] in self.keyAge }
        engine.configuration = configuration
    }

    var configuration: () -> EngineConfiguration {
        { [unowned self] in
            var config = EngineConfiguration()
            config.typingPause = self.typingPause
            config.maxTouchSize = self.maxTouchSize
            config.corners.bottomLeft.holdPeer = "sam"
            config.corners.bottomLeft.single = .mute
            config.corners.topLeft.holdPeer = "sam"
            if let edges = self.edges {
                config.edges = edges
            }
            return config
        }
    }

    func slideEngine(_ engine: SlideEngine, didBeginSlide target: EdgeAction) { began.append(target) }
    func slideEngine(_ engine: SlideEngine, didUpdateSlide target: EdgeAction, level: Double) { updates += 1 }
    func slideEngine(_ engine: SlideEngine, didEndSlide target: EdgeAction, level: Double) {}
    func slideEngine(_ engine: SlideEngine, didTick target: EdgeAction, level: Double) {}
    func slideEngine(_ engine: SlideEngine, didRecognize command: MediaCommand) {}
    func slideEngine(_ engine: SlideEngine, didOpenRaycast link: String) {}
    func slideEngineDidArmSingleTap(_ engine: SlideEngine) { armed += 1 }
    func slideEngine(_ engine: SlideEngine, didBeginHold corner: Corner) { holds.append(corner) }
    func slideEngine(_ engine: SlideEngine, didEndHold corner: Corner) {}
}

private func narrowedTopBrightness() -> EdgeMap {
    var edges = EdgeMap.factory
    edges.left.enabled = false
    edges.right.enabled = false
    edges.top = EdgeSensor(enabled: true, action: .brightness, start: 0.2, end: 0.8, thickness: 0.18)
    return edges
}

private func frame(id: Int32, x: Double, y: Double, major: Double, at time: TimeInterval, touching: Bool = true) -> TouchFrame {
    TouchFrame(
        timestamp: time,
        contacts: [TouchContact(id: id, x: x, y: y, isTouching: touching, majorAxis: major)]
    )
}
