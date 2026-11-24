// Swipes on the demo accounts only (`-demo`): the desktop's defaults, the
// More button, and choosing a side's buttons in Settings → Swipes.

import XCTest

final class SwipeUITests: XCTestCase {
    override func setUp() {
        continueAfterFailure = false
    }

    /// The app on the demo, with Settings → Swipes back on the defaults
    /// (they're kept on the phone, so an earlier run may have changed them).
    private func launch() -> XCUIApplication {
        let app = XCUIApplication()
        app.launchArguments = ["-demo"]
        app.launch()
        openSwipes(app)
        app.buttons["Use the Desktop's Defaults"].tap()
        closeSettings(app)
        return app
    }

    private func openSwipes(_ app: XCUIApplication) {
        let menu = app.navigationBars.buttons.element(boundBy: 0)
        XCTAssertTrue(menu.waitForExistence(timeout: 10))
        menu.tap()
        app.buttons["Settings"].tap()
        let swipes = app.buttons["Swipes"]
        XCTAssertTrue(swipes.waitForExistence(timeout: 5))
        swipes.tap()
    }

    private func closeSettings(_ app: XCUIApplication) {
        app.navigationBars.buttons.element(boundBy: 0).tap()
        app.buttons["Done"].tap()
        XCTAssertTrue(app.cells.firstMatch.waitForExistence(timeout: 5))
    }

    /// Drag a row part of the way, so it opens on its buttons rather than
    /// doing the full swipe.
    private func shortSwipe(_ cell: XCUIElement, left: Bool) {
        let from = cell.coordinate(withNormalizedOffset: CGVector(dx: left ? 0.9 : 0.1, dy: 0.5))
        let to = cell.coordinate(withNormalizedOffset: CGVector(dx: left ? 0.45 : 0.55, dy: 0.5))
        from.press(forDuration: 0.05, thenDragTo: to)
    }

    /// Out of the box the phone swipes as the desktop does: left for Trash,
    /// Snooze and More; right for Archive and Read or Unread.
    func testDefaultsMatchTheDesktop() {
        let app = launch()
        shortSwipe(app.cells.element(boundBy: 1), left: true)
        for b in ["Trash", "Snooze", "More"] {
            XCTAssertTrue(app.buttons[b].waitForExistence(timeout: 5), "swiping left shows \(b)")
        }
        XCTAssertFalse(app.buttons["Archive"].exists)

        // Another row, so the first one closes.
        shortSwipe(app.cells.element(boundBy: 2), left: false)
        XCTAssertTrue(app.buttons["Archive"].waitForExistence(timeout: 5))
        XCTAssertTrue(app.buttons.matching(NSPredicate(format: "label == 'Read' OR label == 'Unread'")).firstMatch.exists)
        XCTAssertFalse(app.buttons["Trash"].exists)
    }

    /// More shows the press-and-hold actions. (It's the full swipe here
    /// because a test's tap on a SwiftUI swipe button is unreliable in the
    /// simulator: in some launches every tap only closes the row, with the
    /// old hard-wired buttons too.)
    func testMoreShowsTheRowActions() {
        let app = launch()
        openSwipes(app)
        app.buttons["Swipe Left 1"].tap()
        app.buttons["More…"].tap()
        closeSettings(app)

        // All the way across: XCTest's swipeLeft() only opens the row.
        let cell = app.cells.element(boundBy: 1)
        cell.coordinate(withNormalizedOffset: CGVector(dx: 0.95, dy: 0.5))
            .press(forDuration: 0.05, thenDragTo: cell.coordinate(withNormalizedOffset: CGVector(dx: -0.2, dy: 0.5)), withVelocity: .fast, thenHoldForDuration: 0)
        let moveOrLabel = app.buttons["Move or Label…"].firstMatch
        XCTAssertTrue(moveOrLabel.waitForExistence(timeout: 5), "More shows the press-and-hold actions")
        moveOrLabel.tap()
        XCTAssertTrue(app.buttons["New Folder…"].waitForExistence(timeout: 5), "and they work")
    }

    /// Picking Star for a full swipe right puts it first on that side, and
    /// Nothing turns the left side off.
    func testChoosingTheButtons() {
        let app = launch()
        openSwipes(app)
        app.buttons["Swipe Right 1"].tap()
        app.buttons["Star / unstar"].tap()
        app.buttons["Swipe Left 1"].tap()
        app.buttons["Nothing"].tap()
        closeSettings(app)

        let cell = app.cells.element(boundBy: 1)
        shortSwipe(cell, left: false)
        XCTAssertTrue(app.buttons.matching(NSPredicate(format: "label == 'Star' OR label == 'Unstar'")).firstMatch.waitForExistence(timeout: 5))
        XCTAssertFalse(app.buttons["Archive"].exists, "Star took Archive's place")
        // Close the row, then try the left side.
        cell.coordinate(withNormalizedOffset: CGVector(dx: 0.9, dy: 0.5)).tap()
        shortSwipe(app.cells.element(boundBy: 2), left: true)
        XCTAssertFalse(app.buttons["Trash"].waitForExistence(timeout: 2), "the left side is off")
    }
}
