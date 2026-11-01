import BackgroundTasks
import Foundation
import Tauri
import UIKit

private struct ChatTaskStart: Decodable {
    let tag: String
    let title: String
}

private struct ChatTaskFinish: Decodable {
    let tag: String
    let success: Bool
}

/// Presents progress for a reply already running in Rust.
private final class NativeChatBackgroundTasks {
    static let shared = NativeChatBackgroundTasks()
    fileprivate let schedulerQueue = DispatchQueue(label: "com.vcp.avatar.chat.background")
    private let queueIdentity = DispatchSpecificKey<Bool>()
    private let scheduler = BGTaskScheduler.shared

    private final class Chat {
        let tag: String
        let identifier: String
        let startedAt = ContinuousClock.now
        var task: BGContinuedProcessingTask?
        private var timer: DispatchSourceTimer?

        init(tag: String, identifier: String) {
            self.tag = tag
            self.identifier = identifier
        }

        private func estimatedUnits() -> Int64 {
            let seconds = startedAt.duration(to: .now).components.seconds
            switch seconds {
            case ..<15:  return seconds * 40 / 15
            case ..<60:  return 40 + (seconds - 15) * 30 / 45
            case ..<180: return 70 + (seconds - 60) * 15 / 120
            case ..<600: return 85 + (seconds - 180) * 14 / 420
            default:     return 99
            }
        }

        func startTimer(on queue: DispatchQueue) {
            let timer = DispatchSource.makeTimerSource(queue: queue)
            self.timer = timer
            timer.setEventHandler { [weak self] in
                guard let self, self.timer != nil, let progress = self.task?.progress else { return }
                let units = self.estimatedUnits()
                if progress.completedUnitCount != units { progress.completedUnitCount = units }
                if units == 99 { self.stopTimer() }
            }
            timer.schedule(deadline: .now(), repeating: .seconds(1), leeway: .milliseconds(100))
            timer.activate()
        }

        func stopTimer() {
            timer?.cancel()
            timer = nil
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

    private init() {
        schedulerQueue.setSpecific(key: queueIdentity, value: true)
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

    fileprivate func begin(_ args: ChatTaskStart) throws {
        dispatchPrecondition(condition: .onQueue(schedulerQueue))
        guard chats[args.tag] == nil else { return }
        let identifier = try takeIdentifier()
        let chat = Chat(tag: args.tag, identifier: identifier)
        let request = BGContinuedProcessingTaskRequest(identifier: identifier, title: args.title, subtitle: "正在回复…")
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
        guard let task = rawTask as? BGContinuedProcessingTask else {
            rawTask.setTaskCompleted(success: false)
            return
        }
        task.progress.totalUnitCount = 100
        // A fast foreground reply can commit after .fail submission succeeds
        // but before this accepted task is delivered. Settle that exact job.
        if let outcome = finishedBeforeLaunch.removeValue(forKey: task.identifier) {
            complete(task, success: outcome)
            return
        }
        guard let chat = chats.values.first(where: { $0.identifier == task.identifier }) else {
            task.setTaskCompleted(success: false)
            return
        }
        chat.task = task
        task.expirationHandler = { [weak chat] in
            guard let chat else { return }
            let expire = {
                guard self.chats[chat.tag] === chat else { return }
                self.finish(tag: chat.tag, success: false)
            }
            if DispatchQueue.getSpecific(key: self.queueIdentity) == true { expire() }
            else { self.schedulerQueue.sync(execute: expire) }
        }
        chat.startTimer(on: schedulerQueue)
    }

    private func complete(_ task: BGContinuedProcessingTask, success: Bool) {
        task.expirationHandler = nil
        if success { task.progress.completedUnitCount = task.progress.totalUnitCount }
        task.setTaskCompleted(success: success)
        freeIdentifiers.append(task.identifier)
    }

    fileprivate func finish(tag: String, success: Bool) {
        dispatchPrecondition(condition: .onQueue(schedulerQueue))
        guard let chat = chats.removeValue(forKey: tag) else { return }
        if let task = chat.task {
            chat.stopTimer()
            complete(task, success: success)
        } else {
            finishedBeforeLaunch[chat.identifier] = success
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
            guard UIApplication.shared.applicationState == .active else {
                invoke.reject("chat_background_task_requires_active_app")
                return
            }
            chatCommand(invoke, ChatTaskStart.self, on: owner.schedulerQueue, owner.begin)
        }
    }

    @objc public func finishChatBackgroundTask(_ invoke: Invoke) {
        let owner = NativeChatBackgroundTasks.shared
        chatCommand(invoke, ChatTaskFinish.self, on: owner.schedulerQueue) { owner.finish(tag: $0.tag, success: $0.success) }
    }
}
