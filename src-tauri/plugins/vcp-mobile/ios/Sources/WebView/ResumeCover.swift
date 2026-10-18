import UIKit

/// Presentation only; NativeWebViewResume owns snapshot lifetime and recovery decisions.
final class ResumeCover: UIView {
    var retry: (() -> Void)?
    var reload: (() -> Void)?
    private var snapshot: UIView?
    private let controls = UIStackView()

    init(snapshot: UIView?) {
        self.snapshot = snapshot
        super.init(frame: .zero)
        backgroundColor = .clear
        accessibilityViewIsModal = true
        if let snapshot = snapshot {
            snapshot.isUserInteractionEnabled = false
            snapshot.accessibilityElementsHidden = true
            snapshot.autoresizingMask = [.flexibleWidth, .flexibleHeight]
            addSubview(snapshot)
        }
        let label = UILabel()
        label.text = "界面仍在恢复"
        label.font = .preferredFont(forTextStyle: .body)
        label.textColor = .label
        label.textAlignment = .center
        label.numberOfLines = 0
        let retryButton = UIButton(type: .system)
        retryButton.setTitle("重试显示", for: .normal)
        retryButton.addTarget(self, action: #selector(retryTapped), for: .touchUpInside)
        let reloadButton = UIButton(type: .system)
        reloadButton.setTitle("重新加载界面", for: .normal)
        reloadButton.addTarget(self, action: #selector(reloadTapped), for: .touchUpInside)
        controls.axis = .vertical
        controls.spacing = 12
        controls.isLayoutMarginsRelativeArrangement = true
        controls.layoutMargins = UIEdgeInsets(top: 16, left: 20, bottom: 16, right: 20)
        controls.backgroundColor = .secondarySystemBackground
        controls.layer.cornerRadius = 12
        let content: [UIView] = [label, retryButton, reloadButton]
        for view in content { controls.addArrangedSubview(view) }
        controls.translatesAutoresizingMaskIntoConstraints = false
        controls.isHidden = true
        addSubview(controls)
        NSLayoutConstraint.activate([
            retryButton.heightAnchor.constraint(greaterThanOrEqualToConstant: 44),
            reloadButton.heightAnchor.constraint(greaterThanOrEqualToConstant: 44),
            controls.centerXAnchor.constraint(equalTo: centerXAnchor),
            controls.centerYAnchor.constraint(equalTo: centerYAnchor),
            controls.leadingAnchor.constraint(greaterThanOrEqualTo: leadingAnchor, constant: 16),
            controls.trailingAnchor.constraint(lessThanOrEqualTo: trailingAnchor, constant: -16),
            controls.widthAnchor.constraint(lessThanOrEqualToConstant: 360)
        ])
    }

    required init?(coder: NSCoder) { return nil }
    override func layoutSubviews() {
        super.layoutSubviews()
        snapshot?.frame = bounds
    }
    func showControls() { controls.isHidden = false }
    func releaseSnapshot() { snapshot?.removeFromSuperview(); snapshot = nil }
    @objc private func retryTapped() { retry?() }
    @objc private func reloadTapped() { reload?() }
}
