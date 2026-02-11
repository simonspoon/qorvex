// QorvexAgentTests.swift
// XCTest entry point for the qorvex agent. This is a UI test that starts the
// TCP server and keeps running indefinitely, accepting commands from the Rust host.

import XCTest

final class QorvexAgentTests: XCTestCase {

    /// The main agent entry point. This test starts the TCP server and blocks
    /// indefinitely, acting as a persistent automation agent.
    ///
    /// The test runner will invoke this via:
    ///   xcodebuild test -only-testing:QorvexAgentUITests/QorvexAgentTests/testRunAgent ...
    ///
    /// The agent operates on whatever app is currently in the foreground of the
    /// simulator. It does NOT launch any app itself -- the Rust host is responsible
    /// for launching and managing the target app via `simctl`.
    func testRunAgent() throws {
        let app = XCUIApplication()
        // Do NOT call app.launch() -- we operate on the current foreground app.
        // XCUIApplication() without launch() gives us a handle to query the
        // accessibility hierarchy without changing the running app.

        let handler = CommandHandler(app: app)
        let server = AgentServer(port: 8080, handler: handler)

        try server.start()

        NSLog("[qorvex-agent] Agent started, waiting for commands on port 8080")

        // Keep the test running indefinitely until the process is killed.
        // The test runner framework requires us to wait on an expectation;
        // using timeout .infinity means this test will never finish on its own.
        let keepAlive = XCTestExpectation(description: "Agent running indefinitely")
        wait(for: [keepAlive], timeout: .infinity)
    }
}
