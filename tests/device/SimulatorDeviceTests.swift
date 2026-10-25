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
}
