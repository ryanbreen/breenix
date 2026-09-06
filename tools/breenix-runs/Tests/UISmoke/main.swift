import AppKit
import ApplicationServices
import Foundation

struct SmokeError: Error, CustomStringConvertible {
    let description: String
}
func require(_ condition: Bool, _ message: String) throws {
    if !condition { throw SmokeError(description: message) }
}
func pollRunLoop() { RunLoop.current.run(until: Date().addingTimeInterval(0.1)) }
func attribute(_ element: AXUIElement, _ name: String) -> CFTypeRef? {
    var value: CFTypeRef?
    let result = AXUIElementCopyAttributeValue(element, name as CFString, &value)
    return result == .success ? value : nil
}
func nodes(_ element: AXUIElement, depth: Int = 0) -> [AXUIElement] {
    if depth > 30 { return [] }
    let children = attribute(element, kAXChildrenAttribute) as? [AXUIElement] ?? []
    return [element] + children.flatMap { nodes($0, depth: depth + 1) }
}
func text(_ element: AXUIElement) -> String {
    [kAXTitleAttribute, kAXValueAttribute, kAXDescriptionAttribute].compactMap {
        attribute(element, $0) as? String
    }.joined(separator: " | ")
}
func alive(_ app: NSRunningApplication) throws {
    try require(!app.isTerminated && kill(app.processIdentifier, 0) == 0,
                "App PID \(app.processIdentifier) exited/crashed")
}
func waitFor(_ app: NSRunningApplication, _ label: String,
             _ test: () throws -> Bool) throws {
    let deadline = Date().addingTimeInterval(20)
    repeat {
        try alive(app)
        if try test() { return }
        pollRunLoop()
    } while Date() < deadline
    throw SmokeError(description: "Timed out: " + label)
}
func smoke(_ app: NSRunningApplication) throws {
    let ax = AXUIElementCreateApplication(app.processIdentifier)
    AXUIElementSetMessagingTimeout(ax, 2)
    var windows: CFTypeRef?
    let result = AXUIElementCopyAttributeValue(ax, kAXWindowsAttribute as CFString, &windows)
    print("PID=\(app.processIdentifier) AXIsProcessTrusted=\(AXIsProcessTrusted())")
    print("AXWindows error=\(result.rawValue) count=\((windows as? [AXUIElement])?.count ?? -1)")
    try require(AXIsProcessTrusted() && result != .apiDisabled,
                "Accessibility permission denied: AXIsProcessTrusted=false; AXWindows error=\(result.rawValue)")
    var window: AXUIElement?
    try waitFor(app, "AXWindows") {
        var value: CFTypeRef?
        let status = AXUIElementCopyAttributeValue(ax, kAXWindowsAttribute as CFString, &value)
        if status == .apiDisabled { throw SmokeError(description: "AXWindows error=-25211 (accessibility denied)") }
        window = (value as? [AXUIElement])?.first
        return status == .success && window != nil
    }
    let root = window!
    var rows: [AXUIElement] = []
    try waitFor(app, "sidebar rows") {
        guard let sidebar = nodes(root).first(where: {
            attribute($0, kAXIdentifierAttribute) as? String == "run-sidebar"
        }) else { return false }
        rows = nodes(sidebar).filter { attribute($0, kAXRoleAttribute) as? String == kAXRowRole }
        return !rows.isEmpty
    }
    print("Sidebar rows=\(rows.count); first=\(nodes(rows[0]).map(text).joined(separator: " | "))")
    let selected = AXUIElementSetAttributeValue(rows[0], kAXSelectedAttribute as CFString, kCFBooleanTrue)
    let pressed = selected == .success ? AXError.success : AXUIElementPerformAction(rows[0], kAXPressAction as CFString)
    try require(pressed == .success, "Row selection failed: AXSelected=\(selected.rawValue), AXPress=\(pressed.rawValue)")
    try alive(app)
    try waitFor(app, "row selected") {
        (attribute(rows[0], kAXSelectedAttribute) as? Bool) == true
    }
    for (tab, expected) in [
        ("Subsystems", "ARM64 kernel starting"),
        ("Messages", "Breenix ARM64 Kernel Starting"),
        ("Traces", "qemu_cpu_s=17.85")
    ] {
        var button: AXUIElement?
        try waitFor(app, tab + " tab") {
            button = nodes(root).first {
                let role = attribute($0, kAXRoleAttribute) as? String
                return (role == kAXRadioButtonRole || role == kAXButtonRole) && text($0).contains(tab)
            }
            return button != nil
        }
        let status = AXUIElementPerformAction(button!, kAXPressAction as CFString)
        try require(status == .success, "\(tab) AXPress error=\(status.rawValue)")
        try alive(app)
        var observed = ""
        try waitFor(app, tab + " content " + expected) {
            guard let pane = nodes(root).first(where: {
                attribute($0, kAXIdentifierAttribute) as? String == "pane-" + tab.lowercased()
            }) else { return false }
            observed = nodes(pane).map(text).first { $0.contains(expected) } ?? ""
            return !observed.isEmpty
        }
        print("\(tab): saw \(observed)")
        try alive(app)
    }
    // Catch asynchronous detail-load failures after the final interaction too.
    let deadline = Date().addingTimeInterval(2)
    while Date() < deadline { try alive(app); pollRunLoop() }
    print("Same PID \(app.processIdentifier) alive after selection and pane assertions")
}

let configuration = NSWorkspace.OpenConfiguration()
configuration.createsNewApplicationInstance = true
configuration.environment = ["BREENIX_RUNS_STORE": CommandLine.arguments[2]]
var launched: NSRunningApplication?
var launchError: Error?
var completed = false
NSWorkspace.shared.openApplication(at: URL(fileURLWithPath: CommandLine.arguments[1]),
                                  configuration: configuration) { app, error in
    launched = app
    launchError = error
    completed = true
}
let deadline = Date().addingTimeInterval(20)
while !completed && Date() < deadline { pollRunLoop() }
var exitStatus: Int32 = 0
if let app = launched {
    print("Launched packaged app PID=\(app.processIdentifier)")
    do { try smoke(app) } catch {
        print("UI SMOKE FAILED: \(error)")
        exitStatus = 1
    }
    if !app.isTerminated { _ = app.terminate() }
    let quitDeadline = Date().addingTimeInterval(10)
    while !app.isTerminated && Date() < quitDeadline { pollRunLoop() }
    if !app.isTerminated {
        print("UI SMOKE FAILED: owned PID did not quit")
        _ = app.forceTerminate()
        exitStatus = 1
    } else {
        print("Owned PID=\(app.processIdentifier) exited")
    }
} else {
    print("UI SMOKE FAILED: application launch: \(String(describing: launchError)) completed=\(completed)")
    exitStatus = 1
}
if exitStatus == 0 { print("UI SMOKE PASSED: real AX selection and pane content asserted") }
exit(exitStatus)
