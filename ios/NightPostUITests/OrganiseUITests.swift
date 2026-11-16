// Filing from the phone, on the demo accounts only (`-demo`): the Inbox
// tabs, hiding pinned conversations, and the Move sheet's folders, labels
// and tabs.

import XCTest

final class OrganiseUITests: XCTestCase {
    override func setUp() {
        continueAfterFailure = false
    }

    private func launch(_ args: [String] = []) -> XCUIApplication {
        let app = XCUIApplication()
        app.launchArguments = ["-demo"] + args
        app.launch()
        return app
    }

    /// Type into the alert's text field (tapped first, so it has the
    /// keyboard however quickly the alert came up).
    private func type(_ app: XCUIApplication, _ text: String) {
        let field = app.alerts.textFields.firstMatch
        XCTAssertTrue(field.waitForExistence(timeout: 5))
        field.tap()
        field.typeText(text)
    }

    private func tab(_ app: XCUIApplication, _ name: String) -> XCUIElement {
        app.buttons.matching(NSPredicate(format: "label == %@ OR label BEGINSWITH %@", name, name + ",")).firstMatch
    }

    /// The tabs show over the Inbox, All first and picked, and picking one
    /// changes the list.
    func testTabsFilterTheInbox() {
        let app = launch()
        let all = tab(app, "All")
        XCTAssertTrue(all.waitForExistence(timeout: 10))
        XCTAssertTrue(all.isSelected, "the Inbox opens on All")
        let everything = app.cells.count
        let social = tab(app, "Social")
        social.tap()
        XCTAssertTrue(social.waitForSelected())
        XCTAssertFalse(all.isSelected)
        XCTAssertLessThan(app.cells.count, everything, "a tab shows part of the Inbox")
    }

    /// Hide tucks the pinned conversations away; Show brings them back.
    func testPinnedCanBeHidden() {
        let app = launch()
        XCTAssertTrue(tab(app, "All").waitForExistence(timeout: 10))
        // The demo has nothing pinned: pin one.
        let cell = app.cells.element(boundBy: 1)
        let who = cell.staticTexts.firstMatch.label
        cell.press(forDuration: 1)
        app.buttons["Pin"].tap()
        // Hide is remembered on the phone (an earlier run may have left it
        // hidden): start from shown.
        if app.buttons["Show pinned"].waitForExistence(timeout: 2) { app.buttons["Show pinned"].tap() }
        let hide = app.buttons["Hide pinned"]
        XCTAssertTrue(hide.waitForExistence(timeout: 5))
        let row = app.cells.staticTexts[who].firstMatch
        hide.tap()
        // (The list only reports rows on screen, so look for this one
        // rather than count them.)
        XCTAssertTrue(app.buttons["Show pinned"].waitForExistence(timeout: 5))
        XCTAssertTrue(row.waitForNonExistence(timeout: 5), "\(who) is tucked away")
        app.buttons["Show pinned"].tap()
        XCTAssertTrue(app.buttons["Hide pinned"].waitForExistence(timeout: 5))
        XCTAssertTrue(row.waitForExistence(timeout: 5))
    }

    /// Press and hold, Move or Label…, a new folder: the conversation goes
    /// into it, out of the Inbox.
    func testMoveToANewFolder() {
        let app = launch()
        XCTAssertTrue(tab(app, "All").waitForExistence(timeout: 10))
        let cell = app.cells.element(boundBy: 0)
        let subject = cell.staticTexts.element(boundBy: 1).label
        cell.press(forDuration: 1)
        app.buttons["Move or Label…"].tap()
        let newFolder = app.buttons["New Folder…"]
        XCTAssertTrue(newFolder.waitForExistence(timeout: 5))
        newFolder.tap()
        type(app, "Clients")
        app.buttons["Make and Move"].tap()
        XCTAssertTrue(newFolder.waitForNonExistence(timeout: 5), "the sheet closes")
        XCTAssertTrue(app.cells.staticTexts[subject].firstMatch.waitForNonExistence(timeout: 5), "out of the Inbox")
    }

    /// Labels go on and off with a tap.
    func testLabelsToggle() {
        let app = launch(["-show", "move"])
        let labels = app.buttons.matching(NSPredicate(format: "label BEGINSWITH 'Labels'")).firstMatch
        XCTAssertTrue(labels.waitForExistence(timeout: 10))
        labels.tap()
        app.buttons["New Label…"].tap()
        type(app, "Receipts")
        app.buttons["Make and Add"].tap()
        let receipts = app.buttons.matching(NSPredicate(format: "label CONTAINS 'Receipts'")).firstMatch
        XCTAssertTrue(receipts.waitForExistence(timeout: 5))
        XCTAssertTrue(receipts.waitForSelected())
        receipts.tap()
        XCTAssertTrue(receipts.waitForSelected(false))
    }

    /// A new tab from the Move sheet holds the conversation, and shows over
    /// the Inbox.
    func testPutInANewTab() {
        let app = launch(["-show", "move"])
        let this = app.buttons.matching(NSPredicate(format: "label BEGINSWITH 'This conversation'")).firstMatch
        XCTAssertTrue(this.waitForExistence(timeout: 10))
        this.tap()
        app.buttons["New Tab…"].tap()
        type(app, "Invoices")
        app.buttons["Make"].tap()
        XCTAssertTrue(app.buttons.matching(NSPredicate(format: "label BEGINSWITH 'This conversation' AND label CONTAINS 'Invoices'")).firstMatch.waitForExistence(timeout: 5))
        app.buttons["Done"].tap()
        let invoices = tab(app, "Invoices")
        XCTAssertTrue(invoices.waitForExistence(timeout: 5))
        invoices.tap()
        XCTAssertTrue(invoices.waitForSelected())
        XCTAssertEqual(app.cells.count, 1, "just the one conversation")
    }
}

private extension XCUIElement {
    /// Wait for the element to be picked (or, with `false`, not).
    func waitForSelected(_ selected: Bool = true, timeout: TimeInterval = 5) -> Bool {
        let done = XCTNSPredicateExpectation(predicate: NSPredicate(format: "isSelected == %@", NSNumber(value: selected)), object: self)
        return XCTWaiter().wait(for: [done], timeout: timeout) == .completed
    }
}
