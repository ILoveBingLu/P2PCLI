import XCTest

/// 在模拟器上把主要页面走一遍，每一步截图（iPhone Duo、普通 iPhone、iPad 都跑）。
///
/// 环境变量（xcodebuild 跑测试时用 TEST_RUNNER_ 开头传进来）：
/// - P2PCLI_SHOTS：截图放哪个目录；不给就只放进测试结果
/// - P2PCLI_CODE、P2PCLI_PASSWORD：一台测试主机（p2pcli-dev host）的设备码和配对密码；给了就连上去开终端打字
/// - P2PCLI_LANG：zh-Hans 或 en
final class P2PCLIUITests: XCTestCase {
    private var app: XCUIApplication!
    private var shots: URL?
    private var device = ""

    override func setUp() {
        continueAfterFailure = false
        let env = ProcessInfo.processInfo.environment
        shots = env["P2PCLI_SHOTS"].map { URL(fileURLWithPath: $0) }
        if let shots { try? FileManager.default.createDirectory(at: shots, withIntermediateDirectories: true) }
        app = XCUIApplication()
        let lang = env["P2PCLI_LANG"] ?? "zh-Hans"
        app.launchArguments += ["-AppleLanguages", "(\(lang))", "-AppleLocale", lang == "en" ? "en_US" : "zh_CN"]
        device = env["SIMULATOR_DEVICE_NAME"]?.replacingOccurrences(of: " ", with: "-") ?? "device"
        XCUIDevice.shared.orientation = .portrait
    }

    private func shot(_ name: String) {
        let image = XCUIScreen.main.screenshot()
        let a = XCTAttachment(screenshot: image)
        a.name = name
        a.lifetime = .keepAlways
        add(a)
        if let shots {
            try? image.pngRepresentation.write(to: shots.appendingPathComponent("\(device)-\(name).png"))
        }
    }

    func testWalkthrough() throws {
        app.launch()
        XCTAssertTrue(app.buttons["open-settings"].firstMatch.waitForExistence(timeout: 30))
        sleep(2)
        shot("01-home")

        app.buttons["open-settings"].firstMatch.tap()
        sleep(1)
        shot("02-settings")
        // 往上滚：内容滚到标题栏下面，看系统的滚动边缘效果
        app.swipeUp()
        sleep(1)
        shot("02b-settings-scrolled")
        app.buttons["close-settings"].firstMatch.tap()
        sleep(1)

        let env = ProcessInfo.processInfo.environment
        guard let code = env["P2PCLI_CODE"], let password = env["P2PCLI_PASSWORD"] else { return }

        app.buttons["connect-new"].firstMatch.tap()
        let codeField = app.textFields["code-field"]
        XCTAssertTrue(codeField.waitForExistence(timeout: 5))
        codeField.tap()
        codeField.typeText(code.filter(\.isNumber))
        let pw = app.secureTextFields["password-field"]
        pw.tap()
        pw.typeText(password)
        shot("03-connect")
        app.buttons["connect-submit"].tap()

        let newTerm = app.buttons["new-terminal"].firstMatch
        XCTAssertTrue(newTerm.waitForExistence(timeout: 40), "没连上测试主机")
        sleep(2)
        shot("04-device")

        newTerm.tap()
        let terminal = app.descendants(matching: .any)["terminal"].firstMatch
        XCTAssertTrue(terminal.waitForExistence(timeout: 10))
        sleep(3)
        terminal.tap()
        sleep(1)
        app.typeText("echo ios-ok-$((6*7))\n")
        sleep(3)
        shot("05-terminal")
        // 命令的输出（ios-ok-42）看截图：终端控件的读屏接口读不到屏幕上的字
        app.typeText("seq 1 60\n")
        sleep(2)
        shot("05b-terminal-scroll")

        // 按键条：上方向键调出上一条命令
        app.buttons["↑"].firstMatch.tap()
        sleep(1)
        shot("06-arrow")

        XCUIDevice.shared.orientation = .landscapeLeft
        sleep(2)
        shot("07-landscape")
        XCUIDevice.shared.orientation = .portrait
        sleep(1)
    }
}
