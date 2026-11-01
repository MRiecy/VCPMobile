import BackgroundTasks
import Foundation
import Tauri
import UIKit

private struct ChatTaskStart: Decodable {
    let tag: String
    let title: String
    let speaker: String
}

private struct ChatTaskUpdate: Decodable {
    let tag: String
    let speaker: String
    let receivedCharacters: Int64
}

private struct ChatTaskFinish: Decodable {
    let tag: String
    let success: Bool
}

private struct ChatCommit: Decodable {
    let tag: String
}

/// A Rust chat already owns the HTTP request. The scheduler grants runtime and
/// presents its progress; launching a BGTask never initiates a second request.
final class NativeChatBackgroundTasks {
    static let shared = NativeChatBackgroundTasks()
    fileprivate let schedulerQueue = DispatchQueue(label: "com.vcp.avatar.chat.background")
    private let queueIdentity = DispatchSpecificKey<Bool>()
    private let scheduler = BGTaskScheduler.shared

    private final class Chat {
        let tag: String
        let identifier: String
        let title: String
        let reportExpiration: (String) -> Void
        var speaker: String
        var characters: Int64 = 0
        var hasReceivedText = false
        var task: BGContinuedProcessingTask?

        init(_ args: ChatTaskStart, identifier: String, reportExpiration: @escaping (String) -> Void) {
            tag = args.tag
            title = args.title
            speaker = args.speaker
            self.identifier = identifier
            self.reportExpiration = reportExpiration
        }

        var subtitle: String {
            if speaker.isEmpty { return "正在等待发言…" }
            if characters == 0 { return "正在等待\(speaker)回复…" }
            return "\(speaker)正在回复 · 已接收 \(characters) 字"
        }

        func showProgress() {
            guard let task else { return }
            task.updateTitle(title, subtitle: subtitle)
            task.progress.completedUnitCount = hasReceivedText ? 2 : 1
        }
    }

    // Scheduler operations and all Chat objects are confined to schedulerQueue.
    private var chats: [String: Chat] = [:]
    private var finishedBeforeLaunch: [String: Bool] = [:]
    private var freeIdentifiers: [String] = []
    private var registeredCount = 0
    // The scheduler retains registered handlers for this process. Bound their
    // count even if an accepted launch is delayed indefinitely by the system.
    private let maximumRegisteredSlots = 16
    // UIKit assertions and the Rust event subscription are main-queue owned.
    private var commits: [String: UIBackgroundTaskIdentifier] = [:]
    fileprivate private(set) var onExpiration: ((String) -> Void)?

    private init() {
        schedulerQueue.setSpecific(key: queueIdentity, value: true)
    }

    func setExpirationHandler(_ handler: ((String) -> Void)?) {
        dispatchPrecondition(condition: .onQueue(.main))
        onExpiration = handler
    }

    private func unavailable(_ reason: String) -> NSError {
        NSError(domain: "VcpChatBackgroundTask", code: 1, userInfo: [NSLocalizedDescriptionKey: reason])
    }

    private func takeIdentifier() throws -> String {
        if let identifier = freeIdentifiers.popLast() { return identifier }
        guard registeredCount < maximumRegisteredSlots else { throw unavailable("chat_background_slots_in_use") }
        guard let bundle = Bundle.main.bundleIdentifier else { throw unavailable("missing_bundle_identifier") }
        let identifier = "\(bundle).chat.\(registeredCount)"
        // Register each concrete slot exactly once. Reuse it after its task
        // finishes so the scheduler doesn't retain a handler for every message.
        guard scheduler.register(forTaskWithIdentifier: identifier, using: schedulerQueue, launchHandler: { task in
            self.launch(task)
        }) else { throw unavailable("chat_background_identifier_not_permitted") }
        registeredCount += 1
        return identifier
    }

    fileprivate func begin(_ args: ChatTaskStart, reportExpiration: @escaping (String) -> Void) throws {
        dispatchPrecondition(condition: .onQueue(schedulerQueue))
        guard chats[args.tag] == nil else { return }
        let identifier = try takeIdentifier()
        let chat = Chat(args, identifier: identifier, reportExpiration: reportExpiration)
        let request = BGContinuedProcessingTaskRequest(identifier: identifier, title: chat.title, subtitle: chat.subtitle)
        request.strategy = .fail
        do {
            try scheduler.submit(request)
            chats[chat.tag] = chat
        } catch {
            freeIdentifiers.append(identifier)
            throw error
        }
    }

    private func launch(_ rawTask: BGTask) {
        dispatchPrecondition(condition: .onQueue(schedulerQueue))
        // A fast foreground reply can commit after .fail submission succeeds
        // but before this accepted task is delivered. Settle that exact job.
        if let outcome = finishedBeforeLaunch.removeValue(forKey: rawTask.identifier) {
            rawTask.setTaskCompleted(success: outcome)
            freeIdentifiers.append(rawTask.identifier)
            return
        }
        guard let task = rawTask as? BGContinuedProcessingTask,
              let chat = chats.values.first(where: { $0.identifier == task.identifier }) else {
            rawTask.setTaskCompleted(success: false)
            return
        }
        chat.task = task
        task.expirationHandler = { [weak chat] in
            guard let chat else { return }
            let expire = {
                guard self.chats[chat.tag] === chat else { return }
                // The immutable process channel synchronously signals Rust's
                // cancellation token before completion permits iOS to suspend us.
                chat.reportExpiration(chat.tag)
                self.finish(tag: chat.tag, success: false)
            }
            if DispatchQueue.getSpecific(key: self.queueIdentity) == true { expire() }
            else { self.schedulerQueue.sync(execute: expire) }
        }
        // Count milestones (request started, first text received, result
        // committed); the server doesn't supply the eventual character count.
        task.progress.totalUnitCount = 3
        chat.showProgress()
    }

    fileprivate func update(_ args: ChatTaskUpdate) {
        dispatchPrecondition(condition: .onQueue(schedulerQueue))
        guard let chat = chats[args.tag] else { return }
        chat.speaker = args.speaker
        chat.characters = args.receivedCharacters
        if args.receivedCharacters > 0 { chat.hasReceivedText = true }
        chat.showProgress()
    }

    fileprivate func finish(tag: String, success: Bool) {
        dispatchPrecondition(condition: .onQueue(schedulerQueue))
        guard let chat = chats.removeValue(forKey: tag) else { return }
        if let task = chat.task {
            task.expirationHandler = nil
            if success {
                task.progress.completedUnitCount = 3
            }
            task.setTaskCompleted(success: success)
            freeIdentifiers.append(chat.identifier)
        } else {
            finishedBeforeLaunch[chat.identifier] = success
        }
    }

    fileprivate func beginCommit(tag: String) throws {
        dispatchPrecondition(condition: .onQueue(.main))
        guard commits[tag] == nil else { return }
        let identifier = UIApplication.shared.beginBackgroundTask(withName: "Save chat reply") { [weak self] in
            self?.endCommit(tag: tag)
        }
        guard identifier != .invalid else { throw unavailable("chat_commit_background_time_unavailable") }
        commits[tag] = identifier
    }

    fileprivate func endCommit(tag: String) {
        dispatchPrecondition(condition: .onQueue(.main))
        if let identifier = commits.removeValue(forKey: tag) {
            UIApplication.shared.endBackgroundTask(identifier)
        }
    }
}

private func chatCommand<Args: Decodable>(_ invoke: Invoke, _ type: Args.Type, on queue: DispatchQueue,
                                          _ action: @escaping (Args) throws -> Void) {
    do {
        let args = try invoke.parseArgs(type)
        queue.async {
            do {
                try action(args)
                invoke.resolve()
            } catch { invoke.reject(error.localizedDescription) }
        }
    } catch { invoke.reject("invalid_chat_background_args: \(error.localizedDescription)") }
}

extension VcpMobilePlugin {
    @objc public func beginChatBackgroundTask(_ invoke: Invoke) {
        let owner = NativeChatBackgroundTasks.shared
        DispatchQueue.main.async {
            guard UIApplication.shared.applicationState == .active, let sender = owner.onExpiration else {
                invoke.reject("chat_background_task_requires_active_app")
                return
            }
            chatCommand(invoke, ChatTaskStart.self, on: owner.schedulerQueue) { try owner.begin($0, reportExpiration: sender) }
        }
    }

    @objc public func updateChatBackgroundTask(_ invoke: Invoke) {
        let owner = NativeChatBackgroundTasks.shared
        chatCommand(invoke, ChatTaskUpdate.self, on: owner.schedulerQueue, owner.update)
    }

    @objc public func finishChatBackgroundTask(_ invoke: Invoke) {
        let owner = NativeChatBackgroundTasks.shared
        chatCommand(invoke, ChatTaskFinish.self, on: owner.schedulerQueue) { owner.finish(tag: $0.tag, success: $0.success) }
    }

    @objc public func beginChatFinalization(_ invoke: Invoke) {
        chatCommand(invoke, ChatCommit.self, on: .main) { try NativeChatBackgroundTasks.shared.beginCommit(tag: $0.tag) }
    }

    @objc public func endChatFinalization(_ invoke: Invoke) {
        chatCommand(invoke, ChatCommit.self, on: .main) { NativeChatBackgroundTasks.shared.endCommit(tag: $0.tag) }
    }
}
