// ProtocolWireTest.kt
// Pure-JVM unit tests proving the Kotlin codec is byte-for-byte compatible with
// the Rust wire protocol in qorvex-core/src/protocol.rs. These assert the exact
// bytes the Rust encoder/decoder produces and consumes — the same invariants as
// the Rust `tests` module (heartbeat_wire_format, tap_coord_wire_format, etc.)
// and the request/response round-trips.

package com.qorvex.agent

import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import java.nio.ByteBuffer
import java.nio.ByteOrder

class ProtocolWireTest {

    private fun le32(v: Int): ByteArray =
        ByteBuffer.allocate(4).order(ByteOrder.LITTLE_ENDIAN).putInt(v).array()

    private fun decodeResponseFrame(frame: ByteArray): ByteArray {
        // Strip the 4-byte length header, returning the payload (opcode + body).
        return frame.copyOfRange(4, frame.size)
    }

    // -- Response wire format (matches Rust response_ok_wire_format) --------

    @Test
    fun responseOkWireFormat() {
        val wire = encodeResponse(AgentResponse.Ok)
        // length: 1 (opcode 0xA0) + 1 (type 0x00) = 2
        assertArrayEquals(le32(2), wire.copyOfRange(0, 4))
        assertEquals(0xA0, wire[4].toInt() and 0xFF) // OpCode::Response
        assertEquals(0x00, wire[5].toInt() and 0xFF) // ResponseType::Ok
        assertEquals(6, wire.size)
    }

    @Test
    fun responseErrorWireFormat() {
        val msg = "element not found"
        val wire = encodeResponse(AgentResponse.Error(msg))
        val payload = decodeResponseFrame(wire)
        assertEquals(0xA0, payload[0].toInt() and 0xFF)
        assertEquals(0x01, payload[1].toInt() and 0xFF) // ResponseType::Error
        // length prefix of the string, then UTF-8 bytes
        assertArrayEquals(le32(msg.toByteArray().size), payload.copyOfRange(2, 6))
        assertEquals(msg, String(payload.copyOfRange(6, payload.size)))
    }

    @Test
    fun responseValueNoneWireFormat() {
        val wire = encodeResponse(AgentResponse.Value(null))
        val payload = decodeResponseFrame(wire)
        assertEquals(0xA0, payload[0].toInt() and 0xFF)
        assertEquals(0x04, payload[1].toInt() and 0xFF) // ResponseType::Value
        assertEquals(0x00, payload[2].toInt() and 0xFF) // None presence flag
        assertEquals(3, payload.size)
    }

    @Test
    fun responseValueSomeWireFormat() {
        val wire = encodeResponse(AgentResponse.Value("hello@example.com"))
        val payload = decodeResponseFrame(wire)
        assertEquals(0x04, payload[1].toInt() and 0xFF)
        assertEquals(0x01, payload[2].toInt() and 0xFF) // Some presence flag
        assertArrayEquals(le32("hello@example.com".toByteArray().size), payload.copyOfRange(3, 7))
        assertEquals("hello@example.com", String(payload.copyOfRange(7, payload.size)))
    }

    @Test
    fun responseScreenshotWireFormat() {
        val png = byteArrayOf(0x89.toByte(), 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A)
        val wire = encodeResponse(AgentResponse.Screenshot(png))
        val payload = decodeResponseFrame(wire)
        assertEquals(0x03, payload[1].toInt() and 0xFF) // ResponseType::Screenshot
        assertArrayEquals(le32(png.size), payload.copyOfRange(2, 6))
        assertArrayEquals(png, payload.copyOfRange(6, payload.size))
    }

    @Test
    fun responseTreeWireFormat() {
        val json = """[{"type":"View","children":[]}]"""
        val wire = encodeResponse(AgentResponse.Tree(json))
        val payload = decodeResponseFrame(wire)
        assertEquals(0x02, payload[1].toInt() and 0xFF) // ResponseType::Tree
        assertEquals(json, String(payload.copyOfRange(6, payload.size)))
    }

    @Test
    fun frameLengthExcludesHeader() {
        val wire = encodeResponse(AgentResponse.TargetInfo("{}"))
        val declaredLen = readFrameLength(wire.copyOfRange(0, 4))
        assertEquals((wire.size - 4).toLong(), declaredLen)
    }

    // -- Request decoding (matches Rust encode_request output) -------------

    private fun rustHeader(payload: ByteArray): ByteArray = le32(payload.size) + payload

    @Test
    fun decodeHeartbeat() {
        // Rust: [1,0,0,0, 0x01]
        val req = decodeRequest(byteArrayOf(0x01))
        assertTrue(req is AgentRequest.Heartbeat)
    }

    @Test
    fun decodeTapCoord() {
        // opcode 0x02 + i32 x(=1) + i32 y(=2), all LE
        val payload = byteArrayOf(0x02) + le32(1) + le32(2)
        val req = decodeRequest(payload) as AgentRequest.TapCoord
        assertEquals(1, req.x)
        assertEquals(2, req.y)
    }

    @Test
    fun decodeTapCoordNegative() {
        val payload = byteArrayOf(0x02) + le32(100) + le32(-42)
        val req = decodeRequest(payload) as AgentRequest.TapCoord
        assertEquals(100, req.x)
        assertEquals(-42, req.y)
    }

    @Test
    fun decodeTapElementNoTimeout() {
        val sel = "login-button"
        val payload = byteArrayOf(0x03) + le32(sel.toByteArray().size) + sel.toByteArray()
        val req = decodeRequest(payload) as AgentRequest.TapElement
        assertEquals(sel, req.selector)
        assertEquals(null, req.timeoutMs)
    }

    @Test
    fun decodeTapElementWithTimeout() {
        val sel = "login-button"
        val timeoutBytes = ByteBuffer.allocate(8).order(ByteOrder.LITTLE_ENDIAN).putLong(5000L).array()
        val payload = byteArrayOf(0x03) +
            le32(sel.toByteArray().size) + sel.toByteArray() +
            byteArrayOf(0x01) + timeoutBytes // Some(5000)
        val req = decodeRequest(payload) as AgentRequest.TapElement
        assertEquals(sel, req.selector)
        assertEquals(5000L, req.timeoutMs)
    }

    @Test
    fun decodeTapWithType() {
        val sel = "submit-btn"
        val type = "Button"
        val payload = byteArrayOf(0x05) +
            le32(sel.toByteArray().size) + sel.toByteArray() +
            byteArrayOf(0x00) + // by_label = false
            le32(type.toByteArray().size) + type.toByteArray()
        val req = decodeRequest(payload) as AgentRequest.TapWithType
        assertEquals(sel, req.selector)
        assertEquals(false, req.byLabel)
        assertEquals(type, req.elementType)
        assertEquals(null, req.timeoutMs)
    }

    @Test
    fun decodeSwipeWithDuration() {
        val durBytes = ByteBuffer.allocate(8).order(ByteOrder.LITTLE_ENDIAN).putDouble(0.5).array()
        val payload = byteArrayOf(0x07) +
            le32(50) + le32(800) + le32(50) + le32(200) +
            byteArrayOf(0x01) + durBytes
        val req = decodeRequest(payload) as AgentRequest.Swipe
        assertEquals(50, req.startX)
        assertEquals(200, req.endY)
        assertEquals(0.5, req.duration!!, 1e-9)
    }

    @Test
    fun decodeSwipeNoDuration() {
        val payload = byteArrayOf(0x07) +
            le32(0) + le32(100) + le32(0) + le32(500) + byteArrayOf(0x00)
        val req = decodeRequest(payload) as AgentRequest.Swipe
        assertEquals(null, req.duration)
    }

    @Test
    fun decodeGetValueWithType() {
        val sel = "Email"
        val type = "TextField"
        val payload = byteArrayOf(0x08) +
            le32(sel.toByteArray().size) + sel.toByteArray() +
            byteArrayOf(0x01) + // by_label = true
            byteArrayOf(0x01) + le32(type.toByteArray().size) + type.toByteArray() + // Some(type)
            byteArrayOf(0x00) // timeout None
        val req = decodeRequest(payload) as AgentRequest.GetValue
        assertEquals(sel, req.selector)
        assertEquals(true, req.byLabel)
        assertEquals(type, req.elementType)
        assertEquals(null, req.timeoutMs)
    }

    @Test
    fun decodeLongPress() {
        val durBytes = ByteBuffer.allocate(8).order(ByteOrder.LITTLE_ENDIAN).putDouble(1.5).array()
        val payload = byteArrayOf(0x09) + le32(150) + le32(300) + durBytes
        val req = decodeRequest(payload) as AgentRequest.LongPress
        assertEquals(150, req.x)
        assertEquals(300, req.y)
        assertEquals(1.5, req.duration, 1e-9)
    }

    @Test
    fun decodeTypeTextUnicode() {
        val text = "café 😀"
        val payload = byteArrayOf(0x06) + le32(text.toByteArray().size) + text.toByteArray()
        val req = decodeRequest(payload) as AgentRequest.TypeText
        assertEquals(text, req.text)
    }

    @Test
    fun decodeSetTarget() {
        val pkg = "com.example.myapp"
        val payload = byteArrayOf(0x12) + le32(pkg.toByteArray().size) + pkg.toByteArray()
        val req = decodeRequest(payload) as AgentRequest.SetTarget
        assertEquals(pkg, req.bundleId)
    }

    @Test
    fun decodeFindElementNoType() {
        val sel = "row"
        val payload = byteArrayOf(0x13) +
            le32(sel.toByteArray().size) + sel.toByteArray() +
            byteArrayOf(0x00) + // by_label false
            byteArrayOf(0x00) // element_type None
        val req = decodeRequest(payload) as AgentRequest.FindElement
        assertEquals(sel, req.selector)
        assertEquals(false, req.byLabel)
        assertEquals(null, req.elementType)
    }

    @Test
    fun decodeDumpTreeAndGetTargetInfoAndScreenshot() {
        assertTrue(decodeRequest(byteArrayOf(0x10)) is AgentRequest.DumpTree)
        assertTrue(decodeRequest(byteArrayOf(0x11)) is AgentRequest.Screenshot)
        assertTrue(decodeRequest(byteArrayOf(0x14)) is AgentRequest.GetTargetInfo)
    }

    // -- Error handling ----------------------------------------------------

    @Test(expected = ProtocolException.InvalidOpCode::class)
    fun decodeInvalidOpcode() {
        decodeRequest(byteArrayOf(0xFF.toByte()))
    }

    @Test(expected = ProtocolException.InsufficientData::class)
    fun decodeTruncatedPayload() {
        // TapCoord needs 8 bytes after opcode; give only 4.
        decodeRequest(byteArrayOf(0x02, 0, 0, 0, 0))
    }

    @Test(expected = ProtocolException.InvalidPayload::class)
    fun decodeResponseOpcodeAsRequestFails() {
        decodeRequest(byteArrayOf(0xA0.toByte()))
    }

    // -- Frame size sanity --------------------------------------------------

    @Test
    fun encodedFrameMatchesRustLayout() {
        // Build a request payload the way Rust does (header + opcode + fields)
        // and confirm our decoder reads the same fields back from the body.
        val sel = "cell_*[1]"
        val body = byteArrayOf(0x03) + le32(sel.toByteArray().size) + sel.toByteArray() +
            byteArrayOf(0x00) // timeout None
        val frame = rustHeader(body)
        val len = readFrameLength(frame.copyOfRange(0, 4))
        assertEquals(body.size.toLong(), len)
        val req = decodeRequest(frame.copyOfRange(4, frame.size)) as AgentRequest.TapElement
        assertEquals(sel, req.selector)
    }
}
