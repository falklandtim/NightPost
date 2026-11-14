// Drives the composer as a finger would, on the demo accounts only
// (`-demo`): tap a field, type, and check the text arrived. `-churn`
// reloads the list every second, as a long sync does.

import XCTest

final class ComposeUITests: XCTestCase {
    override func setUp() {
        continueAfterFailure = false
    }

    /// A new message: To and Subject take typing.
    func testNewMessageFieldsTakeTyping() {
        typeInFields(show: "new", churn: false, subject: "Hello")
    }

    /// The same while a sync keeps reloading the list.
    func testFieldsTakeTypingDuringSync() {
        typeInFields(show: "new", churn: true, subject: "Hello")
    }

    /// A reply, during a sync: the fields start filled.
    func testReplyFieldsTakeTypingDuringSync() {
        typeInFields(show: "compose", churn: true, subject: nil)
    }

    /// Writing in the body first, then going back up to the fields: the
    /// body's web view must give up the keyboard.
    func testFieldsTakeTypingAfterTheBody() {
        let app = XCUIApplication()
        app.launchArguments = ["-demo", "-show", "compose"]
        app.launch()

        let body = app.webViews.firstMatch
        XCTAssertTrue(body.waitForExistence(timeout: 10))
        body.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.05)).tap()
        sleep(1)
        // (XCUITest can't type into the web view itself, so the body only
        // takes the keyboard here.)
        let to = app.textFields["To"]
        to.tap()
        sleep(1)
        to.typeText(", sam@example.com")
        // The tap may land mid-way through the address already there.
        XCTAssertTrue((to.value as? String ?? "").contains("sam@example.com"))
        let subject = app.textFields["Subject"]
        subject.tap()
        sleep(1)
        subject.typeText("!")
        XCTAssertTrue((subject.value as? String ?? "").contains("!"))
    }

    /// Tapping the word "To" or "Subject" starts typing there too.
    func testLabelsFocusTheirFields() {
        let app = XCUIApplication()
        app.launchArguments = ["-demo", "-show", "new"]
        app.launch()

        let toLabel = app.staticTexts["To"]
        XCTAssertTrue(toLabel.waitForExistence(timeout: 10))
        toLabel.tap()
        app.typeText("sam@example.com")
        XCTAssertEqual(app.textFields["To"].value as? String, "sam@example.com")

        app.staticTexts["Subject"].tap()
        app.typeText("Hello")
        XCTAssertEqual(app.textFields["Subject"].value as? String, "Hello")
    }

    private func typeInFields(show: String, churn: Bool, subject expected: String?) {
        let app = XCUIApplication()
        app.launchArguments = ["-demo", "-show", show] + (churn ? ["-churn"] : [])
        app.launch()

        let cc = app.buttons["Cc Bcc"]
        XCTAssertTrue(cc.waitForExistence(timeout: 10))
        cc.tap()
        let bcc = app.textFields["Bcc"]
        XCTAssertTrue(bcc.waitForExistence(timeout: 5))
        bcc.tap()
        bcc.typeText("sam@")
        sleep(2)
        bcc.typeText("example.com")
        XCTAssertEqual(bcc.value as? String, "sam@example.com")

        let subject = app.textFields["Subject"]
        subject.tap()
        sleep(2)
        subject.typeText(show == "new" ? "Hello" : "!")
        // A reply's subject is filled, and the tap may land mid-way.
        if let expected {
            XCTAssertEqual(subject.value as? String, expected)
        } else {
            XCTAssertTrue((subject.value as? String ?? "").contains("!"))
        }
    }
}
