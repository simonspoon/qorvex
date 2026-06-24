// GetValueRuleTest.kt
// Verifies the get-value selection rule (getValueResult) matches the iOS agent's
// handleGetValue semantics and ADR-1. The key parity case: a genuinely empty
// editable field reads as None, NOT its hint/contentDescription label.

package com.qorvex.agent

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class GetValueRuleTest {

    @Test
    fun emptyEditableReadsAsNone_notLabel() {
        // Editable input with no typed text but a hint/contentDescription label.
        // iOS parity: must be None, never the label.
        assertNull(
            "empty editable must not fall back to label",
            getValueResult(editable = true, value = null, label = "Enter email"),
        )
        assertNull(
            "empty-string value editable must not fall back to label",
            getValueResult(editable = true, value = "", label = "Enter email"),
        )
    }

    @Test
    fun nonEmptyEditableReturnsValue() {
        assertEquals(
            "user@example.com",
            getValueResult(editable = true, value = "user@example.com", label = "Enter email"),
        )
    }

    @Test
    fun nonEditableFallsBackToLabel() {
        // Static text: value is null, label carries the text (retrievable).
        assertEquals(
            "Welcome",
            getValueResult(editable = false, value = null, label = "Welcome"),
        )
    }

    @Test
    fun nonEditableEmptyLabelIsNone() {
        assertNull(getValueResult(editable = false, value = null, label = null))
        assertNull(getValueResult(editable = false, value = null, label = ""))
    }

    @Test
    fun nonEditableNeverReportsValueField() {
        // value() returns null for non-editable nodes per ADR-1, but even if a
        // value were supplied, a non-editable node reports its label.
        assertEquals(
            "Total",
            getValueResult(editable = false, value = "ignored", label = "Total"),
        )
    }
}
