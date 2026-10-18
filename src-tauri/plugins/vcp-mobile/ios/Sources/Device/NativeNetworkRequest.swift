import Darwin
import Foundation
import Network
import Tauri

private struct NetworkSnapshot: Encodable {
    let connected: Bool
    let type: String
    let downSpeedKbps: Int? = nil
    let upSpeedKbps: Int? = nil
    let ip: String?
    let isExpensive: Bool
    let isConstrained: Bool
    let unavailable: [String: String]
}

// A one-shot monitor owns its deadline and never reads the unevaluated currentPath.
// Completion and timeout are serialized on main; the deadline retains the request
// until a real callback or timeout, independently of the Rust caller's lifetime.
private final class NativeNetworkRequest {
    private var invoke: Invoke?
    private var monitor: NWPathMonitor?
    private var deadline: DispatchWorkItem?
    private let queue = DispatchQueue(label: "com.vcp.avatar.network-snapshot")

    init(_ invoke: Invoke) { self.invoke = invoke }

    func start() {
        let monitor = NWPathMonitor()
        self.monitor = monitor
        monitor.pathUpdateHandler = { [weak self] path in
            DispatchQueue.main.async { self?.complete(path) }
        }
        let deadline = DispatchWorkItem { [self] in finishError("network_timeout: 未收到网络路径快照") }
        self.deadline = deadline
        DispatchQueue.main.asyncAfter(deadline: .now() + 2, execute: deadline)
        monitor.start(queue: queue)
    }

    private func takeInvoke() -> Invoke? {
        dispatchPrecondition(condition: .onQueue(.main))
        let current = invoke
        invoke = nil
        monitor?.cancel()
        monitor?.pathUpdateHandler = nil
        monitor = nil
        deadline?.cancel()
        deadline = nil
        return current
    }

    private func finishError(_ message: String) { takeInvoke()?.reject(message) }

    private func complete(_ path: NWPath) {
        guard let invoke = takeInvoke() else { return }
        let connected = path.status == .satisfied
        var types: [String] = []
        if path.usesInterfaceType(.wifi) { types.append("Wi-Fi") }
        if path.usesInterfaceType(.cellular) { types.append("蜂窝网络") }
        if path.usesInterfaceType(.wiredEthernet) { types.append("以太网") }
        if path.usesInterfaceType(.other) { types.append("其他接口") }
        let interfaces = Set(path.availableInterfaces.filter { path.usesInterfaceType($0.type) }.map { $0.name })
        let ip = connected ? Self.addresses(interfaces).joined(separator: ", ") : ""
        var missing = [
            "downSpeedKbps": "NWPath 不提供链路下行速率",
            "upSpeedKbps": "NWPath 不提供链路上行速率",
            "serverReachability": "路径可用不代表服务器可达"
        ]
        if ip.isEmpty { missing["ip"] = connected ? "未取得活动接口地址" : "当前没有可用网络路径" }
        invoke.resolve(NetworkSnapshot(
            connected: connected, type: types.isEmpty ? "无可用接口" : types.joined(separator: "/"),
            ip: ip.isEmpty ? nil : ip, isExpensive: path.isExpensive,
            isConstrained: path.isConstrained, unavailable: missing
        ))
    }

    private static func addresses(_ names: Set<String>) -> [String] {
        var head: UnsafeMutablePointer<ifaddrs>?
        guard getifaddrs(&head) == 0 else { return [] }
        defer { freeifaddrs(head) }
        var addresses: [String] = []
        var cursor = head
        while let current = cursor {
            defer { cursor = current.pointee.ifa_next }
            let item = current.pointee
            guard let address = item.ifa_addr, names.contains(String(cString: item.ifa_name)),
                  item.ifa_flags & UInt32(IFF_UP) != 0,
                  item.ifa_flags & UInt32(IFF_LOOPBACK) == 0,
                  address.pointee.sa_family == UInt8(AF_INET) || address.pointee.sa_family == UInt8(AF_INET6)
            else { continue }
            var buffer = [CChar](repeating: 0, count: Int(NI_MAXHOST))
            let bufferCapacity = socklen_t(buffer.count)
            if getnameinfo(address, socklen_t(address.pointee.sa_len), &buffer,
                           bufferCapacity, nil, 0, NI_NUMERICHOST) == 0 {
                addresses.append(String(cString: buffer))
            }
        }
        return Array(Set(addresses)).sorted()
    }
}

extension VcpMobilePlugin {
    @objc public func getNetworkStatus(_ invoke: Invoke) {
        DispatchQueue.main.async { NativeNetworkRequest(invoke).start() }
    }
}
