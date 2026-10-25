import XCTest
import UIKit

final class SimulatorDeviceTests: XCTestCase {
    let safari = XCUIApplication(bundleIdentifier: "com.apple.mobilesafari")
    var environment: [String: String] { ProcessInfo.processInfo.environment }

    func testSafariAutomationAvailable() {
        safari.launch()
        safari.open(URL(string: "about:blank")!)
        XCTAssertTrue(safari.wait(for: .runningForeground, timeout: 15))
    }

    func open(_ item: String) throws {
        let value = try XCTUnwrap(environment["PF_DEVICE_" + item.uppercased()])
        safari.open(try XCTUnwrap(URL(string: value)))
    }

    func content() {
        XCTAssertTrue(safari.webViews.staticTexts["Serving fixture"].firstMatch.waitForExistence(timeout: 30))
    }

    func report(_ item: String) throws {
        let url = try XCTUnwrap(URL(string: try XCTUnwrap(environment["PF_DEVICE_REPORT"])))
        var request = URLRequest(url: url)
        request.httpMethod = "POST"
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        request.httpBody = try JSONSerialization.data(withJSONObject: ["event": "device.check", "item": item, "lane": "ios-simulator", "outcome": "pass", "os": UIDevice.current.systemVersion, "run": environment["PF_DEVICE_NONCE"] ?? ""])
        let complete = expectation(description: "collector received " + item)
        URLSession.shared.dataTask(with: request) { _, response, error in
            XCTAssertNil(error)
            XCTAssertEqual((response as? HTTPURLResponse)?.statusCode, 204)
            complete.fulfill()
        }.resume()
        wait(for: [complete], timeout: 15)
        let image = XCTAttachment(screenshot: safari.webViews.firstMatch.screenshot())
        image.name = item
        image.lifetime = .keepAlways
        add(image)
    }

    func testMobileSafariMatrix() throws {
        try XCTSkipIf(environment["PF_DEVICE_PIXEL"] != nil, "Separate pixel guest lane")
        continueAfterFailure = false
        safari.launch()
        // Guest is first, before this fresh dedicated simulator signs in to the workspace.
        try open("guest")
        content()
        try report("guest_link")
        try open("standalone")
        content()
        try report("standalone")
        try open("signin")
        let email = safari.webViews.textFields.firstMatch
        XCTAssertTrue(email.waitForExistence(timeout: 60))
        email.tap()
        email.typeText("admin@example.com")
        let password = safari.webViews.secureTextFields.firstMatch
        XCTAssertTrue(password.waitForExistence(timeout: 10))
        password.tap()
        password.typeText("password")
        let login = safari.webViews.buttons.matching(NSPredicate(format: "label CONTAINS[c] 'log in' OR label CONTAINS[c] 'login' OR label CONTAINS[c] 'sign in'")).firstMatch
        XCTAssertTrue(login.waitForExistence(timeout: 10))
        login.tap()
        XCTAssertTrue(safari.webViews.staticTexts["Parts"].firstMatch.waitForExistence(timeout: 30))
        try report("sign_in")
        try open("root")
        content()
        try report("part_root_first_open")
        let reload = safari.buttons.matching(NSPredicate(format: "identifier == 'ReloadButton' OR label == 'Reload'")).firstMatch
        XCTAssertTrue(reload.waitForExistence(timeout: 10))
        reload.tap()
        content()
        try report("part_root_reload")
    }

    func testMobileSafariPixelGuest() throws {
        try XCTSkipIf(environment["PF_DEVICE_PIXEL"] == nil, "Explicit pixel guest lane only")
        continueAfterFailure = false
        safari.launch()
        // This device was just created; no workspace login or cookies are installed.
        try open("pixel")
        let paint = safari.webViews.buttons["Paint communal pixel"].firstMatch
        XCTAssertTrue(paint.waitForExistence(timeout: 90))
        paint.tap()
        XCTAssertTrue(safari.webViews.staticTexts["Paint accepted"].firstMatch.waitForExistence(timeout: 30))
        try report("pixel_paint")
        // This accessible name appears only after Chromium's real Freeze response.
        let frozen = safari.webViews.buttons["Paint frozen pixel"].firstMatch
        XCTAssertTrue(frozen.waitForExistence(timeout: 45))
        frozen.tap()
        XCTAssertTrue(safari.webViews.staticTexts["PfError: frozen pixel reverted"].firstMatch.waitForExistence(timeout: 30))
        try report("pixel_freeze")
    }

    func wrapperBarrier(_ item: String) throws {
        let endpoint = try XCTUnwrap(environment["PF_DEVICE_REPORT"])
            .replacingOccurrences(of: "/report", with: "/wait")
        var components = try XCTUnwrap(URLComponents(string: endpoint))
        components.queryItems = [URLQueryItem(name: "run", value: environment["PF_DEVICE_NONCE"]), URLQueryItem(name: "item", value: item)]
        let url = try XCTUnwrap(components.url)
        for _ in 0..<6 {
            let complete = expectation(description: "actual harness barrier " + item)
            var present = false
            var request = URLRequest(url: url)
            request.timeoutInterval = 35
            URLSession.shared.dataTask(with: request) { data, response, error in
                XCTAssertNil(error)
                XCTAssertEqual((response as? HTTPURLResponse)?.statusCode, 200)
                if let data = data, let rows = try? JSONSerialization.jsonObject(with: data) as? [[String: String]] { present = !rows.isEmpty }
                complete.fulfill()
            }.resume()
            wait(for: [complete], timeout: 40)
            if present { return }
        }
        XCTFail("No real harness barrier: " + item)
    }

    func assertCleanWrapperAddress() {
        // Safari 27 calls its collapsed native address field TabBarItemTitle /
        // Address. Expand it so the check sees the full URL, including fragments.
        let collapsed = safari.textFields.matching(NSPredicate(format: "identifier == 'TabBarItemTitle' OR identifier == 'URL' OR label == 'Address'")).firstMatch
        XCTAssertTrue(collapsed.waitForExistence(timeout: 15))
        collapsed.tap()
        let address = safari.textFields.firstMatch
        XCTAssertTrue(address.waitForExistence(timeout: 15))
        let value = String(describing: address.value ?? "")
        XCTAssertTrue(value.contains("ios-wrapper-part.localtest.me"), value)
        XCTAssertFalse(value.contains("#"), value)
        XCTAssertFalse(value.contains("/c/"), value)
        let cancel = safari.buttons["Cancel"].firstMatch
        XCTAssertTrue(cancel.waitForExistence(timeout: 10))
        cancel.tap()
    }

    func testMobileSafariWrapperRegrant() throws {
        try XCTSkipIf(environment["PF_DEVICE_WRAPPER"] == nil, "Explicit private wrapper/regrant lane")
        continueAfterFailure = false
        safari.launch()
        try open("signin")
        let email = safari.webViews.textFields.firstMatch
        XCTAssertTrue(email.waitForExistence(timeout: 60))
        email.tap(); email.typeText("admin@example.com")
        let password = safari.webViews.secureTextFields.firstMatch
        XCTAssertTrue(password.waitForExistence(timeout: 10))
        password.tap(); password.typeText("password")
        let login = safari.webViews.buttons.matching(NSPredicate(format: "label CONTAINS[c] 'log in' OR label CONTAINS[c] 'login' OR label CONTAINS[c] 'sign in'")).firstMatch
        XCTAssertTrue(login.waitForExistence(timeout: 10)); login.tap()
        XCTAssertTrue(safari.webViews.staticTexts["Parts"].firstMatch.waitForExistence(timeout: 30))
        try open("wrapper")
        XCTAssertTrue(safari.webViews.staticTexts["Wrapper ready with real storage"].firstMatch.waitForExistence(timeout: 60))
        assertCleanWrapperAddress()
        try report("wrapper_first_open")
        // The host now restarts the actual service. Safari and its authenticated session stay open.
        try wrapperBarrier("wrapper_service_restarted")
        let read = safari.webViews.buttons["Read with current capability"].firstMatch
        XCTAssertTrue(read.waitForExistence(timeout: 30)); read.tap()
        try wrapperBarrier("wrapper_regrant_observed")
        XCTAssertTrue(read.waitForExistence(timeout: 30)); read.tap()
        XCTAssertTrue(safari.webViews.staticTexts["Current capability read succeeded"].firstMatch.waitForExistence(timeout: 30))
        assertCleanWrapperAddress()
        XCTAssertFalse(safari.webViews.secureTextFields.firstMatch.exists)
        try report("wrapper_regrant")
    }
}
