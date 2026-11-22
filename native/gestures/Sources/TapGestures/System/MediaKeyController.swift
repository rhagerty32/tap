import AppKit
import CoreGraphics
import Foundation

/// Posts the same system-defined key events the media keys on a keyboard send.
public final class MediaKeyController {
    public static let shared = MediaKeyController()

    public enum Command {
        case playPause
        case next
        case previous
        case volumeUp
        case volumeDown
        case brightnessUp
        case brightnessDown
        case mute
    }

    // NX_KEYTYPE_* values from IOKit/hidsystem/ev_keymap.h.
    private enum Key: Int32 {
        case soundUp = 0
        case soundDown = 1
        case brightnessUp = 2
        case brightnessDown = 3
        case mute = 7
        case play = 16
        case next = 17
        case previous = 18
    }

    private init() {}

    public func send(_ command: Command) {
        switch command {
        case .playPause: press(.play)
        case .next: press(.next)
        case .previous: press(.previous)
        case .volumeUp: press(.soundUp)
        case .volumeDown: press(.soundDown)
        case .brightnessUp: press(.brightnessUp)
        case .brightnessDown: press(.brightnessDown)
        case .mute: press(.mute)
        }
    }

    private func press(_ key: Key) {
        post(key: key, down: true)
        post(key: key, down: false)
    }

    private func post(key: Key, down: Bool) {
        let flags = NSEvent.ModifierFlags(rawValue: 0xa00) // fn + numeric pad
        let state: Int = down ? 0xA : 0xB
        let data1 = Int(key.rawValue << 16) | (state << 8)
        guard
            let event = NSEvent.otherEvent(
                with: .systemDefined,
                location: .zero,
                modifierFlags: flags,
                timestamp: 0,
                windowNumber: 0,
                context: nil,
                subtype: 8,
                data1: data1,
                data2: -1
            ),
            let cgEvent = event.cgEvent
        else { return }
        cgEvent.post(tap: .cghidEventTap)
    }
}
