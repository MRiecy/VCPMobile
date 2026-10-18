import Foundation

struct NativePickedFile: Encodable {
    let path: String
    let name: String
    let mime: String
    let size: UInt64
    let hash: String
    let thumbnailPath: String? = nil
}

struct NativeFileSaveResult: Encodable {
    let uri: String
    let displayName: String
    let mimeType: String
    let size: UInt64
}
