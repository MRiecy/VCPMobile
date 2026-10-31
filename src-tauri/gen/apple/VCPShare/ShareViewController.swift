import UIKit
import UniformTypeIdentifiers

/// Presents one acquisition result; provider work and its terminal decision belong to ShareAcquisition.
final class ShareViewController: UIViewController {
    private lazy var acquisition = ShareAcquisition { [weak self] outcome in
        self?.receive(outcome)
    }
    private let label = UILabel()
    private let button = UIButton(type: .system)

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = .systemBackground
        label.numberOfLines = 0
        label.textAlignment = .center
        label.text = "正在保存分享…"
        button.setTitle("取消", for: .normal)
        button.addTarget(self, action: #selector(close), for: .touchUpInside)
        let stack = UIStackView(arrangedSubviews: [label, button])
        stack.axis = .vertical
        stack.spacing = 24
        stack.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(stack)
        NSLayoutConstraint.activate([
            stack.leadingAnchor.constraint(equalTo: view.safeAreaLayoutGuide.leadingAnchor, constant: 24),
            stack.trailingAnchor.constraint(equalTo: view.safeAreaLayoutGuide.trailingAnchor, constant: -24),
            stack.centerYAnchor.constraint(equalTo: view.safeAreaLayoutGuide.centerYAnchor)
        ])
    }

    override func viewDidAppear(_ animated: Bool) {
        super.viewDidAppear(animated)
        let items = extensionContext?.inputItems as? [NSExtensionItem] ?? []
        let providers = items.flatMap { $0.attachments ?? [] }
        let initialText = items.filter { item in
            !(item.attachments ?? []).contains { $0.hasItemConformingToTypeIdentifier(UTType.plainText.identifier) || $0.hasItemConformingToTypeIdentifier(UTType.url.identifier) }
        }.compactMap { $0.attributedContentText?.string }
        acquisition.start(providers, initialText: initialText)
    }

    private func receive(_ outcome: ShareAcquisition.Outcome) {
        dispatchPrecondition(condition: .onQueue(.main))
        switch outcome {
        case .cancelled(let error):
            extensionContext?.cancelRequest(withError: error)
            return
        case .failed(let error):
            label.text = error.localizedDescription
            button.setTitle("关闭", for: .normal)
        case .ready:
            label.text = "已保存。打开 VCPMobile，选择助手后继续。"
            button.setTitle("完成", for: .normal)
        }
        button.removeTarget(self, action: #selector(close), for: .touchUpInside)
        button.addTarget(self, action: #selector(complete), for: .touchUpInside)
    }

    @objc private func close() { acquisition.cancel() }
    @objc private func complete() { extensionContext?.completeRequest(returningItems: nil) }
}
