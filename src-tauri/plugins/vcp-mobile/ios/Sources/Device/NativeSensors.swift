import CoreLocation
import CoreMotion
import Foundation
import Tauri
import UIKit

private struct SensorSampleArgs: Decodable {
    let location: Bool
    let motion: Bool
    let ambient: Bool
}

private struct SensorSampleResult: Encodable {
    var location: String?
    var motion: String?
    var ambient: String?
}

private enum NativeSensors {
    // Apple requires a single CMMotionManager per app. Sampling is serial, matching
    // the Rust streaming scheduler; a second request cannot stop the first one.
    static let motion = CMMotionManager()
    static var active: NativeSensorRequest?

    static func sample(_ args: SensorSampleArgs, invoke: Invoke) {
        dispatchPrecondition(condition: .onQueue(.main))
        guard active == nil else {
            invoke.reject("sensor_sample_busy: 上一次传感器采样尚未结束")
            return
        }
        let request = NativeSensorRequest(args, invoke: invoke, motion: motion)
        active = request
        request.start()
    }

    static func finished(_ request: NativeSensorRequest) {
        if active === request { active = nil }
    }
}

private final class NativeSensorRequest: NSObject, CLLocationManagerDelegate {
    private let args: SensorSampleArgs
    private var invoke: Invoke?
    private let motion: CMMotionManager
    private let altimeter = CMAltimeter()
    private var locationManager: CLLocationManager?
    private var deadline: DispatchWorkItem?
    private var locationEnd: DispatchWorkItem?
    private var sampleEnd: DispatchWorkItem?
    private var backgroundObservers: [NSObjectProtocol] = []
    private var result = SensorSampleResult()
    private var samples: [Double] = []
    private var gyroSamples: [Double] = []
    private var magneticSamples: [Double] = []
    private var accelerometerError: String?
    private var gyroscopeError: String?
    private var magnetometerError: String?
    private var requestedLocation = false
    private var sampleWindowFinished = false

    init(_ args: SensorSampleArgs, invoke: Invoke, motion: CMMotionManager) {
        self.args = args
        self.invoke = invoke
        self.motion = motion
        super.init()
    }

    func start() {
        dispatchPrecondition(condition: .onQueue(.main))
        guard args.location || args.motion || args.ambient else { finish(); return }
        guard UIApplication.shared.applicationState != .background else {
            finish(reason: "App 已在后台，未启动前台传感器采样")
            return
        }
        for name in [UIApplication.didEnterBackgroundNotification, UIScene.didEnterBackgroundNotification] {
            let observer = NotificationCenter.default.addObserver(
                forName: name, object: nil, queue: .main
            ) { [weak self] notification in
                if let scene = notification.object as? UIScene,
                   scene.session.role != .windowApplication { return }
                self?.finish(reason: "App 进入后台，采样已停止")
            }
            backgroundObservers.append(observer)
        }
        // A prompt can outlive its caller. A finite owner keeps its Invoke alive,
        // then stops every sensor and resolves partial data even without a decision.
        let deadline = DispatchWorkItem { [weak self] in
            self?.finish(reason: "本次采样期限内未取得读数或权限结果")
        }
        self.deadline = deadline
        DispatchQueue.main.asyncAfter(deadline: .now() + 20, execute: deadline)
        if args.motion { startMotion() }
        if args.ambient { startAmbient() }
        if args.location { startLocation() }
        let sampleEnd = DispatchWorkItem { [weak self] in self?.finishSampleWindow() }
        self.sampleEnd = sampleEnd
        DispatchQueue.main.asyncAfter(deadline: .now() + 2, execute: sampleEnd)
    }

    private func startMotion() {
        if motion.isAccelerometerAvailable {
            motion.accelerometerUpdateInterval = 0.1
            motion.startAccelerometerUpdates(to: .main) { [weak self] data, error in
                guard let self = self, self.invoke != nil, !self.sampleWindowFinished else { return }
                if let error = error {
                    self.accelerometerError = error.localizedDescription
                    self.motion.stopAccelerometerUpdates()
                    return
                }
                guard let acceleration = data?.acceleration else { return }
                let magnitude = sqrt(acceleration.x * acceleration.x
                    + acceleration.y * acceleration.y + acceleration.z * acceleration.z) * 9.80665
                // Bound memory even if a driver delivers more callbacks than requested.
                if magnitude.isFinite && self.samples.count < 40 { self.samples.append(magnitude) }
            }
        } else {
            accelerometerError = "此设备没有可用加速度计"
        }
        if motion.isGyroAvailable {
            motion.gyroUpdateInterval = 0.1
            motion.startGyroUpdates(to: .main) { [weak self] data, error in
                guard let self = self, self.invoke != nil, !self.sampleWindowFinished else { return }
                if let error = error {
                    self.gyroscopeError = error.localizedDescription
                    self.motion.stopGyroUpdates()
                    return
                }
                guard let rate = data?.rotationRate else { return }
                let magnitude = sqrt(rate.x * rate.x + rate.y * rate.y + rate.z * rate.z)
                if magnitude.isFinite && self.gyroSamples.count < 40 { self.gyroSamples.append(magnitude) }
            }
        } else {
            gyroscopeError = "此设备没有可用陀螺仪"
        }
        if motion.isMagnetometerAvailable {
            motion.magnetometerUpdateInterval = 0.1
            motion.startMagnetometerUpdates(to: .main) { [weak self] data, error in
                guard let self = self, self.invoke != nil, !self.sampleWindowFinished else { return }
                if let error = error {
                    self.magnetometerError = error.localizedDescription
                    self.motion.stopMagnetometerUpdates()
                    return
                }
                guard let field = data?.magneticField else { return }
                let magnitude = sqrt(field.x * field.x + field.y * field.y + field.z * field.z)
                if magnitude.isFinite && self.magneticSamples.count < 40 { self.magneticSamples.append(magnitude) }
            }
        } else {
            magnetometerError = "此设备没有可用磁力计"
        }
    }

    private func startAmbient() {
        guard CMAltimeter.isRelativeAltitudeAvailable() else {
            result.ambient = "环境光: iOS 未公开照度读数 | 气压: 此设备无可用气压计"
            return
        }
        switch CMAltimeter.authorizationStatus() {
        case .denied, .restricted:
            result.ambient = "环境光: iOS 未公开照度读数 | 气压: 运动与健身权限未授予"
            return
        default: break
        }
        // The first real use asks for Motion & Fitness permission when needed.
        altimeter.startRelativeAltitudeUpdates(to: .main) { [weak self] data, error in
            guard let self = self, self.invoke != nil, self.result.ambient == nil else { return }
            if let error = error {
                self.result.ambient = "环境光: iOS 未公开照度读数 | 气压: \(error.localizedDescription)"
                self.altimeter.stopRelativeAltitudeUpdates()
                self.finishIfReady()
                return
            }
            if let value = data?.pressure.doubleValue, value.isFinite, value > 0 {
                // Core Motion reports kPa; the tool uses hPa.
                self.result.ambient = String(format: "环境光: iOS 未公开照度读数 | 气压: %.1f hPa", value * 10)
                self.altimeter.stopRelativeAltitudeUpdates()
                self.finishIfReady()
            }
        }
    }

    private func startLocation() {
        let manager = CLLocationManager()
        locationManager = manager
        manager.delegate = self
        manager.desiredAccuracy = kCLLocationAccuracyHundredMeters
        continueLocation(manager)
    }

    private func continueLocation(_ manager: CLLocationManager) {
        guard invoke != nil, result.location == nil else { return }
        switch manager.authorizationStatus {
        case .notDetermined:
            if !requestedLocation {
                requestedLocation = true
                manager.requestWhenInUseAuthorization()
            }
        case .authorizedAlways, .authorizedWhenInUse:
            // Setting the delegate triggers an initial authorization callback too.
            // Clearing this reference is unnecessary; track the request separately.
            if !locationUpdateRequested {
                locationUpdateRequested = true
                manager.requestLocation()
                let locationEnd = DispatchWorkItem { [weak self] in
                    guard let self = self, self.invoke != nil, self.result.location == nil else { return }
                    self.result.location = "位置信息: 五秒采样窗口内未取得位置"
                    self.locationManager?.stopUpdatingLocation()
                    self.finishIfReady()
                }
                self.locationEnd = locationEnd
                DispatchQueue.main.asyncAfter(deadline: .now() + 5, execute: locationEnd)
            }
        case .denied, .restricted:
            result.location = "位置信息: 定位权限未授予，可在系统设置中修改"
            finishIfReady()
        @unknown default:
            result.location = "位置信息: 未知定位授权状态"
            finishIfReady()
        }
    }

    private var locationUpdateRequested = false

    func locationManagerDidChangeAuthorization(_ manager: CLLocationManager) {
        guard manager === locationManager else { return }
        continueLocation(manager)
    }

    func locationManager(_ manager: CLLocationManager, didUpdateLocations locations: [CLLocation]) {
        guard invoke != nil, manager === locationManager, result.location == nil else { return }
        let approximate = manager.accuracyAuthorization == .reducedAccuracy
        let receivedAt = Date()
        guard let location = locations.last(where: {
            let age = receivedAt.timeIntervalSince($0.timestamp)
            // Reduced-accuracy locations update much less frequently than precise
            // fixes. Keep their actual system timestamp instead of rejecting them
            // under the precise-fix freshness rule or implying a new observation.
            return $0.horizontalAccuracy.isFinite && $0.horizontalAccuracy >= 0
                && CLLocationCoordinate2DIsValid($0.coordinate) && age.isFinite
                && (approximate || abs(age) <= 30)
        }) else {
            result.location = approximate
                ? "位置信息: 本次未取得有效的系统大致位置"
                : "位置信息: 本次未取得 30 秒内的有效位置"
            finishIfReady()
            return
        }
        let latitude = location.coordinate.latitude
        let longitude = location.coordinate.longitude
        let coordinateLabel = approximate ? "大致位置区域中心" : "坐标"
        var text = String(format: "%@: %.5f°%@, %.5f°%@ | 精度: %.0fm", coordinateLabel,
                          abs(latitude), latitude >= 0 ? "N" : "S",
                          abs(longitude), longitude >= 0 ? "E" : "W", location.horizontalAccuracy)
        if location.verticalAccuracy >= 0 {
            text += String(format: " | 海拔: %.0fm", location.altitude)
        } else {
            text += " | 海拔: 本次不可用"
        }
        if location.speed.isFinite && location.speed >= 0 {
            text += String(format: " | 移动速度: %.2fm/s", location.speed)
        } else {
            text += " | 移动速度: 本次不可用"
        }
        let age = receivedAt.timeIntervalSince(location.timestamp)
        let timestamp = ISO8601DateFormatter().string(from: location.timestamp)
        text += " | 系统采样时间: \(timestamp)"
        text += age >= 0
            ? String(format: " (接收时已过去 %.0f 秒)", age)
            : String(format: " (比接收时的系统时间晚 %.0f 秒)", -age)
        if approximate { text += " | 系统提供大致位置，非实时定位" }
        result.location = text
        manager.stopUpdatingLocation()
        finishIfReady()
    }

    func locationManager(_ manager: CLLocationManager, didFailWithError error: Error) {
        guard invoke != nil, manager === locationManager, result.location == nil else { return }
        result.location = "位置信息: \(error.localizedDescription)"
        manager.stopUpdatingLocation()
        finishIfReady()
    }

    private func finishSampleWindow() {
        guard invoke != nil else { return }
        sampleWindowFinished = true
        motion.stopAccelerometerUpdates()
        motion.stopGyroUpdates()
        motion.stopMagnetometerUpdates()
        if args.motion, result.motion == nil {
            result.motion = motionResult()
        }
        if args.ambient, result.ambient == nil,
           CMAltimeter.authorizationStatus() != .notDetermined {
            result.ambient = "环境光: iOS 未公开照度读数 | 气压: 采样窗口内没有读数"
            altimeter.stopRelativeAltitudeUpdates()
        }
        finishIfReady()
    }

    private func motionResult() -> String {
        [
            motionSummary("加速度(含重力)", values: samples, unit: "m/s²", error: accelerometerError),
            motionSummary("旋转角速度", values: gyroSamples, unit: "rad/s", error: gyroscopeError),
            motionSummary("磁场强度", values: magneticSamples, unit: "μT", error: magnetometerError)
        ].joined(separator: " | ")
    }

    private func motionSummary(_ label: String, values: [Double], unit: String, error: String?) -> String {
        guard !values.isEmpty else { return "\(label): \(error ?? "采样窗口内没有读数")" }
        let mean = values.reduce(0, +) / Double(values.count)
        let peak = values.max() ?? mean
        var text = String(format: "%@: 平均 %.2f%@ (峰值 %.2f%@, %d 个样本)",
                          label, mean, unit, peak, unit, values.count)
        if let error = error { text += "，采样中断: \(error)" }
        return text
    }

    private func finishIfReady() {
        guard sampleWindowFinished,
              !args.location || result.location != nil,
              !args.motion || result.motion != nil,
              !args.ambient || result.ambient != nil else { return }
        finish()
    }

    private func finish(reason: String = "本次没有读数") {
        dispatchPrecondition(condition: .onQueue(.main))
        guard let invoke = invoke else { return }
        self.invoke = nil
        deadline?.cancel()
        deadline = nil
        locationEnd?.cancel()
        locationEnd = nil
        sampleEnd?.cancel()
        sampleEnd = nil
        motion.stopAccelerometerUpdates()
        motion.stopGyroUpdates()
        motion.stopMagnetometerUpdates()
        altimeter.stopRelativeAltitudeUpdates()
        locationManager?.stopUpdatingLocation()
        locationManager?.delegate = nil
        locationManager = nil
        for observer in backgroundObservers { NotificationCenter.default.removeObserver(observer) }
        backgroundObservers.removeAll()
        if args.location, result.location == nil { result.location = "位置信息: \(reason)" }
        if args.motion, result.motion == nil {
            if !samples.isEmpty || !gyroSamples.isEmpty || !magneticSamples.isEmpty {
                result.motion = "\(motionResult()) | 采样提前结束: \(reason)"
            } else {
                result.motion = "运动传感器: \(reason)"
            }
        }
        if args.ambient, result.ambient == nil {
            result.ambient = "环境光: iOS 未公开照度读数 | 气压: \(reason)"
        }
        NativeSensors.finished(self)
        invoke.resolve(result)
    }
}

extension VcpMobilePlugin {
    @objc public func sampleSensorData(_ invoke: Invoke) {
        do {
            let args = try invoke.parseArgs(SensorSampleArgs.self)
            DispatchQueue.main.async { NativeSensors.sample(args, invoke: invoke) }
        } catch {
            invoke.reject("invalid_sensor_sample_args: \(error.localizedDescription)")
        }
    }
}
