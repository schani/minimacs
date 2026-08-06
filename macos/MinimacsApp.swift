import AppKit
import CoreText
import Darwin

private let keyChar: UInt32 = 0
private let keyEnter: UInt32 = 1
private let keyTab: UInt32 = 2
private let keyBackspace: UInt32 = 3
private let keyDelete: UInt32 = 4
private let keyEscape: UInt32 = 5
private let keyLeft: UInt32 = 6
private let keyRight: UInt32 = 7
private let keyArrowUp: UInt32 = 8
private let keyArrowDown: UInt32 = 9
private let keyHome: UInt32 = 10
private let keyEnd: UInt32 = 11
private let keyPageUp: UInt32 = 12
private let keyPageDown: UInt32 = 13

private let modControl: UInt8 = 1 << 0
private let modAlt: UInt8 = 1 << 1
private let modShift: UInt8 = 1 << 2

private let mouseClick: UInt32 = 0
private let mouseScrollUp: UInt32 = 1
private let mouseScrollDown: UInt32 = 2

private let commandSave: UInt32 = 0
private let commandUndo: UInt32 = 1
private let commandRedo: UInt32 = 2
private let commandCancel: UInt32 = 3

private let cellBold: UInt8 = 1 << 0
private let cellItalic: UInt8 = 1 << 1
private let cellUnderlined: UInt8 = 1 << 2
private let cellReversed: UInt8 = 1 << 3

private struct CellStyle: Equatable {
  let foreground: NSColor
  let background: NSColor
  let modifiers: UInt8

  static func == (lhs: CellStyle, rhs: CellStyle) -> Bool {
    lhs.foreground.isEqual(rhs.foreground)
      && lhs.background.isEqual(rhs.background)
      && lhs.modifiers == rhs.modifiers
  }
}

final class EditorView: NSView, NSTextInputClient {
  private let handle: UnsafeMutableRawPointer
  private let baseFont = NSFont.monospacedSystemFont(ofSize: 14, weight: .regular)
  private let inset: CGFloat = 6
  private(set) var cellWidth: CGFloat = 8
  private(set) var cellHeight: CGFloat = 18
  private var gridWidth: UInt16 = 0
  private var gridHeight: UInt16 = 0
  private var markedText = NSAttributedString(string: "")
  private var syntaxTimer: Timer?

  init() {
    guard let handle = minimacs_native_new(80, 30) else {
      fatalError("Could not initialize the minimacs Rust core")
    }
    self.handle = handle
    super.init(frame: .zero)
    wantsLayer = true
    layer?.backgroundColor = NSColor.textBackgroundColor.cgColor
    let attributes: [NSAttributedString.Key: Any] = [.font: baseFont]
    cellWidth = ceil(("M" as NSString).size(withAttributes: attributes).width)
    cellHeight = ceil(baseFont.ascender - baseFont.descender + baseFont.leading + 2)
  }

  required init?(coder: NSCoder) {
    fatalError("EditorView is programmatic")
  }

  deinit {
    syntaxTimer?.invalidate()
    minimacs_native_free(handle)
  }

  override var acceptsFirstResponder: Bool { true }
  override var isFlipped: Bool { true }

  override func viewDidMoveToWindow() {
    super.viewDidMoveToWindow()
    window?.makeFirstResponder(self)
  }

  override func setFrameSize(_ newSize: NSSize) {
    super.setFrameSize(newSize)
    resizeCoreIfNeeded()
  }

  private func resizeCoreIfNeeded() {
    let columns = UInt16(max(2, min(65_535, floor((bounds.width - 2 * inset) / cellWidth))))
    let rows = UInt16(max(2, min(65_535, floor((bounds.height - 2 * inset) / cellHeight))))
    guard columns != gridWidth || rows != gridHeight else { return }
    gridWidth = columns
    gridHeight = rows
    _ = minimacs_native_resize(handle, columns, rows)
    scheduleSyntaxPollingIfNeeded()
    needsDisplay = true
  }

  private func scheduleSyntaxPollingIfNeeded() {
    guard syntaxTimer == nil, minimacs_native_has_background_work(handle) else { return }
    syntaxTimer = Timer.scheduledTimer(withTimeInterval: 0.1, repeats: true) {
      [weak self] timer in
      guard let self else {
        timer.invalidate()
        return
      }
      if minimacs_native_poll(self.handle) {
        self.needsDisplay = true
      }
      if !minimacs_native_has_background_work(self.handle) {
        timer.invalidate()
        self.syntaxTimer = nil
      }
    }
  }

  override func draw(_ dirtyRect: NSRect) {
    NSColor.textBackgroundColor.setFill()
    dirtyRect.fill()

    let frame = minimacs_native_frame(handle)
    guard let cells = frame.cells, frame.width > 0, frame.height > 0 else { return }

    for row in 0..<Int(frame.height) {
      drawRow(cells: cells, row: row, width: Int(frame.width))
    }

    if frame.cursor_visible != 0 {
      NSColor.controlAccentColor.setFill()
      NSRect(
        x: inset + CGFloat(frame.cursor_x) * cellWidth,
        y: inset + CGFloat(frame.cursor_y) * cellHeight,
        width: 2,
        height: cellHeight
      ).fill()
    }

    if markedText.length > 0 {
      let point = NSPoint(
        x: inset + CGFloat(frame.cursor_x) * cellWidth,
        y: inset + CGFloat(frame.cursor_y) * cellHeight
      )
      markedText.draw(at: point)
    }
  }

  private func drawRow(cells: UnsafePointer<MmCell>, row: Int, width: Int) {
    drawBackgrounds(cells: cells, row: row, width: width)

    // Core Text's natural monospaced advance is fractional, while the grid is
    // deliberately pixel-aligned. Kern common ASCII runs onto the grid; anchor
    // complex and fallback glyph cells individually so neither path can drift
    // away from cursor and mouse geometry.
    var column = 0
    while column < width {
      let cell = cells[row * width + column]
      if let firstByte = printableASCIIByte(cell) {
        let runStyle = style(for: cell)
        var bytes = [firstByte]
        var end = column + 1
        while end < width {
          let next = cells[row * width + end]
          guard style(for: next) == runStyle, let byte = printableASCIIByte(next) else { break }
          bytes.append(byte)
          end += 1
        }
        let text = String(decoding: bytes, as: UTF8.self)
        if !text.trimmingCharacters(in: .whitespaces).isEmpty {
          (text as NSString).draw(
            at: textOrigin(column: column, row: row),
            withAttributes: textAttributes(style: runStyle, alignASCIIToGrid: true)
          )
        }
        column = end
        continue
      }

      if let text = text(for: cell), !text.trimmingCharacters(in: .whitespaces).isEmpty {
        (text as NSString).draw(
          at: textOrigin(column: column, row: row),
          withAttributes: textAttributes(style: style(for: cell))
        )
      }
      column += 1
    }
  }

  private func printableASCIIByte(_ cell: MmCell) -> UInt8? {
    guard let bytes = cell.text, cell.text_len == 1 else { return nil }
    let byte = bytes.pointee
    return (0x20...0x7e).contains(byte) ? byte : nil
  }

  private func text(for cell: MmCell) -> String? {
    guard let bytes = cell.text, cell.text_len > 0 else { return nil }
    let buffer = UnsafeBufferPointer(start: bytes, count: cell.text_len)
    return String(decoding: buffer, as: UTF8.self)
  }

  private func textOrigin(column: Int, row: Int) -> NSPoint {
    NSPoint(
      x: inset + CGFloat(column) * cellWidth,
      y: inset + CGFloat(row) * cellHeight + 1
    )
  }

  private func drawBackgrounds(cells: UnsafePointer<MmCell>, row: Int, width: Int) {
    var column = 0
    while column < width {
      let runStyle = style(for: cells[row * width + column])
      var end = column + 1
      while end < width && style(for: cells[row * width + end]) == runStyle {
        end += 1
      }
      if !runStyle.background.isEqual(NSColor.textBackgroundColor) {
        runStyle.background.setFill()
        NSRect(
          x: inset + CGFloat(column) * cellWidth,
          y: inset + CGFloat(row) * cellHeight,
          width: CGFloat(end - column) * cellWidth,
          height: cellHeight
        ).fill()
      }
      column = end
    }
  }

  private func style(for cell: MmCell) -> CellStyle {
    var foreground = color(cell.foreground, fallback: .textColor)
    var background = color(cell.background, fallback: .textBackgroundColor)
    if cell.modifiers & cellReversed != 0 {
      swap(&foreground, &background)
    }
    return CellStyle(
      foreground: foreground,
      background: background,
      modifiers: cell.modifiers
    )
  }

  private func color(_ color: MmColor, fallback: NSColor) -> NSColor {
    guard color.valid != 0 else { return fallback }
    return NSColor(
      calibratedRed: CGFloat(color.red) / 255,
      green: CGFloat(color.green) / 255,
      blue: CGFloat(color.blue) / 255,
      alpha: 1
    )
  }

  private func textAttributes(
    style: CellStyle, alignASCIIToGrid: Bool = false
  ) -> [NSAttributedString.Key: Any] {
    var font = baseFont
    if style.modifiers & cellBold != 0 {
      font = NSFontManager.shared.convert(font, toHaveTrait: .boldFontMask)
    }
    if style.modifiers & cellItalic != 0 {
      font = NSFontManager.shared.convert(font, toHaveTrait: .italicFontMask)
    }
    var attributes: [NSAttributedString.Key: Any] = [
      .font: font,
      .foregroundColor: style.foreground,
    ]
    if style.modifiers & cellUnderlined != 0 {
      attributes[.underlineStyle] = NSUnderlineStyle.single.rawValue
    }
    if alignASCIIToGrid {
      let naturalAdvance = ("M" as NSString).size(withAttributes: [.font: font]).width
      attributes[.kern] = cellWidth - naturalAdvance
    }
    return attributes
  }

  private func mutate(_ body: () -> Bool) {
    if body() {
      markedText = NSAttributedString(string: "")
      scheduleSyntaxPollingIfNeeded()
      needsDisplay = true
      if minimacs_native_should_quit(handle) {
        NSApp.terminate(nil)
      }
    }
  }

  override func keyDown(with event: NSEvent) {
    let special: [UInt16: UInt32] = [
      36: keyEnter,
      48: keyTab,
      51: keyBackspace,
      53: keyEscape,
      115: keyHome,
      116: keyPageUp,
      117: keyDelete,
      119: keyEnd,
      121: keyPageDown,
      123: keyLeft,
      124: keyRight,
      125: keyArrowDown,
      126: keyArrowUp,
    ]
    if let code = special[event.keyCode] {
      mutate { minimacs_native_key(handle, code, 0, modifierBits(event)) }
      return
    }

    let flags = event.modifierFlags.intersection([.control, .option, .shift, .command])
    if flags.contains(.control) || flags.contains(.option) {
      let source = event.charactersIgnoringModifiers ?? event.characters ?? ""
      if let scalar = source.unicodeScalars.first {
        mutate {
          minimacs_native_key(handle, keyChar, scalar.value, modifierBits(event))
        }
      }
      return
    }

    if flags.contains(.command) {
      super.keyDown(with: event)
      return
    }
    interpretKeyEvents([event])
  }

  private func modifierBits(_ event: NSEvent) -> UInt8 {
    var bits: UInt8 = 0
    if event.modifierFlags.contains(.control) { bits |= modControl }
    if event.modifierFlags.contains(.option) { bits |= modAlt }
    if event.modifierFlags.contains(.shift) { bits |= modShift }
    return bits
  }

  override func mouseDown(with event: NSEvent) {
    let point = convert(event.locationInWindow, from: nil)
    let column = UInt16(max(0, min(65_535, floor((point.x - inset) / cellWidth))))
    let row = UInt16(max(0, min(65_535, floor((point.y - inset) / cellHeight))))
    mutate { minimacs_native_mouse(handle, mouseClick, column, row) }
  }

  override func scrollWheel(with event: NSEvent) {
    guard event.scrollingDeltaY != 0 else { return }
    let point = convert(event.locationInWindow, from: nil)
    let column = UInt16(max(0, min(65_535, floor((point.x - inset) / cellWidth))))
    let row = UInt16(max(0, min(65_535, floor((point.y - inset) / cellHeight))))
    let kind = event.scrollingDeltaY > 0 ? mouseScrollUp : mouseScrollDown
    mutate { minimacs_native_mouse(handle, kind, column, row) }
  }

  func open(path: String) -> Bool {
    path.withCString { pointer in
      let changed = minimacs_native_open_file(handle, pointer)
      if changed {
        scheduleSyntaxPollingIfNeeded()
        needsDisplay = true
      }
      return changed
    }
  }

  func whenRenderingIsIdle(_ completion: @escaping () -> Void) {
    needsDisplay = true
    displayIfNeeded()
    guard minimacs_native_has_background_work(handle) else {
      completion()
      return
    }
    scheduleSyntaxPollingIfNeeded()
    DispatchQueue.main.asyncAfter(deadline: .now() + 0.05) { [weak self] in
      self?.whenRenderingIsIdle(completion)
    }
  }

  func save() {
    mutate { minimacs_native_command(handle, commandSave) }
  }

  func undo() {
    mutate { minimacs_native_command(handle, commandUndo) }
  }

  func redo() {
    mutate { minimacs_native_command(handle, commandRedo) }
  }

  func cancel() {
    mutate { minimacs_native_command(handle, commandCancel) }
  }

  func pasteFromPasteboard() {
    guard let text = NSPasteboard.general.string(forType: .string) else { return }
    text.withCString { pointer in
      mutate { minimacs_native_insert_utf8(handle, pointer) }
    }
  }

  func requestQuit() -> Bool {
    if minimacs_native_should_quit(handle) { return true }
    _ = minimacs_native_key(
      handle, keyChar, Character("x").asciiValue.map(UInt32.init) ?? 120, modControl)
    _ = minimacs_native_key(
      handle, keyChar, Character("c").asciiValue.map(UInt32.init) ?? 99, modControl)
    needsDisplay = true
    return minimacs_native_should_quit(handle)
  }

  // MARK: NSTextInputClient

  func insertText(_ string: Any, replacementRange: NSRange) {
    let value: String
    if let attributed = string as? NSAttributedString {
      value = attributed.string
    } else {
      value = string as? String ?? ""
    }
    guard !value.isEmpty else { return }
    if value.unicodeScalars.count == 1, let scalar = value.unicodeScalars.first {
      mutate { minimacs_native_key(handle, keyChar, scalar.value, 0) }
    } else {
      value.withCString { pointer in
        mutate { minimacs_native_insert_utf8(handle, pointer) }
      }
    }
  }

  func setMarkedText(_ string: Any, selectedRange: NSRange, replacementRange: NSRange) {
    if let attributed = string as? NSAttributedString {
      markedText = attributed
    } else {
      markedText = NSAttributedString(
        string: string as? String ?? "",
        attributes: [.font: baseFont, .foregroundColor: NSColor.secondaryLabelColor]
      )
    }
    needsDisplay = true
  }

  func unmarkText() {
    markedText = NSAttributedString(string: "")
    needsDisplay = true
  }

  func selectedRange() -> NSRange { NSRange(location: NSNotFound, length: 0) }
  func markedRange() -> NSRange {
    markedText.length == 0
      ? NSRange(location: NSNotFound, length: 0)
      : NSRange(location: 0, length: markedText.length)
  }
  func hasMarkedText() -> Bool { markedText.length > 0 }
  func attributedSubstring(forProposedRange range: NSRange, actualRange: NSRangePointer?)
    -> NSAttributedString?
  { nil }
  func validAttributesForMarkedText() -> [NSAttributedString.Key] { [.font, .foregroundColor] }

  func firstRect(forCharacterRange range: NSRange, actualRange: NSRangePointer?) -> NSRect {
    let frame = minimacs_native_frame(handle)
    let local = NSRect(
      x: inset + CGFloat(frame.cursor_x) * cellWidth,
      y: inset + CGFloat(frame.cursor_y) * cellHeight,
      width: cellWidth,
      height: cellHeight
    )
    guard let window else { return local }
    return window.convertToScreen(convert(local, to: nil))
  }

  func characterIndex(for point: NSPoint) -> Int { NSNotFound }

  override func doCommand(by selector: Selector) {
    switch selector {
    case #selector(NSResponder.deleteBackward(_:)):
      mutate { minimacs_native_key(handle, keyBackspace, 0, 0) }
    case #selector(NSResponder.insertNewline(_:)):
      mutate { minimacs_native_key(handle, keyEnter, 0, 0) }
    case #selector(NSResponder.cancelOperation(_:)):
      cancel()
    default:
      break
    }
  }
}

final class AppDelegate: NSObject, NSApplicationDelegate, NSWindowDelegate {
  private var window: NSWindow!
  private var editorView: EditorView!
  private var pendingFiles: [String] = []
  private var uiReadyPath: String?
  private var uiReadySignalSource: DispatchSourceSignal?

  func applicationDidFinishLaunching(_ notification: Notification) {
    NSApp.setActivationPolicy(.regular)
    configureMenus()

    editorView = EditorView()
    window = NSWindow(
      contentRect: NSRect(x: 0, y: 0, width: 960, height: 640),
      styleMask: [.titled, .closable, .miniaturizable, .resizable],
      backing: .buffered,
      defer: false
    )
    window.title = "minimacs"
    window.delegate = self
    window.contentView = editorView
    window.center()
    window.makeKeyAndOrderFront(nil)
    NSApp.activate(ignoringOtherApps: true)

    let commandLineFiles = CommandLine.arguments.dropFirst()
      .filter { !$0.hasPrefix("-") }
      .map { URL(fileURLWithPath: $0).standardizedFileURL.path }
    let startupFiles = pendingFiles + commandLineFiles
    pendingFiles.removeAll()
    // Yield once so AppKit can commit the empty first window before any file
    // I/O. Syntax loader and parse work is separately background-only.
    DispatchQueue.main.async { [weak self] in
      guard let self else { return }
      var openedAll = true
      for path in startupFiles {
        if !self.editorView.open(path: path) { openedAll = false }
      }
      if let readyPath = ProcessInfo.processInfo.environment["MINIMACS_UI_READY_FILE"] {
        if openedAll {
          self.configureUIReadySignal(path: readyPath)
          self.writeUIReadyWhenIdle()
        } else {
          try? Data().write(
            to: URL(fileURLWithPath: readyPath + ".failed"), options: .atomic)
        }
      }
    }
  }

  private func configureUIReadySignal(path: String) {
    uiReadyPath = path
    signal(SIGUSR1, SIG_IGN)
    let source = DispatchSource.makeSignalSource(signal: SIGUSR1, queue: .main)
    source.setEventHandler { [weak self] in self?.writeUIReadyWhenIdle() }
    source.resume()
    uiReadySignalSource = source
  }

  private func writeUIReadyWhenIdle() {
    guard let path = uiReadyPath else { return }
    editorView.whenRenderingIsIdle {
      try? Data().write(to: URL(fileURLWithPath: path), options: .atomic)
    }
  }

  func application(_ sender: NSApplication, openFiles filenames: [String]) {
    if editorView == nil {
      pendingFiles.append(contentsOf: filenames)
    } else {
      for path in filenames { _ = editorView.open(path: path) }
    }
    sender.reply(toOpenOrPrint: .success)
  }

  func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { true }

  func applicationShouldTerminate(_ sender: NSApplication) -> NSApplication.TerminateReply {
    editorView.requestQuit() ? .terminateNow : .terminateCancel
  }

  func windowShouldClose(_ sender: NSWindow) -> Bool {
    editorView.requestQuit()
  }

  @objc private func openDocument(_ sender: Any?) {
    let panel = NSOpenPanel()
    panel.allowsMultipleSelection = true
    panel.canChooseDirectories = false
    panel.beginSheetModal(for: window) { [weak self] response in
      guard response == .OK, let self else { return }
      for url in panel.urls { _ = self.editorView.open(path: url.path) }
    }
  }

  @objc private func saveDocument(_ sender: Any?) { editorView.save() }
  @objc private func undo(_ sender: Any?) { editorView.undo() }
  @objc private func redo(_ sender: Any?) { editorView.redo() }
  @objc private func paste(_ sender: Any?) { editorView.pasteFromPasteboard() }
  @objc private func cancel(_ sender: Any?) { editorView.cancel() }

  private func configureMenus() {
    let menu = NSMenu()
    NSApp.mainMenu = menu

    let appItem = NSMenuItem()
    menu.addItem(appItem)
    let appMenu = NSMenu()
    appItem.submenu = appMenu
    appMenu.addItem(
      withTitle: "About minimacs",
      action: #selector(NSApplication.orderFrontStandardAboutPanel(_:)), keyEquivalent: "")
    appMenu.addItem(.separator())
    appMenu.addItem(
      withTitle: "Quit minimacs", action: #selector(NSApplication.terminate(_:)), keyEquivalent: "q"
    )

    let fileItem = NSMenuItem()
    menu.addItem(fileItem)
    let fileMenu = NSMenu(title: "File")
    fileItem.submenu = fileMenu
    let open = fileMenu.addItem(
      withTitle: "Open…", action: #selector(openDocument(_:)), keyEquivalent: "o")
    open.target = self
    let save = fileMenu.addItem(
      withTitle: "Save", action: #selector(saveDocument(_:)), keyEquivalent: "s")
    save.target = self

    let editItem = NSMenuItem()
    menu.addItem(editItem)
    let editMenu = NSMenu(title: "Edit")
    editItem.submenu = editMenu
    let undo = editMenu.addItem(withTitle: "Undo", action: #selector(undo(_:)), keyEquivalent: "z")
    undo.target = self
    let redo = editMenu.addItem(withTitle: "Redo", action: #selector(redo(_:)), keyEquivalent: "Z")
    redo.target = self
    redo.keyEquivalentModifierMask = [.command, .shift]
    editMenu.addItem(.separator())
    let paste = editMenu.addItem(
      withTitle: "Paste", action: #selector(paste(_:)), keyEquivalent: "v")
    paste.target = self
    let cancel = editMenu.addItem(
      withTitle: "Cancel", action: #selector(cancel(_:)), keyEquivalent: ".")
    cancel.target = self
  }
}

let application = NSApplication.shared
let delegate = AppDelegate()
application.delegate = delegate
application.run()
