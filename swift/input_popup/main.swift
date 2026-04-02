import AppKit
import Foundation

private let otherLabel = "Other"
private let optionShortcutKeys = Array("123456789abcdefghijklmnopqrstuvwxyz")
private let optionShortcutKeySet = Set(optionShortcutKeys)

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

private enum AnswerInputSource {
    case keyboard
    case mouse
}

enum QuestionPresentationState {
    case pending
    case active
    case answered
}

final class RoundedContainerView: NSView {
    var fillColor: NSColor = .controlBackgroundColor {
        didSet { updateAppearance() }
    }

    var strokeColor: NSColor = .separatorColor.withAlphaComponent(0.45) {
        didSet { updateAppearance() }
    }

    var cornerRadius: CGFloat = 12 {
        didSet { updateAppearance() }
    }

    override init(frame frameRect: NSRect) {
        super.init(frame: frameRect)
        translatesAutoresizingMaskIntoConstraints = false
        wantsLayer = true
        updateAppearance()
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        fatalError("init(coder:) has not been implemented")
    }

    private func updateAppearance() {
        layer?.cornerRadius = cornerRadius
        layer?.backgroundColor = fillColor.cgColor
        layer?.borderColor = strokeColor.cgColor
        layer?.borderWidth = 1
    }
}

final class PopupWindow: NSWindow {
    var onShortcutKey: ((Character) -> Bool)?

    override func performKeyEquivalent(with event: NSEvent) -> Bool {
        let disallowedModifiers = NSEvent.ModifierFlags([.command, .control, .option, .function])
        guard
            let onShortcutKey,
            event.type == .keyDown,
            event.modifierFlags.intersection(disallowedModifiers).isEmpty,
            let character = event.charactersIgnoringModifiers?.lowercased(),
            character.count == 1,
            let shortcut = character.first,
            optionShortcutKeySet.contains(shortcut)
        else {
            return super.performKeyEquivalent(with: event)
        }

        if onShortcutKey(shortcut) {
            return true
        }

        return super.performKeyEquivalent(with: event)
    }
}

final class AutoSelectingTextField: NSTextField {
    var onMouseDown: (() -> Void)?

    override init(frame frameRect: NSRect) {
        super.init(frame: frameRect)
        isEditable = true
        isSelectable = true
        isBordered = true
        isBezeled = true
        drawsBackground = true
    }

    convenience init(string: String) {
        self.init(frame: .zero)
        stringValue = string
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        fatalError("init(coder:) has not been implemented")
    }

    override func mouseDown(with event: NSEvent) {
        onMouseDown?()
        super.mouseDown(with: event)
    }
}

final class FlippedView: NSView {
    override var isFlipped: Bool {
        true
    }
}

final class QuestionView: NSStackView, NSTextFieldDelegate {
    private let question: PopupQuestion
    private let shortcuts: [Character?]
    private let promptLabel: NSTextField
    private let sectionCard = RoundedContainerView()
    private var optionButtons: [NSButton] = []
    private var optionRows: [RoundedContainerView] = []
    private let customField = AutoSelectingTextField(string: "")
    private var selectedIndex: Int?
    private var pendingKeyboardOtherConfirmation = false
    private var presentationState: QuestionPresentationState = .pending
    private var isInteractionEnabled = true
    var onAnswerStateChanged: (() -> Void)?
    var onInteraction: (() -> Void)?
    var onAnswerCommitted: (() -> Void)?

    init(question: PopupQuestion, shortcuts: [Character?]) {
        self.question = question
        self.shortcuts = shortcuts
        self.promptLabel = NSTextField(wrappingLabelWithString: question.question)
        super.init(frame: .zero)
        orientation = .vertical
        alignment = .leading
        spacing = 8
        translatesAutoresizingMaskIntoConstraints = false

        promptLabel.font = .systemFont(ofSize: 15, weight: .semibold)
        promptLabel.maximumNumberOfLines = 0
        promptLabel.translatesAutoresizingMaskIntoConstraints = false

        customField.placeholderString = "Type your answer"
        customField.delegate = self
        customField.font = .systemFont(ofSize: 13)
        customField.controlSize = .regular
        customField.onMouseDown = { [weak self] in
            self?.selectOtherOption()
        }
        customField.translatesAutoresizingMaskIntoConstraints = false

        sectionCard.fillColor = .controlBackgroundColor.withAlphaComponent(0.72)
        sectionCard.strokeColor = .separatorColor.withAlphaComponent(0.55)
        sectionCard.cornerRadius = 14

        let options = optionsView()
        sectionCard.addSubview(options)

        addArrangedSubview(promptLabel)
        addArrangedSubview(sectionCard)

        NSLayoutConstraint.activate([
            promptLabel.widthAnchor.constraint(equalTo: widthAnchor),
            sectionCard.widthAnchor.constraint(equalTo: widthAnchor),
            options.topAnchor.constraint(equalTo: sectionCard.topAnchor, constant: 12),
            options.leadingAnchor.constraint(equalTo: sectionCard.leadingAnchor, constant: 12),
            options.trailingAnchor.constraint(equalTo: sectionCard.trailingAnchor, constant: -12),
            options.bottomAnchor.constraint(equalTo: sectionCard.bottomAnchor, constant: -12),
        ])

        setQuestionState(.pending, isInteractive: false)
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

    var isAnswered: Bool {
        selectedAnswer != nil
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

    func activateShortcut(at index: Int) {
        guard isInteractionEnabled else {
            return
        }

        recordInteraction()
        if isOther(question.options[index]) {
            beginKeyboardOtherSelection(at: index)
            return
        }

        selectOption(at: index, inputSource: .keyboard)
        onAnswerCommitted?()
    }

    func setQuestionState(_ state: QuestionPresentationState, isInteractive: Bool) {
        presentationState = state
        isInteractionEnabled = isInteractive
        optionButtons.forEach { $0.isEnabled = isInteractive }
        customField.isEnabled = isInteractive
        updatePresentationUI()
    }

    func autoSelectFirstOption() -> Bool {
        guard !question.options.isEmpty else {
            return false
        }

        selectOption(at: 0, inputSource: nil)
        return selectedAnswer != nil
    }

    func isEditingCustomField() -> Bool {
        guard let editor = customField.currentEditor() else {
            return false
        }
        return window?.firstResponder === editor
    }

    @objc
    private func selectionChanged(_ sender: NSButton) {
        guard isInteractionEnabled else {
            return
        }

        guard let index = optionButtons.firstIndex(of: sender) else {
            return
        }

        recordInteraction()
        selectOption(at: index, inputSource: .mouse)

        if !isOther(question.options[index]) {
            onAnswerCommitted?()
        }
    }

    private func selectOption(at index: Int, inputSource: AnswerInputSource?) {
        selectedIndex = index
        pendingKeyboardOtherConfirmation = false
        refreshSelectionUI()
        onAnswerStateChanged?()

        guard let window else {
            return
        }

        if isOther(question.options[index]), inputSource == .mouse {
            window.makeFirstResponder(customField)
        } else {
            window.endEditing(for: nil)
        }
    }

    func controlTextDidBeginEditing(_ obj: Notification) {
        guard isInteractionEnabled else {
            return
        }

        guard let otherIndex = otherOptionIndex else {
            return
        }

        recordInteraction()
        selectedIndex = otherIndex
        refreshSelectionUI()
        onAnswerStateChanged?()
        _ = obj
    }

    func controlTextDidChange(_ obj: Notification) {
        guard isInteractionEnabled else {
            return
        }

        guard let otherIndex = otherOptionIndex else {
            return
        }

        recordInteraction()
        selectedIndex = otherIndex
        refreshSelectionUI()
        onAnswerStateChanged?()
        _ = obj
    }

    func control(
        _ control: NSControl,
        textView: NSTextView,
        doCommandBy commandSelector: Selector
    ) -> Bool {
        guard
            isInteractionEnabled,
            control === customField,
            commandSelector == #selector(NSResponder.insertNewline(_:))
        else {
            return false
        }

        confirmCustomFieldSelection()
        _ = textView
        return true
    }

    private var otherOptionIndex: Int? {
        question.options.firstIndex(where: isOther)
    }

    private func selectOtherOption() {
        guard isInteractionEnabled else {
            return
        }

        guard let otherIndex = otherOptionIndex else {
            return
        }

        recordInteraction()
        selectOption(at: otherIndex, inputSource: .mouse)
    }

    private func beginKeyboardOtherSelection(at index: Int) {
        selectedIndex = index
        pendingKeyboardOtherConfirmation = true
        refreshSelectionUI()
        onAnswerStateChanged?()
        window?.makeFirstResponder(customField)
    }

    private func confirmCustomFieldSelection() {
        let hasAnswer = selectedAnswer != nil
        pendingKeyboardOtherConfirmation = false
        onAnswerStateChanged?()
        window?.endEditing(for: nil)
        window?.makeFirstResponder(nil)

        if hasAnswer {
            onAnswerCommitted?()
        }
    }

    private func refreshSelectionUI() {
        for (buttonIndex, button) in optionButtons.enumerated() {
            let isSelected = buttonIndex == selectedIndex
            button.state = isSelected ? .on : .off

            let row = optionRows[buttonIndex]
            row.fillColor = isSelected
                ? .selectedContentBackgroundColor.withAlphaComponent(0.16)
                : .clear
            row.strokeColor = .clear
        }

        customField.alphaValue = selectedIndex.flatMap { isOther(question.options[$0]) ? 1.0 : 0.76 } ?? 0.76
    }

    private func updatePresentationUI() {
        switch presentationState {
        case .pending:
            alphaValue = 0.68
            promptLabel.textColor = .secondaryLabelColor
            sectionCard.fillColor = .controlBackgroundColor.withAlphaComponent(0.46)
            sectionCard.strokeColor = .separatorColor.withAlphaComponent(0.24)
        case .active:
            alphaValue = 1.0
            promptLabel.textColor = .labelColor
            sectionCard.fillColor = .controlBackgroundColor.withAlphaComponent(0.82)
            sectionCard.strokeColor = .controlAccentColor.withAlphaComponent(0.34)
        case .answered:
            alphaValue = 1.0
            promptLabel.textColor = .labelColor
            sectionCard.fillColor = .selectedContentBackgroundColor.withAlphaComponent(0.08)
            sectionCard.strokeColor = .separatorColor.withAlphaComponent(0.26)
        }
    }

    private func optionsView() -> NSView {
        let optionsStack = NSStackView()
        optionsStack.orientation = .vertical
        optionsStack.alignment = .leading
        optionsStack.spacing = 4
        optionsStack.translatesAutoresizingMaskIntoConstraints = false

        for (index, option) in question.options.enumerated() {
            let button = NSButton(radioButtonWithTitle: "", target: self, action: #selector(selectionChanged))
            button.translatesAutoresizingMaskIntoConstraints = false
            button.setContentHuggingPriority(.required, for: .horizontal)
            optionButtons.append(button)

            let rowContainer = RoundedContainerView()
            rowContainer.fillColor = .clear
            rowContainer.strokeColor = .clear
            rowContainer.cornerRadius = 10
            rowContainer.identifier = NSUserInterfaceItemIdentifier(rawValue: "\(index)")
            optionRows.append(rowContainer)

            let gesture = NSClickGestureRecognizer(target: self, action: #selector(descriptionClicked(_:)))
            rowContainer.addGestureRecognizer(gesture)

            let rowContent: NSView
            if isOther(option) {
                rowContent = otherOptionContent(for: option, button: button, shortcut: shortcuts[index])
            } else {
                rowContent = optionRowContent(
                    description: option.description,
                    button: button,
                    shortcut: shortcuts[index]
                )
            }

            rowContainer.addSubview(rowContent)
            optionsStack.addArrangedSubview(rowContainer)

            NSLayoutConstraint.activate([
                rowContent.topAnchor.constraint(equalTo: rowContainer.topAnchor, constant: 10),
                rowContent.leadingAnchor.constraint(equalTo: rowContainer.leadingAnchor, constant: 10),
                rowContent.trailingAnchor.constraint(equalTo: rowContainer.trailingAnchor, constant: -10),
                rowContent.bottomAnchor.constraint(equalTo: rowContainer.bottomAnchor, constant: -10),
                rowContainer.widthAnchor.constraint(equalTo: optionsStack.widthAnchor),
            ])
        }

        refreshSelectionUI()
        return optionsStack
    }

    private func optionRowContent(
        description: String,
        button: NSButton,
        shortcut: Character?
    ) -> NSView {
        let rowStack = NSStackView()
        rowStack.orientation = .horizontal
        rowStack.alignment = .top
        rowStack.spacing = 10
        rowStack.translatesAutoresizingMaskIntoConstraints = false

        let descriptionLabel = NSTextField(wrappingLabelWithString: description)
        descriptionLabel.maximumNumberOfLines = 0
        descriptionLabel.textColor = .labelColor
        descriptionLabel.font = .systemFont(ofSize: 13)
        descriptionLabel.isSelectable = false
        descriptionLabel.allowsEditingTextAttributes = false
        descriptionLabel.refusesFirstResponder = true

        rowStack.addArrangedSubview(button)
        rowStack.addArrangedSubview(descriptionLabel)
        rowStack.addArrangedSubview(flexibleSpacer())

        if let badge = shortcutBadge(for: shortcut) {
            rowStack.addArrangedSubview(badge)
        }

        return rowStack
    }

    private func otherOptionContent(
        for option: PopupOption,
        button: NSButton,
        shortcut: Character?
    ) -> NSView {
        let rowStack = NSStackView()
        rowStack.orientation = .horizontal
        rowStack.alignment = .centerY
        rowStack.spacing = 10
        rowStack.translatesAutoresizingMaskIntoConstraints = false

        customField.placeholderString = option.description
        customField.setContentHuggingPriority(.defaultLow, for: .horizontal)
        customField.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)

        let fieldContainer = NSView()
        fieldContainer.translatesAutoresizingMaskIntoConstraints = false
        fieldContainer.setContentHuggingPriority(.defaultLow, for: .horizontal)
        fieldContainer.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        fieldContainer.addSubview(customField)

        rowStack.addArrangedSubview(button)
        rowStack.addArrangedSubview(fieldContainer)
        rowStack.addArrangedSubview(flexibleSpacer())

        if let badge = shortcutBadge(for: shortcut) {
            rowStack.addArrangedSubview(badge)
        }

        NSLayoutConstraint.activate([
            customField.heightAnchor.constraint(equalToConstant: 28),
            customField.topAnchor.constraint(equalTo: fieldContainer.topAnchor),
            customField.leadingAnchor.constraint(equalTo: fieldContainer.leadingAnchor),
            customField.trailingAnchor.constraint(equalTo: fieldContainer.trailingAnchor),
            customField.bottomAnchor.constraint(equalTo: fieldContainer.bottomAnchor),
            fieldContainer.widthAnchor.constraint(greaterThanOrEqualToConstant: 220),
        ])

        return rowStack
    }

    @objc
    private func descriptionClicked(_ sender: NSClickGestureRecognizer) {
        guard isInteractionEnabled else {
            return
        }

        guard
            let rawValue = sender.view?.identifier?.rawValue,
            let index = Int(rawValue)
        else {
            return
        }

        recordInteraction()
        selectOption(at: index, inputSource: .mouse)

        if !isOther(question.options[index]) {
            onAnswerCommitted?()
        }
    }

    private func recordInteraction() {
        onInteraction?()
    }

    private func isOther(_ option: PopupOption) -> Bool {
        option.label.trimmingCharacters(in: .whitespacesAndNewlines)
            .caseInsensitiveCompare(otherLabel) == .orderedSame
    }

    private func shortcutBadge(for shortcut: Character?) -> NSView? {
        guard let shortcut else {
            return nil
        }

        let badge = RoundedContainerView()
        badge.fillColor = .quaternaryLabelColor.withAlphaComponent(0.08)
        badge.strokeColor = .separatorColor.withAlphaComponent(0.22)
        badge.cornerRadius = 7
        badge.setContentHuggingPriority(.required, for: .horizontal)
        badge.setContentCompressionResistancePriority(.required, for: .horizontal)

        let label = NSTextField(labelWithString: String(shortcut).uppercased())
        label.font = .monospacedSystemFont(ofSize: 11, weight: .medium)
        label.textColor = .secondaryLabelColor
        label.translatesAutoresizingMaskIntoConstraints = false

        badge.addSubview(label)
        NSLayoutConstraint.activate([
            label.topAnchor.constraint(equalTo: badge.topAnchor, constant: 3),
            label.leadingAnchor.constraint(equalTo: badge.leadingAnchor, constant: 7),
            label.trailingAnchor.constraint(equalTo: badge.trailingAnchor, constant: -7),
            label.bottomAnchor.constraint(equalTo: badge.bottomAnchor, constant: -3),
        ])

        return badge
    }

    private func flexibleSpacer() -> NSView {
        let spacer = NSView()
        spacer.translatesAutoresizingMaskIntoConstraints = false
        spacer.setContentHuggingPriority(.defaultLow, for: .horizontal)
        spacer.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        return spacer
    }
}

final class PopupWindowController: NSWindowController, NSWindowDelegate {
    private static let windowWidth: CGFloat = 620
    private static let maxWindowHeight: CGFloat = 800
    private static let questionTimeoutSeconds = 30
    private static let modalPanelRunLoopMode = RunLoop.Mode("NSModalPanelRunLoopMode")
    private static let topInset: CGFloat = 18
    private static let sideInset: CGFloat = 20
    private static let bottomInset: CGFloat = 18
    private static let contentToActionsSpacing: CGFloat = 14

    private let request: PopupInputRequest
    private let questionViews: [QuestionView]
    private let shortcutAssignments: [[Character?]]
    private let progressLabel = NSTextField(wrappingLabelWithString: "")
    private let countdownLabel = NSTextField(wrappingLabelWithString: "")
    private let errorLabel = NSTextField(wrappingLabelWithString: "")
    private let stopTimerButton = NSButton(title: "Turn Off Auto-Select", target: nil, action: nil)
    private let submitButton = NSButton(title: "Continue", target: nil, action: nil)
    private weak var contentScrollView: NSScrollView?
    private weak var contentStack: NSStackView?
    private weak var actionsView: NSView?
    private var contentScrollHeightConstraint: NSLayoutConstraint?
    private var isClosingProgrammatically = false
    private var isCountdownPermanentlyStopped = false
    private var activeQuestionIndex = 0
    private var questionTimer: Timer?
    private var remainingQuestionSeconds = 10
    private(set) var response = PopupInputResponse.cancelled()

    init(request: PopupInputRequest) {
        self.request = request
        self.shortcutAssignments = Self.assignShortcuts(for: request)
        self.questionViews = zip(request.questions, shortcutAssignments).map { question, shortcuts in
            QuestionView(question: question, shortcuts: shortcuts)
        }

        let window = PopupWindow(
            contentRect: NSRect(x: 0, y: 0, width: Self.windowWidth, height: 420),
            styleMask: [.titled, .closable],
            backing: .buffered,
            defer: false
        )
        window.title = "Request User Input"
        window.titleVisibility = .hidden
        window.titlebarAppearsTransparent = true
        window.isReleasedWhenClosed = false
        super.init(window: window)
        window.delegate = self
        window.onShortcutKey = { [weak self] shortcut in
            self?.handleShortcutKey(shortcut) ?? false
        }
        wireQuestionCallbacks()
        buildInterface()
        activateQuestion(at: 0, startCountdown: false)
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
        updateWindowSizing()
        let targetFrame = presentedFrame(for: window)
        let startFrame = hiddenStartFrame(for: targetFrame, window: window)
        window.setFrame(startFrame, display: false)
        window.makeKeyAndOrderFront(nil)
        activateQuestion(at: 0, startCountdown: true)
        focusStopTimerButtonIfNeeded()

        NSAnimationContext.runAnimationGroup { context in
            context.duration = 0.22
            context.allowsImplicitAnimation = true
            window.animator().setFrame(targetFrame, display: true)
        }

        _ = NSApp.runModal(for: window)
        return response
    }

    func windowWillClose(_ notification: Notification) {
        invalidateQuestionTimer()

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
        guard let firstInvalidIndex = questionViews.firstIndex(where: { $0.selectedAnswer == nil }) else {
            let answers = Dictionary(uniqueKeysWithValues: zip(request.questions, questionViews).compactMap { pair in
                let (question, view) = pair
                return view.selectedAnswer.map { answer in
                    (question.id, PopupAnswerValue(answers: [answer]))
                }
            })
            setValidationMessage(nil)
            response = PopupInputResponse(answers: answers)
            close(with: .OK)
            return
        }

        if firstInvalidIndex != activeQuestionIndex {
            activateQuestion(at: firstInvalidIndex, startCountdown: true)
        }

        setValidationMessage("Choose one answer for every question.")
        questionViews[firstInvalidIndex].focusFirstInvalidControl()
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

        invalidateQuestionTimer()
        isClosingProgrammatically = true
        let finalFrame = window.frame.offsetBy(dx: 0, dy: -12)

        NSAnimationContext.runAnimationGroup { context in
            context.duration = 0.16
            context.allowsImplicitAnimation = true
            window.animator().alphaValue = 0
            window.animator().setFrame(finalFrame, display: true)
        } completionHandler: {
            NSApp.stopModal(withCode: code)
            window.orderOut(nil)
            window.alphaValue = 1
            window.close()
        }
    }

    private func wireQuestionCallbacks() {
        for (index, view) in questionViews.enumerated() {
            view.onAnswerStateChanged = { [weak self] in
                self?.setValidationMessage(nil)
                self?.updateSubmitButtonState()
            }
            view.onInteraction = { [weak self] in
                self?.handleQuestionInteraction(at: index)
            }
            view.onAnswerCommitted = { [weak self] in
                self?.handleAnswerCommitted(at: index)
            }
        }
    }

    private func handleShortcutKey(_ shortcut: Character) -> Bool {
        guard
            questionViews.indices.contains(activeQuestionIndex),
            !questionViews[activeQuestionIndex].isEditingCustomField()
        else {
            return false
        }

        guard let optionIndex = shortcutAssignments[activeQuestionIndex].firstIndex(where: { $0 == shortcut }) else {
            return false
        }

        questionViews[activeQuestionIndex].activateShortcut(at: optionIndex)
        return true
    }

    private func handleQuestionInteraction(at index: Int) {
        guard index == activeQuestionIndex else {
            return
        }

        if questionTimer != nil {
            invalidateQuestionTimer()
            updateStatusLabel()
        }
    }

    private func handleAnswerCommitted(at index: Int) {
        guard index == activeQuestionIndex else {
            return
        }

        setValidationMessage(nil)

        if let nextIndex = nextUnansweredQuestionIndex(startingAt: index + 1) {
            activateQuestion(at: nextIndex, startCountdown: true)
            return
        }

        submit(nil)
    }

    private func buildInterface() {
        guard let contentView = window?.contentView else {
            return
        }

        let backgroundView = NSVisualEffectView()
        backgroundView.translatesAutoresizingMaskIntoConstraints = false
        backgroundView.material = .windowBackground
        backgroundView.blendingMode = .behindWindow
        backgroundView.state = .active

        let contentStack = NSStackView()
        contentStack.orientation = .vertical
        contentStack.alignment = .leading
        contentStack.spacing = 14
        contentStack.translatesAutoresizingMaskIntoConstraints = false
        contentStack.detachesHiddenViews = true

        errorLabel.textColor = .systemRed
        errorLabel.font = .systemFont(ofSize: 12)
        errorLabel.maximumNumberOfLines = 0
        errorLabel.stringValue = ""
        errorLabel.isHidden = true

        let header = headerView()
        header.setContentHuggingPriority(.required, for: .vertical)
        header.setContentCompressionResistancePriority(.required, for: .vertical)
        contentStack.addArrangedSubview(header)
        header.widthAnchor.constraint(equalTo: contentStack.widthAnchor).isActive = true
        contentStack.setCustomSpacing(16, after: header)

        for view in questionViews {
            view.setContentHuggingPriority(.required, for: .vertical)
            view.setContentCompressionResistancePriority(.required, for: .vertical)
            contentStack.addArrangedSubview(view)
            view.widthAnchor.constraint(equalTo: contentStack.widthAnchor).isActive = true
        }

        errorLabel.setContentHuggingPriority(.required, for: .vertical)
        errorLabel.setContentCompressionResistancePriority(.required, for: .vertical)
        contentStack.addArrangedSubview(errorLabel)
        let actions = buttonRow()
        actions.setContentHuggingPriority(.required, for: .vertical)
        actions.setContentCompressionResistancePriority(.required, for: .vertical)

        let scrollView = NSScrollView()
        scrollView.translatesAutoresizingMaskIntoConstraints = false
        scrollView.drawsBackground = false
        scrollView.borderType = .noBorder
        scrollView.hasVerticalScroller = false
        scrollView.hasHorizontalScroller = false
        scrollView.autohidesScrollers = true

        let documentView = FlippedView()
        documentView.translatesAutoresizingMaskIntoConstraints = false
        scrollView.documentView = documentView
        documentView.addSubview(contentStack)

        contentView.addSubview(backgroundView)
        backgroundView.addSubview(scrollView)
        backgroundView.addSubview(actions)

        let scrollHeightConstraint = scrollView.heightAnchor.constraint(equalToConstant: 200)
        self.contentScrollHeightConstraint = scrollHeightConstraint
        self.contentScrollView = scrollView
        self.contentStack = contentStack
        self.actionsView = actions

        NSLayoutConstraint.activate([
            backgroundView.topAnchor.constraint(equalTo: contentView.topAnchor),
            backgroundView.leadingAnchor.constraint(equalTo: contentView.leadingAnchor),
            backgroundView.trailingAnchor.constraint(equalTo: contentView.trailingAnchor),
            backgroundView.bottomAnchor.constraint(equalTo: contentView.bottomAnchor),
            scrollView.topAnchor.constraint(equalTo: backgroundView.topAnchor, constant: Self.topInset),
            scrollView.leadingAnchor.constraint(equalTo: backgroundView.leadingAnchor, constant: Self.sideInset),
            scrollView.trailingAnchor.constraint(equalTo: backgroundView.trailingAnchor, constant: -Self.sideInset),
            actions.leadingAnchor.constraint(equalTo: backgroundView.leadingAnchor, constant: Self.sideInset),
            actions.trailingAnchor.constraint(equalTo: backgroundView.trailingAnchor, constant: -Self.sideInset),
            actions.bottomAnchor.constraint(equalTo: backgroundView.bottomAnchor, constant: -Self.bottomInset),
            actions.topAnchor.constraint(greaterThanOrEqualTo: scrollView.bottomAnchor, constant: Self.contentToActionsSpacing),
            scrollHeightConstraint,
            contentStack.topAnchor.constraint(equalTo: documentView.topAnchor),
            contentStack.leadingAnchor.constraint(equalTo: documentView.leadingAnchor),
            contentStack.trailingAnchor.constraint(equalTo: documentView.trailingAnchor),
            contentStack.bottomAnchor.constraint(equalTo: documentView.bottomAnchor),
            contentStack.widthAnchor.constraint(equalTo: scrollView.contentView.widthAnchor),
        ])

        updateWindowSizing()
    }

    private func headerView() -> NSView {
        let stack = NSStackView()
        stack.orientation = .vertical
        stack.alignment = .leading
        stack.spacing = 3
        stack.translatesAutoresizingMaskIntoConstraints = false

        progressLabel.font = .systemFont(ofSize: 15, weight: .semibold)
        progressLabel.textColor = .labelColor
        progressLabel.maximumNumberOfLines = 1
        progressLabel.translatesAutoresizingMaskIntoConstraints = false

        countdownLabel.font = .systemFont(ofSize: 14)
        countdownLabel.textColor = .secondaryLabelColor
        countdownLabel.maximumNumberOfLines = 0
        countdownLabel.setContentHuggingPriority(.defaultLow, for: .horizontal)
        countdownLabel.setContentCompressionResistancePriority(.defaultHigh, for: .horizontal)
        countdownLabel.translatesAutoresizingMaskIntoConstraints = false

        stopTimerButton.target = self
        stopTimerButton.action = #selector(turnOffAutoSelect(_:))
        stopTimerButton.controlSize = .small
        stopTimerButton.bezelStyle = .rounded
        stopTimerButton.setContentHuggingPriority(.required, for: .horizontal)
        stopTimerButton.setContentCompressionResistancePriority(.required, for: .horizontal)
        stopTimerButton.translatesAutoresizingMaskIntoConstraints = false

        let countdownRow = NSStackView()
        countdownRow.orientation = .horizontal
        countdownRow.alignment = .firstBaseline
        countdownRow.spacing = 10
        countdownRow.translatesAutoresizingMaskIntoConstraints = false

        let countdownSpacer = NSView()
        countdownSpacer.translatesAutoresizingMaskIntoConstraints = false
        countdownSpacer.setContentHuggingPriority(.defaultLow, for: .horizontal)
        countdownSpacer.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)

        let trailingInset = NSView()
        trailingInset.translatesAutoresizingMaskIntoConstraints = false
        trailingInset.setContentHuggingPriority(.required, for: .horizontal)
        trailingInset.setContentCompressionResistancePriority(.required, for: .horizontal)
        trailingInset.widthAnchor.constraint(equalToConstant: 6).isActive = true

        countdownRow.addArrangedSubview(countdownLabel)
        countdownRow.addArrangedSubview(countdownSpacer)
        countdownRow.addArrangedSubview(stopTimerButton)
        countdownRow.addArrangedSubview(trailingInset)

        stack.addArrangedSubview(progressLabel)
        stack.addArrangedSubview(countdownRow)
        progressLabel.widthAnchor.constraint(equalTo: stack.widthAnchor).isActive = true
        countdownRow.widthAnchor.constraint(equalTo: stack.widthAnchor).isActive = true

        return stack
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
        spacer.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)

        let cancelButton = NSButton(title: "Cancel", target: self, action: #selector(cancel))
        cancelButton.keyEquivalent = "\u{1b}"
        submitButton.target = self
        submitButton.action = #selector(submit)
        submitButton.keyEquivalent = "\r"
        updateSubmitButtonState()

        buttons.addArrangedSubview(spacer)
        buttons.addArrangedSubview(cancelButton)
        buttons.addArrangedSubview(submitButton)

        return buttons
    }

    private func updateSubmitButtonState() {
        submitButton.isEnabled = questionViews.allSatisfy { $0.selectedAnswer != nil }
    }

    private func focusStopTimerButtonIfNeeded() {
        guard
            let window,
            !isCountdownPermanentlyStopped
        else {
            return
        }

        window.makeFirstResponder(stopTimerButton)
    }

    private func setValidationMessage(_ message: String?) {
        let value = message ?? ""
        errorLabel.stringValue = value
        errorLabel.isHidden = value.isEmpty
        updateWindowSizing()
    }

    private func updateWindowSizing() {
        guard
            let window,
            let contentView = window.contentView,
            let contentStack,
            let contentScrollView,
            let actionsView,
            let contentScrollHeightConstraint
        else {
            return
        }

        contentView.layoutSubtreeIfNeeded()
        let contentHeight = contentStack.fittingSize.height
        let actionsHeight = actionsView.fittingSize.height
        let chromeHeight = window.frameRect(forContentRect: NSRect(x: 0, y: 0, width: Self.windowWidth, height: 0)).height
        let maxContentRectHeight = max(0, Self.maxWindowHeight - chromeHeight)
        let fixedHeight = Self.topInset + Self.contentToActionsSpacing + actionsHeight + Self.bottomInset
        let availableScrollableHeight = max(0, maxContentRectHeight - fixedHeight)
        let targetScrollHeight = min(contentHeight, availableScrollableHeight)
        let targetContentRectHeight = fixedHeight + targetScrollHeight

        contentScrollHeightConstraint.constant = targetScrollHeight
        contentScrollView.hasVerticalScroller = contentHeight > availableScrollableHeight
        contentScrollView.verticalScrollElasticity = contentHeight > availableScrollableHeight ? .automatic : .none

        contentView.layoutSubtreeIfNeeded()
        window.setContentSize(NSSize(width: Self.windowWidth, height: targetContentRectHeight))
    }

    private func activateQuestion(at index: Int, startCountdown: Bool) {
        guard questionViews.indices.contains(index) else {
            return
        }

        activeQuestionIndex = index
        updateQuestionStates()
        updateWindowSizing()
        scrollQuestionIntoView(index)

        if startCountdown {
            startQuestionTimer()
        } else {
            invalidateQuestionTimer()
            remainingQuestionSeconds = Self.questionTimeoutSeconds
            updateStatusLabel()
        }
    }

    private func updateQuestionStates() {
        for (index, view) in questionViews.enumerated() {
            if index < activeQuestionIndex || view.isAnswered {
                view.setQuestionState(.answered, isInteractive: false)
            } else if index == activeQuestionIndex {
                view.setQuestionState(.active, isInteractive: true)
            } else {
                view.setQuestionState(.pending, isInteractive: false)
            }
        }
    }

    private func startQuestionTimer() {
        guard !isCountdownPermanentlyStopped else {
            invalidateQuestionTimer()
            remainingQuestionSeconds = Self.questionTimeoutSeconds
            updateStatusLabel()
            return
        }

        invalidateQuestionTimer()
        remainingQuestionSeconds = Self.questionTimeoutSeconds

        let timer = Timer.scheduledTimer(withTimeInterval: 1, repeats: true) { [weak self] _ in
            self?.tickQuestionTimer()
        }
        RunLoop.main.add(timer, forMode: .common)
        RunLoop.main.add(timer, forMode: Self.modalPanelRunLoopMode)
        questionTimer = timer
        updateStatusLabel()
    }

    private func tickQuestionTimer() {
        guard remainingQuestionSeconds > 0 else {
            return
        }

        remainingQuestionSeconds -= 1
        if remainingQuestionSeconds == 0 {
            invalidateQuestionTimer()
            autoSelectActiveQuestionIfNeeded()
            return
        }

        updateStatusLabel()
    }

    private func autoSelectActiveQuestionIfNeeded() {
        guard questionViews.indices.contains(activeQuestionIndex) else {
            return
        }

        let activeView = questionViews[activeQuestionIndex]
        guard !activeView.isAnswered else {
            return
        }

        guard activeView.autoSelectFirstOption() else {
            updateStatusLabel()
            return
        }

        handleAnswerCommitted(at: activeQuestionIndex)
    }

    private func stopCountdownPermanently() {
        isCountdownPermanentlyStopped = true
        invalidateQuestionTimer()
        updateStatusLabel()
    }

    @objc
    private func turnOffAutoSelect(_ sender: Any?) {
        window?.makeFirstResponder(nil)
        stopCountdownPermanently()
        _ = sender
    }

    private func invalidateQuestionTimer() {
        questionTimer?.invalidate()
        questionTimer = nil
    }

    private func updateStatusLabel() {
        guard questionViews.indices.contains(activeQuestionIndex) else {
            progressLabel.stringValue = ""
            countdownLabel.stringValue = ""
            return
        }

        let questionNumber = activeQuestionIndex + 1
        let totalQuestions = questionViews.count
        progressLabel.stringValue = "Question \(questionNumber) of \(totalQuestions)"
        stopTimerButton.isHidden = isCountdownPermanentlyStopped
        stopTimerButton.isEnabled = !isCountdownPermanentlyStopped
        if questionTimer != nil {
            countdownLabel.stringValue =
                "Auto-selects the first option in \(remainingQuestionSeconds) seconds unless you interact."
            return
        }

        if isCountdownPermanentlyStopped {
            countdownLabel.stringValue =
                "Timer disabled for the rest of this dialog. Confirm this answer to continue."
            return
        }

        countdownLabel.stringValue =
            "Timer stopped. Confirm this answer to continue."
    }

    private func nextUnansweredQuestionIndex(startingAt startIndex: Int) -> Int? {
        guard startIndex < questionViews.count else {
            return nil
        }

        return (startIndex..<questionViews.count).first(where: { !questionViews[$0].isAnswered })
    }

    private func scrollQuestionIntoView(_ index: Int) {
        guard questionViews.indices.contains(index) else {
            return
        }

        questionViews[index].scrollToVisible(questionViews[index].bounds)
    }

    private func presentedFrame(for window: NSWindow) -> NSRect {
        let visibleFrame = activeVisibleFrame(for: window)
        let size = window.frame.size
        let originX = visibleFrame.midX - (size.width / 2)
        let originY = visibleFrame.minY + 16
        return NSRect(origin: NSPoint(x: originX, y: originY), size: size)
    }

    private func hiddenStartFrame(for targetFrame: NSRect, window: NSWindow) -> NSRect {
        let visibleFrame = activeVisibleFrame(for: window)
        var frame = targetFrame
        frame.origin.y = visibleFrame.minY - frame.height
        return frame
    }

    private func activeVisibleFrame(for window: NSWindow?) -> NSRect {
        let mouseLocation = NSEvent.mouseLocation
        if let screen = NSScreen.screens.first(where: { $0.frame.contains(mouseLocation) }) {
            return screen.visibleFrame
        }
        if let screen = window?.screen {
            return screen.visibleFrame
        }
        if let screen = NSScreen.main {
            return screen.visibleFrame
        }
        let fallbackHeight = max(window?.frame.height ?? 420, 420) + 32
        let fallbackWidth = max(window?.frame.width ?? Self.windowWidth, Self.windowWidth)
        return NSRect(x: 0, y: 0, width: fallbackWidth, height: fallbackHeight)
    }

    private static func assignShortcuts(for request: PopupInputRequest) -> [[Character?]] {
        var cursor = optionShortcutKeys.startIndex
        return request.questions.map { question in
            question.options.map { _ in
                guard cursor < optionShortcutKeys.endIndex else {
                    return nil
                }

                defer { cursor = optionShortcutKeys.index(after: cursor) }
                return optionShortcutKeys[cursor]
            }
        }
    }

    private static func isOtherLabel(_ label: String) -> Bool {
        label.trimmingCharacters(in: .whitespacesAndNewlines)
            .caseInsensitiveCompare(otherLabel) == .orderedSame
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

private func showPopup(for request: PopupInputRequest) throws -> PopupInputResponse {
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
    let response = try showPopup(for: request)
    try writeResponse(response)
} catch {
    fputs("\(error.localizedDescription)\n", stderr)
    exit(1)
}
