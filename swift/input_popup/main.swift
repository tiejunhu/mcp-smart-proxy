import AppKit
import Foundation

private let otherLabel = "Other"

struct PopupInputRequest: Decodable {
    let questions: [PopupQuestion]
}

struct PopupQuestion: Decodable {
    let id: String
    let question: String
    let options: [PopupOption]
}

struct PopupOption: Decodable {
    let label: String
    let description: String
}

struct PopupInputResponse: Encodable {
    let answers: [String: PopupAnswerValue]

    static func cancelled() -> Self {
        Self(answers: [:])
    }
}

struct PopupAnswerValue: Encodable {
    let answers: [String]
}

final class AutoSelectingTextField: NSTextField {
    var onFocus: (() -> Void)?

    override func becomeFirstResponder() -> Bool {
        onFocus?()
        return super.becomeFirstResponder()
    }

    override func mouseDown(with event: NSEvent) {
        onFocus?()
        super.mouseDown(with: event)
    }
}

final class QuestionView: NSStackView, NSTextFieldDelegate {
    private let question: PopupQuestion
    private var optionButtons: [NSButton] = []
    private let customField = AutoSelectingTextField(string: "")

    init(question: PopupQuestion) {
        self.question = question
        super.init(frame: .zero)
        orientation = .vertical
        alignment = .leading
        spacing = 8
        translatesAutoresizingMaskIntoConstraints = false

        let promptLabel = NSTextField(wrappingLabelWithString: question.question)
        promptLabel.font = .systemFont(ofSize: 13, weight: .semibold)
        promptLabel.maximumNumberOfLines = 0

        customField.placeholderString = "Enter a custom answer"
        customField.delegate = self
        customField.onFocus = { [weak self] in
            self?.selectOtherOption()
        }
        customField.translatesAutoresizingMaskIntoConstraints = false

        addArrangedSubview(promptLabel)
        addArrangedSubview(optionsView())
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        fatalError("init(coder:) has not been implemented")
    }

    var selectedAnswer: String? {
        guard let selectedIndex else {
            return nil
        }

        let option = question.options[selectedIndex]
        if isOther(option) {
            let answer = customField.stringValue.trimmingCharacters(in: .whitespacesAndNewlines)
            return answer.isEmpty ? nil : answer
        }

        return option.label
    }

    func focusFirstInvalidControl() {
        guard let selectedIndex else {
            window?.makeFirstResponder(optionButtons.first)
            return
        }

        if isOther(question.options[selectedIndex]) {
            window?.makeFirstResponder(customField)
        }
    }

    @objc
    private func selectionChanged(_ sender: NSButton) {
        guard let index = optionButtons.firstIndex(of: sender) else {
            return
        }

        selectOption(at: index)
    }

    private var selectedIndex: Int? {
        optionButtons.firstIndex(where: { $0.state == .on })
    }

    private func selectOption(at index: Int) {
        for (buttonIndex, button) in optionButtons.enumerated() {
            button.state = buttonIndex == index ? .on : .off
        }
    }

    func controlTextDidBeginEditing(_ obj: Notification) {
        guard let otherIndex = otherOptionIndex else {
            return
        }

        selectOption(at: otherIndex)
        _ = obj
    }

    func controlTextDidChange(_ obj: Notification) {
        guard let otherIndex = otherOptionIndex else {
            return
        }

        selectOption(at: otherIndex)
        _ = obj
    }

    private var otherOptionIndex: Int? {
        question.options.firstIndex(where: isOther)
    }

    private func selectOtherOption() {
        guard let otherIndex = otherOptionIndex else {
            return
        }

        selectOption(at: otherIndex)
    }

    private func optionsView() -> NSView {
        let optionsStack = NSStackView()
        optionsStack.orientation = .vertical
        optionsStack.alignment = .leading
        optionsStack.spacing = 8
        optionsStack.translatesAutoresizingMaskIntoConstraints = false

        for (index, option) in question.options.enumerated() {
            let button = NSButton(radioButtonWithTitle: "", target: self, action: #selector(selectionChanged))
            button.translatesAutoresizingMaskIntoConstraints = false
            optionButtons.append(button)

            let rowStack = NSStackView()
            rowStack.orientation = .horizontal
            rowStack.alignment = .top
            rowStack.spacing = 8
            rowStack.translatesAutoresizingMaskIntoConstraints = false
            rowStack.edgeInsets = NSEdgeInsets(top: 0, left: 0, bottom: 4, right: 0)
            rowStack.identifier = NSUserInterfaceItemIdentifier(rawValue: "\(index)")
            rowStack.addArrangedSubview(button)

            if isOther(option) {
                customField.placeholderString = option.description
                rowStack.addArrangedSubview(customField)
                NSLayoutConstraint.activate([
                    customField.widthAnchor.constraint(equalToConstant: 388),
                ])
            } else {
                let descriptionLabel = NSTextField(wrappingLabelWithString: option.description)
                descriptionLabel.maximumNumberOfLines = 0
                descriptionLabel.textColor = .labelColor
                descriptionLabel.font = .systemFont(ofSize: 12)
                descriptionLabel.isSelectable = false
                descriptionLabel.allowsEditingTextAttributes = false
                descriptionLabel.refusesFirstResponder = true
                descriptionLabel.identifier = rowStack.identifier
                rowStack.addArrangedSubview(descriptionLabel)
                let gesture = NSClickGestureRecognizer(target: self, action: #selector(descriptionClicked(_:)))
                rowStack.addGestureRecognizer(gesture)
            }

            optionsStack.addArrangedSubview(rowStack)
        }

        NSLayoutConstraint.activate([
            optionsStack.widthAnchor.constraint(equalToConstant: 420),
        ])

        return optionsStack
    }

    @objc
    private func descriptionClicked(_ sender: NSClickGestureRecognizer) {
        guard
            let rawValue = sender.view?.identifier?.rawValue,
            let index = Int(rawValue)
        else {
            return
        }

        selectOption(at: index)
    }

    private func isOther(_ option: PopupOption) -> Bool {
        option.label.trimmingCharacters(in: .whitespacesAndNewlines)
            .caseInsensitiveCompare(otherLabel) == .orderedSame
    }
}

final class PopupWindowController: NSWindowController, NSWindowDelegate {
    private let request: PopupInputRequest
    private let questionViews: [QuestionView]
    private let errorLabel = NSTextField(wrappingLabelWithString: "")
    private var isClosingProgrammatically = false
    private(set) var response = PopupInputResponse.cancelled()

    init(request: PopupInputRequest) {
        self.request = request
        self.questionViews = request.questions.map(QuestionView.init)

        let window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 560, height: Self.windowHeight(for: request)),
            styleMask: [.titled, .closable],
            backing: .buffered,
            defer: false
        )
        window.title = "Request User Input"
        window.isReleasedWhenClosed = false
        window.center()
        super.init(window: window)
        window.delegate = self
        buildInterface()
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        fatalError("init(coder:) has not been implemented")
    }

    func present() -> PopupInputResponse {
        guard let window else {
            return .cancelled()
        }

        NSApp.activate(ignoringOtherApps: true)
        showWindow(nil)
        window.makeKeyAndOrderFront(nil)
        window.makeFirstResponder(nil)

        _ = NSApp.runModal(for: window)
        return response
    }

    func windowWillClose(_ notification: Notification) {
        guard let window else {
            return
        }

        if isClosingProgrammatically {
            _ = notification
            return
        }

        response = .cancelled()
        if NSApp.modalWindow === window {
            NSApp.stopModal(withCode: .abort)
        }

        _ = notification
    }

    @objc
    private func submit(_ sender: Any?) {
        guard let firstInvalid = questionViews.first(where: { $0.selectedAnswer == nil }) else {
            let answers = Dictionary(uniqueKeysWithValues: zip(request.questions, questionViews).compactMap { pair in
                let (question, view) = pair
                return view.selectedAnswer.map { answer in
                    (question.id, PopupAnswerValue(answers: [answer]))
                }
            })
            errorLabel.stringValue = ""
            response = PopupInputResponse(answers: answers)
            close(with: .OK)
            return
        }

        errorLabel.stringValue = "Choose one answer for every question."
        firstInvalid.focusFirstInvalidControl()
        _ = sender
    }

    @objc
    private func cancel(_ sender: Any?) {
        response = .cancelled()
        close(with: .abort)
        _ = sender
    }

    private func close(with code: NSApplication.ModalResponse) {
        guard let window else {
            return
        }

        isClosingProgrammatically = true
        NSApp.stopModal(withCode: code)
        window.orderOut(nil)
        window.close()
    }

    private func buildInterface() {
        guard let contentView = window?.contentView else {
            return
        }

        let rootStack = NSStackView()
        rootStack.orientation = .vertical
        rootStack.alignment = .leading
        rootStack.spacing = 16
        rootStack.translatesAutoresizingMaskIntoConstraints = false

        errorLabel.textColor = .systemRed
        errorLabel.maximumNumberOfLines = 0
        errorLabel.stringValue = ""
        errorLabel.isHidden = false

        for view in questionViews {
            rootStack.addArrangedSubview(view)
        }
        rootStack.addArrangedSubview(errorLabel)
        rootStack.addArrangedSubview(buttonRow())

        contentView.addSubview(rootStack)

        NSLayoutConstraint.activate([
            rootStack.topAnchor.constraint(equalTo: contentView.topAnchor, constant: 20),
            rootStack.leadingAnchor.constraint(equalTo: contentView.leadingAnchor, constant: 20),
            rootStack.trailingAnchor.constraint(equalTo: contentView.trailingAnchor, constant: -20),
            rootStack.bottomAnchor.constraint(equalTo: contentView.bottomAnchor, constant: -20),
        ])
    }

    private func buttonRow() -> NSView {
        let buttons = NSStackView()
        buttons.orientation = .horizontal
        buttons.spacing = 12
        buttons.alignment = .centerY
        buttons.translatesAutoresizingMaskIntoConstraints = false

        let spacer = NSView()
        spacer.translatesAutoresizingMaskIntoConstraints = false
        spacer.setContentHuggingPriority(.defaultLow, for: .horizontal)

        let cancelButton = NSButton(title: "Cancel", target: self, action: #selector(cancel))
        let submitButton = NSButton(title: "Submit", target: self, action: #selector(submit))
        submitButton.keyEquivalent = "\r"

        buttons.addArrangedSubview(spacer)
        buttons.addArrangedSubview(cancelButton)
        buttons.addArrangedSubview(submitButton)

        return buttons
    }

    private static func windowHeight(for request: PopupInputRequest) -> CGFloat {
        let questionCount = max(request.questions.count, 1)
        return CGFloat(min(220 + questionCount * 120, 640))
    }
}

enum PopupInputError: Error {
    case invalidRequest(String)
    case invalidResponse(String)
}

extension PopupInputError: LocalizedError {
    var errorDescription: String? {
        switch self {
        case let .invalidRequest(message), let .invalidResponse(message):
            return message
        }
    }
}

private func decodeRequest(from data: Data) throws -> PopupInputRequest {
    guard !data.isEmpty else {
        throw PopupInputError.invalidRequest("missing popup input request JSON on stdin")
    }

    do {
        return try JSONDecoder().decode(PopupInputRequest.self, from: data)
    } catch {
        throw PopupInputError.invalidRequest("failed to decode popup input request JSON: \(error)")
    }
}

private func showPopup(for request: PopupInputRequest) -> PopupInputResponse {
    guard !request.questions.isEmpty else {
        return .cancelled()
    }

    let app = NSApplication.shared
    app.setActivationPolicy(.accessory)
    app.finishLaunching()

    let controller = PopupWindowController(request: request)
    return controller.present()
}

private func writeResponse(_ response: PopupInputResponse) throws {
    do {
        let data = try JSONEncoder().encode(response)
        FileHandle.standardOutput.write(data)
    } catch {
        throw PopupInputError.invalidResponse("failed to encode popup input response JSON: \(error)")
    }
}

do {
    let request = try decodeRequest(from: FileHandle.standardInput.readDataToEndOfFile())
    let response = showPopup(for: request)
    try writeResponse(response)
} catch {
    fputs("\(error.localizedDescription)\n", stderr)
    exit(1)
}
