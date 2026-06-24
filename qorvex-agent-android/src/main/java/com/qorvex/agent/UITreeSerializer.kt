// UITreeSerializer.kt
// Serializes the Android AccessibilityNodeInfo tree to JSON matching the
// UIElement structure in qorvex-core/src/element.rs and the Swift agent's
// UITreeSerializer.swift (UIElementJSON / FrameJSON).
//
// Frozen serde JSON keys (see arch §3.1 / ADR-1):
//   AXUniqueId -> identifier   (string, optional)
//   AXLabel    -> label        (string, optional)
//   AXValue    -> value        (string, optional)
//   type       -> element_type (string, optional)
//   frame      -> { x, y, width, height } (f64, optional)
//   children   -> [UIElement]  (array, default [])
//   role       -> role         (string, optional)
//   hittable   -> hittable     (bool, optional)

package com.qorvex.agent

/**
 * JSON representation of an element's frame (position and size in screen pixels).
 * Matches FrameJSON in the Swift agent.
 */
data class FrameJSON(
    val x: Double,
    val y: Double,
    val width: Double,
    val height: Double,
)

/**
 * JSON representation of a UI element, matching the Rust `UIElement` struct and
 * the Swift `UIElementJSON`. Null optionals are omitted on the wire (Rust's
 * `#[serde(default)]` supplies the defaults), mirroring Swift's JSONEncoder.
 */
data class UIElementJSON(
    val axUniqueId: String?,
    val axLabel: String?,
    val axValue: String?,
    val type: String?,
    val frame: FrameJSON?,
    val children: List<UIElementJSON>,
    val role: String?,
    val hittable: Boolean?,
) {
    /** Serialize this element (and its subtree) to a JSON object string. */
    fun toJson(): String {
        val sb = StringBuilder()
        writeTo(sb)
        return sb.toString()
    }

    private fun writeTo(sb: StringBuilder) {
        sb.append('{')
        var first = true
        first = appendStringField(sb, first, "AXUniqueId", axUniqueId)
        first = appendStringField(sb, first, "AXLabel", axLabel)
        first = appendStringField(sb, first, "AXValue", axValue)
        first = appendStringField(sb, first, "type", type)
        if (frame != null) {
            appendKeySep(sb, first, "frame")
            first = false
            appendFrame(sb, frame)
        }
        // children is always present (defaults to [] in the model).
        appendKeySep(sb, first, "children")
        first = false
        sb.append('[')
        for ((i, child) in children.withIndex()) {
            if (i > 0) sb.append(',')
            child.writeTo(sb)
        }
        sb.append(']')
        first = appendStringField(sb, first, "role", role)
        if (hittable != null) {
            appendKeySep(sb, first, "hittable")
            sb.append(if (hittable) "true" else "false")
        }
        sb.append('}')
    }

    private fun appendFrame(sb: StringBuilder, f: FrameJSON) {
        sb.append('{')
        sb.append("\"x\":").append(numToJson(f.x)).append(',')
        sb.append("\"y\":").append(numToJson(f.y)).append(',')
        sb.append("\"width\":").append(numToJson(f.width)).append(',')
        sb.append("\"height\":").append(numToJson(f.height))
        sb.append('}')
    }

    /** Appends `"key":` with a leading comma when not the first field. */
    private fun appendKeySep(sb: StringBuilder, first: Boolean, key: String) {
        if (!first) sb.append(',')
        sb.append('"').append(key).append("\":")
    }

    private fun appendStringField(
        sb: StringBuilder,
        first: Boolean,
        key: String,
        value: String?,
    ): Boolean {
        if (value == null) return first
        appendKeySep(sb, first, key)
        sb.append(quote(value))
        return false
    }

    companion object {
        /** Format a Double as a finite JSON number (NaN/Inf become 0.0). */
        private fun numToJson(d: Double): String {
            if (d.isNaN() || d.isInfinite()) return "0.0"
            // Whole numbers serialize as e.g. "12.0" to remain valid JSON floats,
            // which Rust's f64 deserializer accepts identically to "12".
            return if (d == d.toLong().toDouble()) "${d.toLong()}.0" else d.toString()
        }

        /** JSON-escape and quote a string. */
        fun quote(s: String): String {
            val sb = StringBuilder(s.length + 2)
            sb.append('"')
            for (c in s) {
                when (c.code) {
                    '"'.code -> sb.append("\\\"")
                    '\\'.code -> sb.append("\\\\")
                    0x0A -> sb.append("\\n") // newline
                    0x0D -> sb.append("\\r") // carriage return
                    0x09 -> sb.append("\\t") // tab
                    0x08 -> sb.append("\\b") // backspace
                    0x0C -> sb.append("\\f") // form feed
                    else ->
                        if (c.code < 0x20) {
                            sb.append("\\u%04x".format(c.code))
                        } else {
                            sb.append(c)
                        }
                }
            }
            sb.append('"')
            return sb.toString()
        }
    }
}

/** Serialize a list of UIElementJSON to a JSON array string (the DumpTree shape). */
fun serializeTree(elements: List<UIElementJSON>): String {
    val sb = StringBuilder()
    sb.append('[')
    for ((i, e) in elements.withIndex()) {
        if (i > 0) sb.append(',')
        sb.append(e.toJson())
    }
    sb.append(']')
    return sb.toString()
}

/**
 * The get-value result rule, matching the iOS agent's `handleGetValue`
 * (Swift `CommandHandler.handleGetValue`) and ADR-1 (value = editable text else
 * null; label = text else contentDescription).
 *
 * Pure decision so it is unit-testable on the JVM, decoupled from
 * `AccessibilityNodeInfo`:
 *  - editable control: report the value field only; a genuinely empty editable
 *    returns null (None) — never the hint/contentDescription. This is the iOS
 *    parity fix: an empty input reads as None, not its label.
 *  - non-editable node: fall back to the label so static text stays retrievable
 *    (empty label -> null).
 *
 * @param editable whether the node is an editable input control
 * @param value    the node's value field (editable text, else null)
 * @param label    the node's label (text or contentDescription, else null)
 */
fun getValueResult(editable: Boolean, value: String?, label: String?): String? {
    if (editable) {
        return if (value.isNullOrEmpty()) null else value
    }
    return if (label.isNullOrEmpty()) null else label
}
