// The tabs' bar over the Inbox on the demo accounts (`-demo`): nothing
// covers the title, the tabs or the first rows. On iOS 26 a bar that
// arrived after the list went behind it, under a frosted layer.

import XCTest

final class TopBarUITests: XCTestCase {
    override func setUp() {
        continueAfterFailure = false
    }

    /// A first sync: the tabs arrive only once it reaches the Inbox, after
    /// the list is on screen (`-latetabs`), and the list keeps reloading
    /// (`-churn`). (More's actions were also once a dialog on the list,
    /// which left the same layer.)
    func testTabsThatArriveLateAreUncovered() {
        let app = XCUIApplication()
        app.launchArguments = ["-demo", "-syncing", "-churn", "-latetabs"]
        app.launch()
        let all = allTab(app)
        XCTAssertTrue(all.waitForExistence(timeout: 10))
        sleep(6)
        assertUncovered(app, all)
    }

    /// Archive has no tabs, so the bar goes; back in the Inbox it returns.
    func testTabsAreUncoveredBackInTheInbox() {
        let app = XCUIApplication()
        app.launchArguments = ["-demo"]
        app.launch()
        XCTAssertTrue(allTab(app).waitForExistence(timeout: 10))
        pick(app, "Archive")
        XCTAssertTrue(allTab(app).waitForNonExistence(timeout: 5))
        pick(app, "Inbox")
        let all = allTab(app)
        XCTAssertTrue(all.waitForExistence(timeout: 5))
        sleep(2)
        assertUncovered(app, all)
    }

    private func allTab(_ app: XCUIApplication) -> XCUIElement {
        app.buttons.matching(NSPredicate(format: "label BEGINSWITH 'All'")).firstMatch
    }

    /// Choose a list from the ☰ menu.
    private func pick(_ app: XCUIApplication, _ list: String) {
        app.navigationBars.buttons.element(boundBy: 0).tap()
        let item = app.buttons[list]
        XCTAssertTrue(item.waitForExistence(timeout: 5))
        item.tap()
    }

    /// The layer only covered the screen, so look at it: the picked tab's
    /// blue shows through only when nothing is over it.
    private func assertUncovered(_ app: XCUIApplication, _ tab: XCUIElement) {
        let uncovered = hasBlue(tab.screenshot().image)
        if !uncovered {
            let shot = XCTAttachment(screenshot: app.screenshot())
            shot.lifetime = .keepAlways
            add(shot)
        }
        XCTAssertTrue(uncovered, "the All tab is drawn uncovered")
    }

    /// Whether any pixel is a strong blue (the accent colour), not the
    /// washed-out grey of something seen through frosted glass.
    private func hasBlue(_ image: UIImage) -> Bool {
        guard let cg = image.cgImage else { return false }
        let (w, h) = (cg.width, cg.height)
        var px = [UInt8](repeating: 0, count: w * h * 4)
        guard let ctx = CGContext(data: &px, width: w, height: h, bitsPerComponent: 8, bytesPerRow: w * 4,
                                  space: CGColorSpaceCreateDeviceRGB(), bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue) else { return false }
        ctx.draw(cg, in: CGRect(x: 0, y: 0, width: w, height: h))
        return stride(from: 0, to: px.count, by: 4).contains { i in
            Int(px[i + 2]) > 200 && Int(px[i]) < 90 && Int(px[i + 1]) < 170
        }
    }
}
