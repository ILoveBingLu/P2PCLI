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

        guard let terminal = openTerminal() else { return }
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

    /// 假装半折立着（启动参数 -P2PCLIFakeFold，只在调试版有效）：看下半屏的键盘模式和 AI 面板。
    /// 要测试主机上装了 Codex：在终端里启动它，截 AI 面板，再退出
    func testFoldDeck() throws {
        app.launchArguments += ["-P2PCLIFakeFold"]
        app.launch()
        XCTAssertTrue(app.buttons["open-settings"].firstMatch.waitForExistence(timeout: 30))
        guard let terminal = openTerminal() else { return }
        sleep(2)
        shot("f1-folded")

        // 立起来时终端空闲会自己弹键盘；没弹就点下半屏的“点这里打字”
        let typeHere = app.buttons["fold-type-here"].firstMatch
        if typeHere.exists { typeHere.tap() }
        sleep(2)
        shot("f2-keyboard")
        app.typeText("echo fold-ok\n")
        sleep(2)
        shot("f3-typed")

        app.typeText("codex\n")
        let compose = app.buttons["fold-compose"].firstMatch
        let found = compose.waitForExistence(timeout: 30)
        sleep(3)
        shot("f4-agent-panel")
        XCTAssertTrue(found, "没认出 Codex，或者 AI 面板没出来")
        compose.tap()
        sleep(2)
        shot("f5-compose")

        // 退出 Codex：收键盘，按两次 Ctrl+C
        terminal.tap()
        app.typeText(XCUIKeyboardKey.escape.rawValue)
        for _ in 0..<2 {
            app.buttons["Ctrl"].firstMatch.tap()
            app.typeText("c")
            sleep(1)
        }
        sleep(3)
        shot("f6-after")
    }

    /// 连上测试主机（环境变量 P2PCLI_CODE、P2PCLI_PASSWORD），开一个新终端；没给测试主机返回 nil
    private func openTerminal() -> XCUIElement? {
        let env = ProcessInfo.processInfo.environment
        guard let code = env["P2PCLI_CODE"], let password = env["P2PCLI_PASSWORD"] else { return nil }

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
        return terminal
    }
}
