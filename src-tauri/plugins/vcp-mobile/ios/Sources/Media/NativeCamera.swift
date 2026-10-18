import AVFoundation
import Foundation
import Tauri
import UIKit

/// Still capture only. No decoded UIImage is materialized by the application.
final class NativeCamera: UIViewController, AVCapturePhotoCaptureDelegate {
    private let session = AVCaptureSession()
    private let output = AVCapturePhotoOutput()
    private let queue = DispatchQueue(label: "com.vcp.avatar.camera", qos: .userInitiated)
    private let owner: NativePresentation
    private let operation: NativeFileOperation
    private let stagingDir: String
    private let maxBytes: UInt64
    private var preview: AVCaptureVideoPreviewLayer?
    private var input: AVCaptureDeviceInput?
    private var interruption: NSObjectProtocol?
    private var runtimeError: NSObjectProtocol?
    private var capturing = false // main queue
    private var captureResult: Result<NativePickedFile, Error>? // capture queue
    private var terminated = false // capture queue
    private let shutter = UIButton(type: .system)
    private let flip = UIButton(type: .system)

    private init(owner: NativePresentation, operation: NativeFileOperation, stagingDir: String, maxBytes: UInt64) {
        self.owner = owner; self.operation = operation; self.stagingDir = stagingDir; self.maxBytes = maxBytes
        super.init(nibName: nil, bundle: nil)
        modalPresentationStyle = .fullScreen
    }
    required init?(coder: NSCoder) { return nil }

    static func start(owner: NativePresentation, operation: NativeFileOperation, stagingDir: String, maxBytes: UInt64) {
        operation.working = true
        let authorized: (Bool) -> Void = { allowed in
            DispatchQueue.main.async {
                operation.working = false
                guard !operation.cancelled else { owner.fail(operation, "cancelled: camera"); return }
                guard allowed else { owner.fail(operation, "camera_permission: allow camera access in Settings"); return }
                let camera = NativeCamera(owner: owner, operation: operation, stagingDir: stagingDir, maxBytes: maxBytes)
                operation.delegate = camera
                operation.cancellationHandler = { [weak camera] in camera?.stopWithError("cancelled: camera") }
                do { try owner.present(camera, for: operation) }
                catch { owner.fail(operation, error.localizedDescription) }
            }
        }
        switch AVCaptureDevice.authorizationStatus(for: .video) {
        case .authorized: authorized(true)
        case .notDetermined: AVCaptureDevice.requestAccess(for: .video, completionHandler: authorized)
        default: authorized(false)
        }
    }

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = .black
        let layer = AVCaptureVideoPreviewLayer(session: session)
        layer.videoGravity = .resizeAspectFill
        view.layer.addSublayer(layer); preview = layer
        let cancel = UIButton(type: .system)
        cancel.setTitle("取消", for: .normal)
        cancel.addTarget(self, action: #selector(cancelCapture), for: .touchUpInside)
        shutter.setTitle("拍照", for: .normal)
        shutter.addTarget(self, action: #selector(takePhoto), for: .touchUpInside)
        flip.setTitle("切换镜头", for: .normal)
        flip.addTarget(self, action: #selector(switchCamera), for: .touchUpInside)
        let controls = UIStackView(arrangedSubviews: [cancel, shutter, flip])
        controls.axis = .horizontal; controls.distribution = .fillEqually
        controls.backgroundColor = UIColor.black.withAlphaComponent(0.7)
        controls.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(controls)
        NSLayoutConstraint.activate([
            controls.leadingAnchor.constraint(equalTo: view.leadingAnchor),
            controls.trailingAnchor.constraint(equalTo: view.trailingAnchor),
            controls.bottomAnchor.constraint(equalTo: view.safeAreaLayoutGuide.bottomAnchor),
            controls.heightAnchor.constraint(equalToConstant: 64)
        ])
        interruption = NotificationCenter.default.addObserver(forName: .AVCaptureSessionWasInterrupted, object: session, queue: .main) { [weak self] _ in
            self?.stopWithError("camera_interrupted: capture session interrupted; please try again")
        }
        runtimeError = NotificationCenter.default.addObserver(forName: .AVCaptureSessionRuntimeError, object: session, queue: .main) { [weak self] notification in
            let error = notification.userInfo?[AVCaptureSessionErrorKey] as? Error
            self?.stopWithError("camera_runtime: \(error?.localizedDescription ?? "capture session failed")")
        }
        operation.working = true
        queue.async {
            do {
                try self.configure(position: .back)
                try self.operation.check()
                self.session.startRunning()
                DispatchQueue.main.async {
                    self.operation.working = false
                    if self.operation.cancelled { self.stopWithError("cancelled: camera") }
                }
            } catch { DispatchQueue.main.async { self.operation.working = false; self.stopWithError(error.localizedDescription) } }
        }
    }
    override func viewDidLayoutSubviews() {
        super.viewDidLayoutSubviews(); preview?.frame = view.bounds
        if let connection = preview?.connection {
            let angle = rotationAngle()
            if connection.isVideoRotationAngleSupported(angle) { connection.videoRotationAngle = angle }
        }
    }
    private func rotationAngle() -> CGFloat {
        switch view.window?.windowScene?.interfaceOrientation {
        case .landscapeLeft: return 0
        case .landscapeRight: return 180
        case .portraitUpsideDown: return 270
        default: return 90
        }
    }
    private func configure(position: AVCaptureDevice.Position) throws {
        guard let device = AVCaptureDevice.default(.builtInWideAngleCamera, for: .video, position: position) else {
            throw NativeFileError.failure("camera_unavailable: no camera for this position")
        }
        session.beginConfiguration()
        defer { session.commitConfiguration() }
        session.sessionPreset = .photo
        if let previous = input { session.removeInput(previous) }
        let next = try AVCaptureDeviceInput(device: device)
        guard session.canAddInput(next) else { throw NativeFileError.failure("camera_unavailable: cannot attach capture input") }
        session.addInput(next); input = next
        if !session.outputs.contains(output) {
            guard session.canAddOutput(output) else { throw NativeFileError.failure("camera_unavailable: cannot attach photo output") }
            session.addOutput(output)
        }
        let supported = device.activeFormat.supportedMaxPhotoDimensions.sorted { Int64($0.width) * Int64($0.height) < Int64($1.width) * Int64($1.height) }
        guard let dimension = supported.last(where: { Int64($0.width) * Int64($0.height) <= 12_000_000 }) ?? supported.first,
              Int64(dimension.width) * Int64(dimension.height) <= 24_000_000 else {
            throw NativeFileError.failure("camera_budget: device has no bounded still output")
        }
        output.maxPhotoDimensions = dimension
    }
    @objc private func switchCamera() {
        guard !capturing, !operation.working else { return }
        operation.working = true; flip.isEnabled = false
        queue.async {
            do {
                try self.configure(position: self.input?.device.position == .front ? .back : .front)
                DispatchQueue.main.async {
                    self.operation.working = false; self.flip.isEnabled = true
                    if self.operation.cancelled { self.stopWithError("cancelled: camera") }
                }
            } catch { DispatchQueue.main.async { self.operation.working = false; self.stopWithError(error.localizedDescription) } }
        }
    }
    @objc private func takePhoto() {
        guard !capturing, !operation.working else { return }
        capturing = true; operation.working = true; shutter.isEnabled = false; flip.isEnabled = false
        let angle = rotationAngle()
        queue.async {
            guard !self.terminated else { return }
            if self.operation.cancelled {
                self.terminated = true
                self.session.stopRunning()
                DispatchQueue.main.async {
                    self.capturing = false; self.operation.working = false
                    self.owner.fail(self.operation, "cancelled: camera")
                }
                return
            }
            let settings = AVCapturePhotoSettings(format: [AVVideoCodecKey: AVVideoCodecType.jpeg])
            settings.maxPhotoDimensions = self.output.maxPhotoDimensions
            if let connection = self.output.connection(with: .video) {
                if connection.isVideoRotationAngleSupported(angle) { connection.videoRotationAngle = angle }
                if connection.isVideoMirroringSupported { connection.isVideoMirrored = false }
            }
            self.output.capturePhoto(with: settings, delegate: self)
        }
    }
    func photoOutput(_ output: AVCapturePhotoOutput, didFinishProcessingPhoto photo: AVCapturePhoto, error: Error?) {
        queue.async {
            guard !self.terminated else { return }
            let result = Result { () -> NativePickedFile in
                try self.operation.check()
                if let error = error { throw error }
                guard let data = photo.fileDataRepresentation(), UInt64(data.count) <= min(self.maxBytes, 32 * 1024 * 1024) else {
                    throw NativeFileError.failure("camera_budget: encoded photo exceeds byte budget")
                }
                let root = URL(fileURLWithPath: self.stagingDir, isDirectory: true)
                try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
                let file = root.appendingPathComponent("\(self.operation.id)-\(UUID().uuidString)-camera.jpg")
                self.operation.owned.append(file)
                try data.write(to: file, options: .atomic)
                try self.operation.check()
                return NativePickedFile(path: file.path, name: "Camera.jpg", mime: "image/jpeg", size: UInt64(data.count), hash: "")
            }
            self.captureResult = result
        }
    }
    func photoOutput(_ output: AVCapturePhotoOutput, didFinishCaptureFor resolvedSettings: AVCaptureResolvedPhotoSettings, error: Error?) {
        queue.async {
            guard !self.terminated else { return }
            self.terminated = true
            let result: Result<NativePickedFile, Error>
            if let error = error { result = .failure(error) }
            else { result = self.captureResult ?? .failure(NativeFileError.failure("camera_capture: capture completed without a photo")) }
            self.captureResult = nil
            self.session.stopRunning()
            DispatchQueue.main.async {
                self.capturing = false
                self.operation.working = false
                switch result {
                case .success(let file):
                    if self.operation.cancelled { self.owner.fail(self.operation, "cancelled: camera") }
                    else { self.owner.resolve(self.operation, file, keeping: URL(fileURLWithPath: file.path)) }
                case .failure(let error): self.owner.fail(self.operation, error.localizedDescription)
                }
            }
        }
    }
    @objc private func cancelCapture() { owner.cancel(operation.id) }
    private func stopWithError(_ message: String) {
        operation.cancel()
        // Serial shutdown follows any outstanding configuration/capture work.
        queue.async {
            guard !self.terminated else { return }
            self.terminated = true
            self.session.stopRunning()
            DispatchQueue.main.async {
                self.capturing = false
                self.operation.working = false
                self.owner.fail(self.operation, message)
            }
        }
    }
    override func viewDidDisappear(_ animated: Bool) {
        super.viewDidDisappear(animated)
        queue.async { self.session.stopRunning() }
    }
    deinit {
        if let observer = interruption { NotificationCenter.default.removeObserver(observer) }
        if let observer = runtimeError { NotificationCenter.default.removeObserver(observer) }
    }
}
