// The iOS host (ADR-0070): the daemon linked in, platform facts written beside it, and the tail of
// its log on screen. Everything else — enrolment, runs, policy — is the ordinary CLI pointed at the
// daemon's socket, as on Android (ADR-0066).

import Network
import UIKit

/// Where everything lives. The state directory is the app's own; the socket is not, on the
/// Simulator, because the container path is far past `SUN_LEN` and the Mac's CLI has to reach it.
enum Paths {
    static let library = FileManager.default.urls(for: .libraryDirectory, in: .userDomainMask)[0]
    static let stateDir = library.appendingPathComponent("s")
    static let config = library.appendingPathComponent("node.toml")
    static let daemonLog = stateDir.appendingPathComponent("daemon.log")
    static let hostFacts = stateDir.appendingPathComponent("host-facts.json")

    static var socket: String {
        #if targetEnvironment(simulator)
            return "/tmp/offload-ios.sock"
        #else
            return stateDir.appendingPathComponent("offloadd.sock").path
        #endif
    }

    /// A starting config, written once; after that it is the owner's to edit — except for the two
    /// paths inside the app's container, which `repoint` keeps current.
    static func writeDefaultConfig() {
        if FileManager.default.fileExists(atPath: config.path) {
            repoint()
            return
        }
        try? FileManager.default.createDirectory(at: stateDir, withIntermediateDirectories: true)
        let form = UIDevice.current.userInterfaceIdiom == .pad ? "ipad" : "iphone"
        #if targetEnvironment(simulator)
            // The Simulator shares the Mac's network stack, where `offloadd` may already hold 7433.
            let listen = "[::]:7435"
        #else
            let listen = "[::]:7433"
        #endif
        let text = """
            name = "ios-\(form)"
            state_dir = "\(stateDir.path)"
            socket = "\(socket)"

            [cluster]
            listen = "\(listen)"
            mdns = true

            """
        try? text.write(to: config, atomically: true, encoding: .utf8)
    }

    /// Point `state_dir` (and `socket`, on a device) at the container this launch was given.
    ///
    /// iOS moves an app's container when it is reinstalled or updated, and a config written with
    /// the old absolute path sends the daemon to a directory that is no longer the app's. On the
    /// Simulator it created that directory afresh and **minted a new node identity** there: the
    /// device silently became a different node, outside its fleet (session ninety-two). On a
    /// device the old path is outside the sandbox and the daemon would not start at all.
    static func repoint() {
        guard var text = try? String(contentsOf: config, encoding: .utf8) else { return }
        let original = text
        text = replacing(key: "state_dir", with: stateDir.path, in: text)
        #if !targetEnvironment(simulator)
            text = replacing(key: "socket", with: socket, in: text)
        #endif
        if text != original {
            try? text.write(to: config, atomically: true, encoding: .utf8)
        }
    }

    /// Replace the value of a top-level `key = "…"` line, leaving every other line as written.
    private static func replacing(key: String, with value: String, in text: String) -> String {
        var inTable = false
        return text.split(separator: "\n", omittingEmptySubsequences: false).map { line in
            let trimmed = line.trimmingCharacters(in: .whitespaces)
            if trimmed.hasPrefix("[") { inTable = true }
            let isKey = trimmed.hasPrefix(key)
                && trimmed.dropFirst(key.count).trimmingCharacters(in: .whitespaces).hasPrefix("=")
            return !inTable && isKey ? "\(key) = \"\(value)\"" : String(line)
        }.joined(separator: "\n")
    }
}

/// `host-facts.json` every 15 s, in the shape the daemon already reads (ADR-0066 §3, ADR-0068).
final class Facts {
    private let monitor = NWPathMonitor()
    private var metered: Bool?
    private var timer: Timer?

    func start() {
        UIDevice.current.isBatteryMonitoringEnabled = true
        monitor.pathUpdateHandler = { [weak self] path in
            self?.metered = path.status == .satisfied ? (path.isExpensive || path.isConstrained) : nil
        }
        monitor.start(queue: .main)
        write()
        timer = Timer.scheduledTimer(withTimeInterval: 15, repeats: true) { [weak self] _ in
            self?.write()
        }
    }

    private func write() {
        let device = UIDevice.current
        // The Simulator has no battery: level -1 and state unknown, which is null, not "flat".
        let percent: Any = device.batteryLevel >= 0 ? Int(device.batteryLevel * 100) : NSNull()
        let charging: Any
        switch device.batteryState {
        case .charging, .full: charging = true
        case .unplugged: charging = false
        default: charging = NSNull()
        }
        // ADR-0068's scale is Android's: 0 none, 1 light, 2 moderate, 3 severe, 4 critical.
        let thermal: Int
        switch ProcessInfo.processInfo.thermalState {
        case .nominal: thermal = 0
        case .fair: thermal = 1
        case .serious: thermal = 3
        case .critical: thermal = 4
        @unknown default: thermal = 0
        }
        let facts: [String: Any] = [
            "battery_percent": percent,
            "charging": charging,
            "metered": metered.map { $0 as Any } ?? NSNull(),
            "thermal": thermal,
            "form": device.userInterfaceIdiom == .pad ? "tablet" : "phone",
        ]
        guard let data = try? JSONSerialization.data(withJSONObject: facts) else { return }
        try? data.write(to: Paths.hostFacts, options: .atomic)
    }
}

final class LogController: UIViewController {
    private let text = UITextView()
    private let status = UILabel()
    private var timer: Timer?

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = .systemBackground
        status.font = .preferredFont(forTextStyle: .headline)
        status.numberOfLines = 0
        text.isEditable = false
        text.font = .monospacedSystemFont(ofSize: 10, weight: .regular)
        for v in [status, text] {
            v.translatesAutoresizingMaskIntoConstraints = false
            view.addSubview(v)
        }
        let g = view.safeAreaLayoutGuide
        NSLayoutConstraint.activate([
            status.topAnchor.constraint(equalTo: g.topAnchor, constant: 12),
            status.leadingAnchor.constraint(equalTo: g.leadingAnchor, constant: 16),
            status.trailingAnchor.constraint(equalTo: g.trailingAnchor, constant: -16),
            text.topAnchor.constraint(equalTo: status.bottomAnchor, constant: 8),
            text.leadingAnchor.constraint(equalTo: g.leadingAnchor, constant: 8),
            text.trailingAnchor.constraint(equalTo: g.trailingAnchor, constant: -8),
            text.bottomAnchor.constraint(equalTo: g.bottomAnchor),
        ])
        refresh()
        timer = Timer.scheduledTimer(withTimeInterval: 2, repeats: true) { [weak self] _ in
            self?.refresh()
        }
    }

    func setStatus(_ s: String) { status.text = s }

    private func refresh() {
        guard let log = try? String(contentsOf: Paths.daemonLog, encoding: .utf8) else { return }
        text.text = log.split(separator: "\n").suffix(60).joined(separator: "\n")
        text.scrollRangeToVisible(NSRange(location: text.text.utf16.count, length: 0))
    }
}

@main
final class AppDelegate: UIResponder, UIApplicationDelegate {
    var window: UIWindow?
    private let facts = Facts()
    private let log = LogController()
    private var background: UIBackgroundTaskIdentifier = .invalid

    func application(
        _ application: UIApplication,
        didFinishLaunchingWithOptions _: [UIApplication.LaunchOptionsKey: Any]?
    ) -> Bool {
        Paths.writeDefaultConfig()
        facts.start()
        window = UIWindow(frame: UIScreen.main.bounds)
        window?.rootViewController = log
        window?.makeKeyAndVisible()
        startDaemon()
        return true
    }

    func applicationWillEnterForeground(_: UIApplication) {
        startDaemon()
    }

    /// ADR-0070 §3: a suspended app is gone, so it leaves politely rather than by probe timeout.
    func applicationDidEnterBackground(_ application: UIApplication) {
        background = application.beginBackgroundTask(withName: "offload-drain") { [weak self] in
            self?.endBackground(application)
        }
        DispatchQueue.global().async { [weak self] in
            offload_stop()
            DispatchQueue.main.async { self?.endBackground(application) }
        }
        log.setStatus("offloadd stopped (in background)")
    }

    private func endBackground(_ application: UIApplication) {
        if background != .invalid {
            application.endBackgroundTask(background)
            background = .invalid
        }
    }

    private func startDaemon() {
        let code = offload_start(Paths.config.path)
        let said: String
        switch code {
        case 0: said = "offloadd running"
        case 1: said = "offloadd already running"
        case 2: said = "offloadd: no config at \(Paths.config.path)"
        default: said = "offloadd: \(Paths.config.path) does not load"
        }
        log.setStatus("\(said)\nsocket \(Paths.socket)")
    }
}
