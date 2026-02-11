// Protocol.swift
// Binary wire protocol types and serialization matching qorvex-core/src/protocol.rs.
//
// Packet format: [4-byte LE length][1-byte opcode][payload]
// Length = size of opcode + payload (NOT including the 4-byte header).

import Foundation

// MARK: - OpCode

/// On-the-wire operation codes matching the Rust `OpCode` enum.
enum OpCode: UInt8 {
    case heartbeat  = 0x01
    case tapCoord   = 0x02
    case tapElement = 0x03
    case tapByLabel = 0x04
    case tapWithType = 0x05
    case typeText   = 0x06
    case swipe      = 0x07
    case getValue   = 0x08
    case longPress  = 0x09
    case dumpTree   = 0x10
    case screenshot  = 0x11
    case setTarget   = 0x12
    case error      = 0x99
    case response   = 0xA0
}

// MARK: - Response type discriminator

/// Sub-type byte inside the Response opcode payload.
enum ResponseType: UInt8 {
    case ok         = 0x00
    case error      = 0x01
    case tree       = 0x02
    case screenshot = 0x03
    case value      = 0x04
}

// MARK: - Request

/// A decoded request from the Rust host.
enum AgentRequest {
    case heartbeat
    case tapCoord(x: Int32, y: Int32)
    case tapElement(selector: String)
    case tapByLabel(label: String)
    case tapWithType(selector: String, byLabel: Bool, elementType: String)
    case typeText(text: String)
    case swipe(startX: Int32, startY: Int32, endX: Int32, endY: Int32, duration: Double?)
    case getValue(selector: String, byLabel: Bool, elementType: String?)
    case longPress(x: Int32, y: Int32, duration: Double)
    case dumpTree
    case screenshot
    case setTarget(bundleId: String)
}

// MARK: - Response

/// A response to send back to the Rust host.
enum AgentResponse {
    case ok
    case error(message: String)
    case tree(json: String)
    case screenshot(data: Data)
    case value(String?)
}

// MARK: - Protocol errors

enum ProtocolError: Error, CustomStringConvertible {
    case invalidOpCode(UInt8)
    case insufficientData
    case utf8Error
    case invalidPayload(String)

    var description: String {
        switch self {
        case .invalidOpCode(let byte):
            return String(format: "invalid opcode: 0x%02X", byte)
        case .insufficientData:
            return "insufficient data in buffer"
        case .utf8Error:
            return "invalid UTF-8 in string field"
        case .invalidPayload(let msg):
            return "invalid payload: \(msg)"
        }
    }
}

// MARK: - Cursor (sequential reader)

/// A simple cursor over Data for sequential little-endian reads.
final class ProtocolCursor {
    private let data: Data
    private(set) var position: Int

    var remaining: Int { data.count - position }

    init(_ data: Data) {
        self.data = data
        self.position = 0
    }

    func readUInt8() throws -> UInt8 {
        guard remaining >= 1 else { throw ProtocolError.insufficientData }
        let value = data[data.startIndex + position]
        position += 1
        return value
    }

    func readBool() throws -> Bool {
        return try readUInt8() != 0
    }

    func readInt32() throws -> Int32 {
        guard remaining >= 4 else { throw ProtocolError.insufficientData }
        let start = data.startIndex + position
        let bytes = data[start..<start + 4]
        position += 4
        return bytes.withUnsafeBytes { $0.loadUnaligned(as: Int32.self).littleEndian }
    }

    func readUInt32() throws -> UInt32 {
        guard remaining >= 4 else { throw ProtocolError.insufficientData }
        let start = data.startIndex + position
        let bytes = data[start..<start + 4]
        position += 4
        return bytes.withUnsafeBytes { $0.loadUnaligned(as: UInt32.self).littleEndian }
    }

    func readFloat64() throws -> Double {
        guard remaining >= 8 else { throw ProtocolError.insufficientData }
        let start = data.startIndex + position
        let bytes = data[start..<start + 8]
        position += 8
        let bits = bytes.withUnsafeBytes { $0.loadUnaligned(as: UInt64.self).littleEndian }
        return Double(bitPattern: bits)
    }

    /// Read a length-prefixed UTF-8 string: [u32 LE byte_count][UTF-8 bytes].
    func readString() throws -> String {
        let byteCount = Int(try readUInt32())
        guard remaining >= byteCount else { throw ProtocolError.insufficientData }
        let start = data.startIndex + position
        let slice = data[start..<start + byteCount]
        position += byteCount
        guard let string = String(data: slice, encoding: .utf8) else {
            throw ProtocolError.utf8Error
        }
        return string
    }

    /// Read an optional string: [u8 flag] then optional [string].
    func readOptionalString() throws -> String? {
        let flag = try readUInt8()
        if flag == 0 { return nil }
        return try readString()
    }
}

// MARK: - Decode request

/// Decode a request from wire bytes (opcode + payload, after the 4-byte length header).
func decodeRequest(from data: Data) throws -> AgentRequest {
    let cursor = ProtocolCursor(data)
    let rawOpCode = try cursor.readUInt8()
    guard let opCode = OpCode(rawValue: rawOpCode) else {
        throw ProtocolError.invalidOpCode(rawOpCode)
    }

    switch opCode {
    case .heartbeat:
        return .heartbeat

    case .tapCoord:
        let x = try cursor.readInt32()
        let y = try cursor.readInt32()
        return .tapCoord(x: x, y: y)

    case .tapElement:
        let selector = try cursor.readString()
        return .tapElement(selector: selector)

    case .tapByLabel:
        let label = try cursor.readString()
        return .tapByLabel(label: label)

    case .tapWithType:
        let selector = try cursor.readString()
        let byLabel = try cursor.readBool()
        let elementType = try cursor.readString()
        return .tapWithType(selector: selector, byLabel: byLabel, elementType: elementType)

    case .typeText:
        let text = try cursor.readString()
        return .typeText(text: text)

    case .swipe:
        let startX = try cursor.readInt32()
        let startY = try cursor.readInt32()
        let endX = try cursor.readInt32()
        let endY = try cursor.readInt32()
        let hasDuration = try cursor.readBool()
        let duration: Double? = hasDuration ? try cursor.readFloat64() : nil
        return .swipe(startX: startX, startY: startY, endX: endX, endY: endY, duration: duration)

    case .getValue:
        let selector = try cursor.readString()
        let byLabel = try cursor.readBool()
        let elementType = try cursor.readOptionalString()
        return .getValue(selector: selector, byLabel: byLabel, elementType: elementType)

    case .longPress:
        let x = try cursor.readInt32()
        let y = try cursor.readInt32()
        let duration = try cursor.readFloat64()
        return .longPress(x: x, y: y, duration: duration)

    case .dumpTree:
        return .dumpTree

    case .screenshot:
        return .screenshot

    case .setTarget:
        let bundleId = try cursor.readString()
        return .setTarget(bundleId: bundleId)

    case .error, .response:
        throw ProtocolError.invalidPayload(
            String(format: "opcode 0x%02X is not a valid request opcode", rawOpCode)
        )
    }
}

// MARK: - Encode response

/// Encode a response into wire format including the 4-byte length header.
func encodeResponse(_ response: AgentResponse) -> Data {
    var payload = Data()

    payload.append(OpCode.response.rawValue)

    switch response {
    case .ok:
        payload.append(ResponseType.ok.rawValue)

    case .error(let message):
        payload.append(ResponseType.error.rawValue)
        writeString(&payload, message)

    case .tree(let json):
        payload.append(ResponseType.tree.rawValue)
        writeString(&payload, json)

    case .screenshot(let data):
        payload.append(ResponseType.screenshot.rawValue)
        writeBytes(&payload, data)

    case .value(let optValue):
        payload.append(ResponseType.value.rawValue)
        writeOptionalString(&payload, optValue)
    }

    return encodeFrame(payload)
}

// MARK: - Wire helpers

/// Wrap a payload with the 4-byte LE length header.
func encodeFrame(_ payload: Data) -> Data {
    var frame = Data(capacity: 4 + payload.count)
    var length = UInt32(payload.count).littleEndian
    frame.append(Data(bytes: &length, count: 4))
    frame.append(payload)
    return frame
}

/// Read the payload length from a 4-byte LE header.
func readFrameLength(_ header: Data) -> UInt32 {
    precondition(header.count >= 4)
    return header.withUnsafeBytes { $0.loadUnaligned(as: UInt32.self).littleEndian }
}

/// Write a length-prefixed UTF-8 string into a Data buffer.
private func writeString(_ buf: inout Data, _ string: String) {
    let bytes = Array(string.utf8)
    var length = UInt32(bytes.count).littleEndian
    buf.append(Data(bytes: &length, count: 4))
    buf.append(contentsOf: bytes)
}

/// Write raw bytes with a u32 LE length prefix.
private func writeBytes(_ buf: inout Data, _ data: Data) {
    var length = UInt32(data.count).littleEndian
    buf.append(Data(bytes: &length, count: 4))
    buf.append(data)
}

/// Write an optional string: [u8 flag] then optional length-prefixed string.
private func writeOptionalString(_ buf: inout Data, _ opt: String?) {
    if let s = opt {
        buf.append(1 as UInt8)
        writeString(&buf, s)
    } else {
        buf.append(0 as UInt8)
    }
}
