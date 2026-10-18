import Foundation
import CoreGraphics
import ImageIO
import Photos
import Tauri
import UniformTypeIdentifiers

struct NativeDerivedImage: Encodable {
    let path: String
    let mime: String
    let size: UInt64
    let width: Int
    let height: Int
}
private struct DeriveImageArgs: Decodable {
    let sourcePath: String; let outputPath: String
    let maxDimension: Int; let maxPixels: UInt64; let maxBytes: UInt64
    let quality: Double; let preserveAlpha: Bool
}
private struct SavePhotoArgs: Decodable { let imagePath: String; let fileName: String? }
private struct SavePhotoSourceArgs: Decodable { let sourceUrl: String; let fileName: String? }

enum NativeImages {
    static let queue = DispatchQueue(label: "com.vcp.avatar.image-io", qos: .userInitiated)
    private static let decodeLock = NSLock()
    private static var savingPhoto = false // image queue; Photos keeps one owned copy

    static func derive(source: URL, output: URL, dimension: Int, pixels: UInt64, bytes: UInt64, quality: Double, preserveAlpha: Bool) throws -> NativeDerivedImage {
        decodeLock.lock()
        defer { decodeLock.unlock() }
        return try autoreleasepool {
            guard dimension > 0, dimension <= 8192, pixels > 0, pixels <= 33_554_432, bytes > 0 else {
                throw NativeFileError.failure("image_budget: invalid dimensions or bytes")
            }
            guard let imageSource = CGImageSourceCreateWithURL(source as CFURL, [kCGImageSourceShouldCache: false] as CFDictionary) else {
                throw NativeFileError.failure("image_decode: unsupported image")
            }
            let index = CGImageSourceGetPrimaryImageIndex(imageSource)
            guard let properties = CGImageSourceCopyPropertiesAtIndex(imageSource, index, nil) as? [CFString: Any],
                  let width = properties[kCGImagePropertyPixelWidth] as? NSNumber,
                  let height = properties[kCGImagePropertyPixelHeight] as? NSNumber,
                  width.doubleValue > 0, height.doubleValue > 0,
                  width.doubleValue <= 1_000_000, height.doubleValue <= 1_000_000 else {
                throw NativeFileError.failure("image_metadata: invalid source dimensions")
            }
            let longest = max(width.doubleValue, height.doubleValue)
            let scale = min(1, min(Double(dimension) / longest, sqrt(Double(pixels) / (width.doubleValue * height.doubleValue))))
            let target = max(1, Int(floor(longest * scale)))
            let options: [CFString: Any] = [
                kCGImageSourceCreateThumbnailFromImageAlways: true,
                kCGImageSourceCreateThumbnailWithTransform: true,
                kCGImageSourceThumbnailMaxPixelSize: target,
                kCGImageSourceShouldCacheImmediately: true,
                kCGImageSourceShouldAllowFloat: false
            ]
            guard let thumbnail = CGImageSourceCreateThumbnailAtIndex(imageSource, index, options as CFDictionary),
                  UInt64(thumbnail.width) * UInt64(thumbnail.height) <= pixels else {
                throw NativeFileError.failure("image_decode: bounded thumbnail failed")
            }
            let hasAlpha = preserveAlpha && [.first, .last, .premultipliedFirst, .premultipliedLast, .alphaOnly].contains(thumbnail.alphaInfo)
            // Explicit 8-bit sRGB destination bounds app-owned buffers and makes
            // display/model derivatives SDR regardless of the source colour space.
            guard let space = CGColorSpace(name: CGColorSpace.sRGB),
                  let context = CGContext(data: nil, width: thumbnail.width, height: thumbnail.height,
                      bitsPerComponent: 8, bytesPerRow: 0, space: space,
                      bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue) else {
                throw NativeFileError.failure("image_memory: could not allocate bounded SDR buffer")
            }
            context.draw(thumbnail, in: CGRect(x: 0, y: 0, width: CGFloat(thumbnail.width), height: CGFloat(thumbnail.height)))
            guard let normalized = context.makeImage() else { throw NativeFileError.failure("image_decode: SDR conversion failed") }
            let type = hasAlpha ? UTType.png : UTType.jpeg
            let targetURL = output.appendingPathExtension(hasAlpha ? "png" : "jpg")
            try FileManager.default.createDirectory(at: targetURL.deletingLastPathComponent(), withIntermediateDirectories: true)
            var keep = false
            defer { if !keep { try? FileManager.default.removeItem(at: targetURL) } }
            guard let destination = CGImageDestinationCreateWithURL(targetURL as CFURL, type.identifier as CFString, 1, nil) else {
                throw NativeFileError.failure("image_encode: requested encoder unavailable")
            }
            CGImageDestinationAddImage(destination, normalized, [kCGImageDestinationLossyCompressionQuality: min(1, max(0.1, quality))] as CFDictionary)
            guard CGImageDestinationFinalize(destination) else { throw NativeFileError.failure("image_encode: could not finalize output") }
            let size = try targetURL.resourceValues(forKeys: [.fileSizeKey]).fileSize ?? 0
            guard size > 0, UInt64(size) <= bytes else { throw NativeFileError.failure("image_too_large: encoded derivative exceeds byte budget") }
            keep = true
            return NativeDerivedImage(path: targetURL.path, mime: hasAlpha ? "image/png" : "image/jpeg", size: UInt64(size), width: normalized.width, height: normalized.height)
        }
    }

    static func savePhoto(path: String, name: String?, invoke: Invoke) {
        let sourceURL = URL(fileURLWithPath: path)
        queue.async {
            guard !savingPhoto else { invoke.reject("busy: a photo save is still awaiting Photos"); return }
            savingPhoto = true
            let url = NativeFileIO.temporary(name ?? sourceURL.lastPathComponent)
            let cleanup = {
                queue.async {
                    try? FileManager.default.removeItem(at: url.deletingLastPathComponent())
                    savingPhoto = false
                }
            }
            do {
                let operation = NativeFileOperation(id: UUID().uuidString, invoke: invoke)
                // savePhoto always enters on the image queue. Joining the file
                // queue here puts creation after its process-start cleanup;
                // no new Photos copy can race removal of abandoned exports.
                _ = try NativeFileIO.queue.sync {
                    try NativeFileIO.copy(sourceURL, to: url, limit: 512 * 1024 * 1024, operation: operation)
                }
                let size = try url.resourceValues(forKeys: [.fileSizeKey]).fileSize ?? 0
                guard size > 0, size <= 512 * 1024 * 1024,
                      let source = CGImageSourceCreateWithURL(url as CFURL, [kCGImageSourceShouldCache: false] as CFDictionary),
                      let sourceType = CGImageSourceGetType(source), let type = UTType(sourceType as String) else {
                    throw NativeFileError.failure("photo_input: unreadable or unsupported image")
                }
                let displayName = NativeFileIO.leaf(name ?? url.lastPathComponent)
                let submit: (PHAuthorizationStatus) -> Void = { status in
                    guard status == .authorized || status == .limited else { cleanup(); invoke.reject("photo_permission: allow adding photos in Settings"); return }
                    var assetID = ""
                    PHPhotoLibrary.shared().performChanges({
                        let request = PHAssetCreationRequest.forAsset()
                        let options = PHAssetResourceCreationOptions()
                        options.shouldMoveFile = false
                        options.originalFilename = displayName
                        request.addResource(with: .photo, fileURL: url, options: options)
                        assetID = request.placeholderForCreatedAsset?.localIdentifier ?? ""
                    }, completionHandler: { success, error in
                        defer { cleanup() }
                        if success {
                            invoke.resolve(NativeFileSaveResult(uri: assetID, displayName: displayName, mimeType: type.preferredMIMEType ?? "application/octet-stream", size: UInt64(size)))
                        } else { invoke.reject("photo_save: \(error?.localizedDescription ?? "Photos rejected the resource")") }
                    })
                }
                DispatchQueue.main.async {
                    let status = PHPhotoLibrary.authorizationStatus(for: .addOnly)
                    if status == .notDetermined { PHPhotoLibrary.requestAuthorization(for: .addOnly, handler: submit) }
                    else { submit(status) }
                }
            } catch { cleanup(); invoke.reject(error.localizedDescription) }
        }
    }
}

extension VcpMobilePlugin {
    @objc public func deriveImage(_ invoke: Invoke) {
        do {
            let args = try invoke.parseArgs(DeriveImageArgs.self)
            NativeImages.queue.async {
                do { invoke.resolve(try NativeImages.derive(source: URL(fileURLWithPath: args.sourcePath), output: URL(fileURLWithPath: args.outputPath), dimension: args.maxDimension, pixels: args.maxPixels, bytes: args.maxBytes, quality: args.quality, preserveAlpha: args.preserveAlpha)) }
                catch { invoke.reject(error.localizedDescription) }
            }
        } catch { invoke.reject(error.localizedDescription) }
    }
    @objc public func saveImageFromPath(_ invoke: Invoke) {
        do { let args = try invoke.parseArgs(SavePhotoArgs.self); NativeImages.savePhoto(path: args.imagePath, name: args.fileName, invoke: invoke) }
        catch { invoke.reject(error.localizedDescription) }
    }
    @objc public func saveImageToGallery(_ invoke: Invoke) {
        do {
            let args = try invoke.parseArgs(SavePhotoSourceArgs.self)
            let path = args.sourceUrl.hasPrefix("file://") ? URL(string: args.sourceUrl)?.path : args.sourceUrl
            guard let path = path, path.hasPrefix("/") else { throw NativeFileError.failure("photo_input: prepare the image as a local export file first") }
            NativeImages.savePhoto(path: path, name: args.fileName, invoke: invoke)
        } catch { invoke.reject(error.localizedDescription) }
    }
}
