import CoreAudio
import Foundation

/// Reads and writes the system output volume with CoreAudio.
///
/// Devices differ: some expose a virtual master volume, others only per-channel
/// scalars, so every call walks a list of candidate properties.
public final class AudioController: LevelController {
    public static let shared = AudioController()

    /// FourCC for `kAudioHardwareServiceDeviceProperty_VirtualMainVolume`.
    private static let virtualMainVolume: AudioObjectPropertySelector = 0x766D_7663 // 'vmvc'

    private var cachedDevice = AudioDeviceID(0)
    private var cachedLevel: Double = 0.5
    private var cachedMuted = false

    /// Called on the main queue whenever the system volume, mute state or
    /// default output device changes. There is no timer behind this: CoreAudio
    /// pushes the notification.
    public var onChange: (() -> Void)?

    private struct Listener {
        let object: AudioObjectID
        let address: AudioObjectPropertyAddress
        let block: AudioObjectPropertyListenerBlock
    }

    private var systemListeners: [Listener] = []
    private var deviceListeners: [Listener] = []

    public init() {
        refresh()
    }

    public var level: Double {
        cachedLevel
    }

    public var isMuted: Bool {
        cachedMuted
    }

    public func refresh() {
        let device = defaultOutputDevice()
        cachedDevice = device
        guard device != 0 else { return }
        if let value = readVolume(device: device) {
            cachedLevel = Double(value)
        }
        if let muted = readMute(device: device) {
            cachedMuted = muted
        }
    }

    // MARK: - Observation

    /// Starts event-driven observation. Cheap when idle: no polling, no timer.
    public func startObserving() {
        guard systemListeners.isEmpty, deviceListeners.isEmpty else { return }

        addSystemListener(selector: kAudioHardwarePropertyDefaultOutputDevice)
        registerDeviceListeners()
    }

    public func stopObserving() {
        remove(systemListeners)
        remove(deviceListeners)
        systemListeners.removeAll()
        deviceListeners.removeAll()
    }

    private func addSystemListener(selector: AudioObjectPropertySelector) {
        addListener(
            on: AudioObjectID(kAudioObjectSystemObject),
            selector: selector,
            scope: kAudioObjectPropertyScopeGlobal,
            element: kAudioObjectPropertyElementMain,
            into: &systemListeners
        )
    }

    private func registerDeviceListeners() {
        let device = currentDevice()
        guard device != 0 else { return }
        addListener(on: device, selector: kAudioDevicePropertyVolumeScalar, scope: kAudioObjectPropertyScopeOutput, element: kAudioObjectPropertyElementMain, into: &deviceListeners)
        addListener(on: device, selector: kAudioDevicePropertyMute, scope: kAudioObjectPropertyScopeOutput, element: kAudioObjectPropertyElementMain, into: &deviceListeners)
        for element in [AudioObjectPropertyElement(1), AudioObjectPropertyElement(2)] {
            addListener(on: device, selector: kAudioDevicePropertyVolumeScalar, scope: kAudioObjectPropertyScopeOutput, element: element, into: &deviceListeners)
        }
    }

    private func addListener(
        on object: AudioObjectID,
        selector: AudioObjectPropertySelector,
        scope: AudioObjectPropertyScope,
        element: AudioObjectPropertyElement,
        into list: inout [Listener]
    ) {
        var address = AudioObjectPropertyAddress(mSelector: selector, mScope: scope, mElement: element)
        guard AudioObjectHasProperty(object, &address) else { return }

        let block: AudioObjectPropertyListenerBlock = { [weak self] _, _ in
            self?.configurationChanged()
        }
        let status = AudioObjectAddPropertyListenerBlock(object, &address, DispatchQueue.main, block)
        guard status == noErr else { return }
        list.append(Listener(object: object, address: address, block: block))
    }

    private func remove(_ list: [Listener]) {
        for listener in list {
            var address = listener.address
            AudioObjectRemovePropertyListenerBlock(listener.object, &address, DispatchQueue.main, listener.block)
        }
    }

    private func configurationChanged() {
        let previousDevice = cachedDevice
        refresh()

        if cachedDevice != previousDevice {
            // The default output changed: the per-device listeners are stale.
            remove(deviceListeners)
            deviceListeners.removeAll()
            registerDeviceListeners()
        }
        onChange?()
    }

    public func setLevel(_ level: Double) {
        let clamped = Float(min(max(level, 0), 1))
        let device = currentDevice()
        guard device != 0 else { return }

        // Writing a non-zero level always unmutes, like the system volume keys.
        if clamped > 0, cachedMuted {
            writeMute(device: device, muted: false)
            cachedMuted = false
        }
        writeVolume(device: device, value: clamped)
        cachedLevel = Double(clamped)
    }

    public func toggleMute() {
        let device = currentDevice()
        guard device != 0 else { return }
        let newValue = !cachedMuted
        writeMute(device: device, muted: newValue)
        cachedMuted = newValue
    }

    // MARK: - Device

    private func currentDevice() -> AudioDeviceID {
        if cachedDevice != 0, deviceIsAlive(cachedDevice) {
            return cachedDevice
        }
        let device = defaultOutputDevice()
        cachedDevice = device
        return device
    }

    private func deviceIsAlive(_ device: AudioDeviceID) -> Bool {
        var address = AudioObjectPropertyAddress(
            mSelector: kAudioDevicePropertyDeviceIsAlive,
            mScope: kAudioObjectPropertyScopeGlobal,
            mElement: kAudioObjectPropertyElementMain
        )
        var alive = UInt32(0)
        var size = UInt32(MemoryLayout<UInt32>.size)
        let status = AudioObjectGetPropertyData(device, &address, 0, nil, &size, &alive)
        return status == noErr && alive != 0
    }

    private func defaultOutputDevice() -> AudioDeviceID {
        var address = AudioObjectPropertyAddress(
            mSelector: kAudioHardwarePropertyDefaultOutputDevice,
            mScope: kAudioObjectPropertyScopeGlobal,
            mElement: kAudioObjectPropertyElementMain
        )
        var device = AudioDeviceID(0)
        var size = UInt32(MemoryLayout<AudioDeviceID>.size)
        let status = AudioObjectGetPropertyData(
            AudioObjectID(kAudioObjectSystemObject), &address, 0, nil, &size, &device
        )
        return status == noErr ? device : 0
    }

    // MARK: - Volume

    private func volumeAddresses(device: AudioDeviceID) -> [AudioObjectPropertyAddress] {
        let candidates: [AudioObjectPropertyAddress] = [
            .init(mSelector: Self.virtualMainVolume, mScope: kAudioObjectPropertyScopeGlobal, mElement: kAudioObjectPropertyElementMain),
            .init(mSelector: Self.virtualMainVolume, mScope: kAudioObjectPropertyScopeOutput, mElement: kAudioObjectPropertyElementMain),
            .init(mSelector: kAudioDevicePropertyVolumeScalar, mScope: kAudioObjectPropertyScopeOutput, mElement: kAudioObjectPropertyElementMain),
            .init(mSelector: kAudioDevicePropertyVolumeScalar, mScope: kAudioObjectPropertyScopeOutput, mElement: 1),
            .init(mSelector: kAudioDevicePropertyVolumeScalar, mScope: kAudioObjectPropertyScopeOutput, mElement: 2)
        ]
        return candidates.filter { address in
            var address = address
            return AudioObjectHasProperty(device, &address) && isSettable(device, &address)
        }
    }

    private func readVolume(device: AudioDeviceID) -> Float? {
        for var address in volumeAddresses(device: device) {
            var value = Float32(0)
            var size = UInt32(MemoryLayout<Float32>.size)
            if AudioObjectGetPropertyData(device, &address, 0, nil, &size, &value) == noErr {
                return value
            }
        }
        return nil
    }

    private func writeVolume(device: AudioDeviceID, value: Float) {
        for var address in volumeAddresses(device: device) {
            var payload = value
            _ = AudioObjectSetPropertyData(
                device,
                &address,
                0,
                nil,
                UInt32(MemoryLayout<Float32>.size),
                &payload
            )
        }
    }

    // MARK: - Mute

    private func muteAddresses(device: AudioDeviceID) -> [AudioObjectPropertyAddress] {
        let candidates: [AudioObjectPropertyAddress] = [
            .init(mSelector: kAudioDevicePropertyMute, mScope: kAudioObjectPropertyScopeOutput, mElement: kAudioObjectPropertyElementMain),
            .init(mSelector: kAudioDevicePropertyMute, mScope: kAudioObjectPropertyScopeOutput, mElement: 1),
            .init(mSelector: kAudioDevicePropertyMute, mScope: kAudioObjectPropertyScopeOutput, mElement: 2)
        ]
        return candidates.filter { address in
            var address = address
            return AudioObjectHasProperty(device, &address) && isSettable(device, &address)
        }
    }

    private func readMute(device: AudioDeviceID) -> Bool? {
        for var address in muteAddresses(device: device) {
            var value = UInt32(0)
            var size = UInt32(MemoryLayout<UInt32>.size)
            if AudioObjectGetPropertyData(device, &address, 0, nil, &size, &value) == noErr {
                return value != 0
            }
        }
        return nil
    }

    private func writeMute(device: AudioDeviceID, muted: Bool) {
        for var address in muteAddresses(device: device) {
            var value = UInt32(muted ? 1 : 0)
            _ = AudioObjectSetPropertyData(
                device,
                &address,
                0,
                nil,
                UInt32(MemoryLayout<UInt32>.size),
                &value
            )
        }
    }

    private func isSettable(_ device: AudioDeviceID, _ address: inout AudioObjectPropertyAddress) -> Bool {
        var settable = DarwinBoolean(false)
        let status = AudioObjectIsPropertySettable(device, &address, &settable)
        return status == noErr && settable.boolValue
    }
}
