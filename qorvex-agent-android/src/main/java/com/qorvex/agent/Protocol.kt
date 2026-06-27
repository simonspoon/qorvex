// Protocol.kt
// Binary wire protocol types and serialization matching qorvex-core/src/protocol.rs
// and the Swift agent's Protocol.swift.
//
// Packet format: [4-byte LE length][1-byte opcode][payload]
// Length = size of opcode + payload (NOT including the 4-byte header).
//
// Strings are length-prefixed: [u32 LE byte_count][UTF-8 bytes].
// Optionals use a u8 presence flag (0 = None, 1 = Some) followed by the value.

package com.qorvex.agent

import java.io.ByteArrayOutputStream
import java.nio.ByteBuffer
import java.nio.ByteOrder

// ---------------------------------------------------------------------------
// OpCode
// ---------------------------------------------------------------------------

/** On-the-wire operation codes matching the Rust `OpCode` enum. */
enum class OpCode(val value: Int) {
    HEARTBEAT(0x01),
    TAP_COORD(0x02),
    TAP_ELEMENT(0x03),
    TAP_BY_LABEL(0x04),
    TAP_WITH_TYPE(0x05),
    TYPE_TEXT(0x06),
    SWIPE(0x07),
    GET_VALUE(0x08),
    LONG_PRESS(0x09),
    DUMP_TREE(0x10),
    SCREENSHOT(0x11),
    SET_TARGET(0x12),
    FIND_ELEMENT(0x13),
    GET_TARGET_INFO(0x14),
    BRIDGE_HEALTH(0x15),
    ERROR(0x99),
    RESPONSE(0xA0);

    companion object {
        fun fromByte(b: Int): OpCode? = entries.firstOrNull { it.value == (b and 0xFF) }
    }
}

/** Sub-type byte inside the Response opcode payload. */
enum class ResponseType(val value: Int) {
    OK(0x00),
    ERROR(0x01),
    TREE(0x02),
    SCREENSHOT(0x03),
    VALUE(0x04),
    ELEMENT(0x05),
    TARGET_INFO(0x06),
}

// ---------------------------------------------------------------------------
// Request / Response models
// ---------------------------------------------------------------------------

/** A decoded request from the Rust host. */
sealed class AgentRequest {
    object Heartbeat : AgentRequest()
    data class TapCoord(val x: Int, val y: Int) : AgentRequest()
    data class TapElement(val selector: String, val timeoutMs: Long?) : AgentRequest()
    data class TapByLabel(val label: String, val timeoutMs: Long?) : AgentRequest()
    data class TapWithType(
        val selector: String,
        val byLabel: Boolean,
        val elementType: String,
        val timeoutMs: Long?,
    ) : AgentRequest()
    data class TypeText(val text: String) : AgentRequest()
    data class Swipe(
        val startX: Int,
        val startY: Int,
        val endX: Int,
        val endY: Int,
        val duration: Double?,
    ) : AgentRequest()
    data class GetValue(
        val selector: String,
        val byLabel: Boolean,
        val elementType: String?,
        val timeoutMs: Long?,
    ) : AgentRequest()
    data class LongPress(val x: Int, val y: Int, val duration: Double) : AgentRequest()
    object DumpTree : AgentRequest()
    object Screenshot : AgentRequest()
    data class SetTarget(val bundleId: String) : AgentRequest()
    data class FindElement(
        val selector: String,
        val byLabel: Boolean,
        val elementType: String?,
    ) : AgentRequest()
    object GetTargetInfo : AgentRequest()
    object BridgeHealth : AgentRequest()
}

/** A response to send back to the Rust host. */
sealed class AgentResponse {
    object Ok : AgentResponse()
    data class Error(val message: String) : AgentResponse()
    data class Tree(val json: String) : AgentResponse()
    data class Screenshot(val data: ByteArray) : AgentResponse()
    data class Value(val value: String?) : AgentResponse()
    data class Element(val json: String) : AgentResponse()
    data class TargetInfo(val json: String) : AgentResponse()
}

// ---------------------------------------------------------------------------
// Protocol errors
// ---------------------------------------------------------------------------

sealed class ProtocolException(message: String) : Exception(message) {
    class InvalidOpCode(val byte: Int) :
        ProtocolException("invalid opcode: 0x%02X".format(byte and 0xFF))
    class InsufficientData : ProtocolException("insufficient data in buffer")
    class Utf8Error : ProtocolException("invalid UTF-8 in string field")
    class InvalidPayload(msg: String) : ProtocolException("invalid payload: $msg")
}

// ---------------------------------------------------------------------------
// Cursor (sequential little-endian reader)
// ---------------------------------------------------------------------------

/** A simple cursor over a ByteArray for sequential little-endian reads. */
class ProtocolCursor(private val data: ByteArray) {
    var position: Int = 0
        private set

    val remaining: Int get() = data.size - position

    fun readUInt8(): Int {
        if (remaining < 1) throw ProtocolException.InsufficientData()
        return (data[position++].toInt() and 0xFF)
    }

    fun readBool(): Boolean = readUInt8() != 0

    fun readInt32(): Int {
        if (remaining < 4) throw ProtocolException.InsufficientData()
        val v = ByteBuffer.wrap(data, position, 4).order(ByteOrder.LITTLE_ENDIAN).int
        position += 4
        return v
    }

    fun readUInt32(): Long {
        if (remaining < 4) throw ProtocolException.InsufficientData()
        val v = ByteBuffer.wrap(data, position, 4).order(ByteOrder.LITTLE_ENDIAN).int
        position += 4
        return v.toLong() and 0xFFFFFFFFL
    }

    fun readUInt64(): Long {
        if (remaining < 8) throw ProtocolException.InsufficientData()
        val v = ByteBuffer.wrap(data, position, 8).order(ByteOrder.LITTLE_ENDIAN).long
        position += 8
        return v
    }

    fun readFloat64(): Double {
        if (remaining < 8) throw ProtocolException.InsufficientData()
        val v = ByteBuffer.wrap(data, position, 8).order(ByteOrder.LITTLE_ENDIAN).double
        position += 8
        return v
    }

    /** Read a length-prefixed UTF-8 string: [u32 LE byte_count][UTF-8 bytes]. */
    fun readString(): String {
        val len = readUInt32().toInt()
        if (len < 0 || remaining < len) throw ProtocolException.InsufficientData()
        val slice = data.copyOfRange(position, position + len)
        position += len
        return try {
            String(slice, Charsets.UTF_8)
        } catch (e: Exception) {
            throw ProtocolException.Utf8Error()
        }
    }

    /** Read an optional string: [u8 flag] then optional [string]. */
    fun readOptionalString(): String? {
        val flag = readUInt8()
        return if (flag == 0) null else readString()
    }

    /** Read an optional trailing timeout_ms. Returns null if no bytes remain. */
    fun readOptionalTimeoutMs(): Long? {
        if (remaining == 0) return null
        val flag = readUInt8()
        return if (flag == 0) null else readUInt64()
    }
}

// ---------------------------------------------------------------------------
// Decode request
// ---------------------------------------------------------------------------

/** Decode a request from wire bytes (opcode + payload, after the 4-byte length header). */
fun decodeRequest(data: ByteArray): AgentRequest {
    val cursor = ProtocolCursor(data)
    val raw = cursor.readUInt8()
    val opCode = OpCode.fromByte(raw) ?: throw ProtocolException.InvalidOpCode(raw)

    return when (opCode) {
        OpCode.HEARTBEAT -> AgentRequest.Heartbeat

        OpCode.TAP_COORD -> {
            val x = cursor.readInt32()
            val y = cursor.readInt32()
            AgentRequest.TapCoord(x, y)
        }

        OpCode.TAP_ELEMENT -> {
            val selector = cursor.readString()
            val timeoutMs = cursor.readOptionalTimeoutMs()
            AgentRequest.TapElement(selector, timeoutMs)
        }

        OpCode.TAP_BY_LABEL -> {
            val label = cursor.readString()
            val timeoutMs = cursor.readOptionalTimeoutMs()
            AgentRequest.TapByLabel(label, timeoutMs)
        }

        OpCode.TAP_WITH_TYPE -> {
            val selector = cursor.readString()
            val byLabel = cursor.readBool()
            val elementType = cursor.readString()
            val timeoutMs = cursor.readOptionalTimeoutMs()
            AgentRequest.TapWithType(selector, byLabel, elementType, timeoutMs)
        }

        OpCode.TYPE_TEXT -> AgentRequest.TypeText(cursor.readString())

        OpCode.SWIPE -> {
            val startX = cursor.readInt32()
            val startY = cursor.readInt32()
            val endX = cursor.readInt32()
            val endY = cursor.readInt32()
            val hasDuration = cursor.readBool()
            val duration = if (hasDuration) cursor.readFloat64() else null
            AgentRequest.Swipe(startX, startY, endX, endY, duration)
        }

        OpCode.GET_VALUE -> {
            val selector = cursor.readString()
            val byLabel = cursor.readBool()
            val elementType = cursor.readOptionalString()
            val timeoutMs = cursor.readOptionalTimeoutMs()
            AgentRequest.GetValue(selector, byLabel, elementType, timeoutMs)
        }

        OpCode.LONG_PRESS -> {
            val x = cursor.readInt32()
            val y = cursor.readInt32()
            val duration = cursor.readFloat64()
            AgentRequest.LongPress(x, y, duration)
        }

        OpCode.DUMP_TREE -> AgentRequest.DumpTree

        OpCode.SCREENSHOT -> AgentRequest.Screenshot

        OpCode.SET_TARGET -> AgentRequest.SetTarget(cursor.readString())

        OpCode.FIND_ELEMENT -> {
            val selector = cursor.readString()
            val byLabel = cursor.readBool()
            val elementType = cursor.readOptionalString()
            AgentRequest.FindElement(selector, byLabel, elementType)
        }

        OpCode.GET_TARGET_INFO -> AgentRequest.GetTargetInfo

        OpCode.BRIDGE_HEALTH -> AgentRequest.BridgeHealth

        OpCode.ERROR, OpCode.RESPONSE ->
            throw ProtocolException.InvalidPayload(
                "opcode 0x%02X is not a valid request opcode".format(raw),
            )
    }
}

// ---------------------------------------------------------------------------
// Encode response
// ---------------------------------------------------------------------------

/** Encode a response into wire format including the 4-byte length header. */
fun encodeResponse(response: AgentResponse): ByteArray {
    val payload = ByteArrayOutputStream()
    payload.write(OpCode.RESPONSE.value)

    when (response) {
        is AgentResponse.Ok -> payload.write(ResponseType.OK.value)

        is AgentResponse.Error -> {
            payload.write(ResponseType.ERROR.value)
            writeString(payload, response.message)
        }

        is AgentResponse.Tree -> {
            payload.write(ResponseType.TREE.value)
            writeString(payload, response.json)
        }

        is AgentResponse.Screenshot -> {
            payload.write(ResponseType.SCREENSHOT.value)
            writeBytes(payload, response.data)
        }

        is AgentResponse.Value -> {
            payload.write(ResponseType.VALUE.value)
            writeOptionalString(payload, response.value)
        }

        is AgentResponse.Element -> {
            payload.write(ResponseType.ELEMENT.value)
            writeString(payload, response.json)
        }

        is AgentResponse.TargetInfo -> {
            payload.write(ResponseType.TARGET_INFO.value)
            writeString(payload, response.json)
        }
    }

    return encodeFrame(payload.toByteArray())
}

// ---------------------------------------------------------------------------
// Wire helpers
// ---------------------------------------------------------------------------

/** Wrap a payload (opcode + data) with the 4-byte LE length header. */
fun encodeFrame(payload: ByteArray): ByteArray {
    val out = ByteArrayOutputStream(4 + payload.size)
    out.write(u32le(payload.size.toLong()))
    out.write(payload)
    return out.toByteArray()
}

/** Read the payload length from a 4-byte LE header. */
fun readFrameLength(header: ByteArray): Long {
    require(header.size >= 4)
    return ByteBuffer.wrap(header, 0, 4).order(ByteOrder.LITTLE_ENDIAN).int.toLong() and 0xFFFFFFFFL
}

private fun u32le(v: Long): ByteArray =
    ByteBuffer.allocate(4).order(ByteOrder.LITTLE_ENDIAN).putInt(v.toInt()).array()

/** Write a length-prefixed UTF-8 string: [u32 LE byte_count][UTF-8 bytes]. */
private fun writeString(buf: ByteArrayOutputStream, s: String) {
    val bytes = s.toByteArray(Charsets.UTF_8)
    buf.write(u32le(bytes.size.toLong()))
    buf.write(bytes)
}

/** Write raw bytes with a u32 LE length prefix. */
private fun writeBytes(buf: ByteArrayOutputStream, data: ByteArray) {
    buf.write(u32le(data.size.toLong()))
    buf.write(data)
}

/** Write an optional string: [u8 flag] then optional length-prefixed string. */
private fun writeOptionalString(buf: ByteArrayOutputStream, opt: String?) {
    if (opt != null) {
        buf.write(1)
        writeString(buf, opt)
    } else {
        buf.write(0)
    }
}
