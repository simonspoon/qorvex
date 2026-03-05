// ParseSelectorIndexTests.swift
// Unit tests for the parseSelectorIndex free function in CommandHandler.swift.

import XCTest

final class ParseSelectorIndexTests: XCTestCase {

    func testWithIndex() {
        let (base, index) = parseSelectorIndex("row[2]")
        XCTAssertEqual(base, "row")
        XCTAssertEqual(index, 2)
    }

    func testNoIndex() {
        let (base, index) = parseSelectorIndex("row")
        XCTAssertEqual(base, "row")
        XCTAssertNil(index)
    }

    func testZeroIndex() {
        let (base, index) = parseSelectorIndex("cell[0]")
        XCTAssertEqual(base, "cell")
        XCTAssertEqual(index, 0)
    }

    func testGlobPlusIndex() {
        let (base, index) = parseSelectorIndex("cell_*[1]")
        XCTAssertEqual(base, "cell_*")
        XCTAssertEqual(index, 1)
    }

    func testMalformedNonNumeric() {
        let (base, index) = parseSelectorIndex("row[abc]")
        XCTAssertEqual(base, "row[abc]")
        XCTAssertNil(index)
    }

    func testEmptyBrackets() {
        let (base, index) = parseSelectorIndex("row[]")
        XCTAssertEqual(base, "row[]")
        XCTAssertNil(index)
    }

    func testNegativeNumber() {
        let (base, index) = parseSelectorIndex("row[-1]")
        XCTAssertEqual(base, "row[-1]")
        XCTAssertNil(index)
    }

    func testLargeIndex() {
        let (base, index) = parseSelectorIndex("item[999]")
        XCTAssertEqual(base, "item")
        XCTAssertEqual(index, 999)
    }

    func testNoBrackets() {
        let (base, index) = parseSelectorIndex("simple-selector")
        XCTAssertEqual(base, "simple-selector")
        XCTAssertNil(index)
    }

    func testUnclosedBracket() {
        let (base, index) = parseSelectorIndex("row[2")
        XCTAssertEqual(base, "row[2")
        XCTAssertNil(index)
    }
}
