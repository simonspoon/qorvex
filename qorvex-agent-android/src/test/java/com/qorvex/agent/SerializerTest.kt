// SerializerTest.kt
// Verifies the UIElement JSON serializer emits the frozen serde keys (§3.1 /
// ADR-1) that Rust's UIElement deserializer expects, and that the produced JSON
// round-trips through a JSON parser. The exact keys are the cross-area contract.

package com.qorvex.agent

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class SerializerTest {

    @Test
    fun emitsFrozenKeys() {
        val el = UIElementJSON(
            axUniqueId = "login",
            axLabel = "Sign In",
            axValue = "user@example.com",
            type = "Button",
            frame = FrameJSON(10.0, 20.0, 100.0, 44.0),
            children = emptyList(),
            role = "android.widget.Button",
            hittable = true,
        )
        val json = el.toJson()
        // The Rust serde renames: AXUniqueId/AXLabel/AXValue/type/frame/children/role/hittable.
        assertTrue("AXUniqueId key present", json.contains("\"AXUniqueId\":\"login\""))
        assertTrue("AXLabel key present", json.contains("\"AXLabel\":\"Sign In\""))
        assertTrue("AXValue key present", json.contains("\"AXValue\":\"user@example.com\""))
        assertTrue("type key present", json.contains("\"type\":\"Button\""))
        assertTrue("role FQCN present", json.contains("\"role\":\"android.widget.Button\""))
        assertTrue("hittable present", json.contains("\"hittable\":true"))
        assertTrue("frame x present", json.contains("\"x\":10.0"))
        assertTrue("frame width present", json.contains("\"width\":100.0"))
        assertTrue("children array present", json.contains("\"children\":["))
    }

    @Test
    fun omitsNullOptionals() {
        // A read-only TextView: text lands in label, value is null and omitted.
        val el = UIElementJSON(
            axUniqueId = null,
            axLabel = "Welcome",
            axValue = null,
            type = "TextView",
            frame = FrameJSON(0.0, 0.0, 200.0, 30.0),
            children = emptyList(),
            role = "android.widget.TextView",
            hittable = false,
        )
        val json = el.toJson()
        assertTrue("AXValue omitted when null", !json.contains("AXValue"))
        assertTrue("AXUniqueId omitted when null", !json.contains("AXUniqueId"))
        assertTrue("label still present", json.contains("\"AXLabel\":\"Welcome\""))
        assertTrue("hittable false present", json.contains("\"hittable\":false"))
    }

    @Test
    fun nestedChildrenSerialize() {
        val child = UIElementJSON(
            axUniqueId = "child",
            axLabel = null,
            axValue = null,
            type = "TextView",
            frame = FrameJSON(0.0, 0.0, 50.0, 20.0),
            children = emptyList(),
            role = null,
            hittable = false,
        )
        val parent = UIElementJSON(
            axUniqueId = "parent",
            axLabel = null,
            axValue = null,
            type = "FrameLayout",
            frame = FrameJSON(0.0, 0.0, 100.0, 100.0),
            children = listOf(child),
            role = null,
            hittable = false,
        )
        val tree = serializeTree(listOf(parent))
        assertTrue("wrapped in array", tree.startsWith("[") && tree.endsWith("]"))
        assertTrue("child nested", tree.contains("\"AXUniqueId\":\"child\""))
        assertTrue("parent present", tree.contains("\"AXUniqueId\":\"parent\""))
    }

    @Test
    fun jsonStringsAreEscaped() {
        val el = UIElementJSON(
            axUniqueId = null,
            axLabel = "say \"hi\"\nthere",
            axValue = null,
            type = null,
            frame = null,
            children = emptyList(),
            role = null,
            hittable = null,
        )
        val json = el.toJson()
        assertTrue("quotes escaped", json.contains("\\\"hi\\\""))
        assertTrue("newline escaped", json.contains("\\n"))
    }

    @Test
    fun emptyTreeIsEmptyArray() {
        assertEquals("[]", serializeTree(emptyList()))
    }
}
