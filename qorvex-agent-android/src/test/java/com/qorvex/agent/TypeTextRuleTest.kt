// TypeTextRuleTest.kt
// Verifies the send-keys text rule (typeTextResult): append keeps the current
// text (hint counts as empty), replace sets exactly the typed text.

package com.qorvex.agent

import org.junit.Assert.assertEquals
import org.junit.Test

class TypeTextRuleTest {

    @Test
    fun appendAddsAfterCurrentText() {
        assertEquals("xabc", typeTextResult(current = "x", showingHint = false, typed = "abc", replace = false))
        assertEquals("abc", typeTextResult(current = null, showingHint = false, typed = "abc", replace = false))
    }

    @Test
    fun appendTreatsHintAsEmpty() {
        assertEquals("abc", typeTextResult(current = "Enter email", showingHint = true, typed = "abc", replace = false))
    }

    @Test
    fun replaceSetsExactlyTyped() {
        assertEquals("abc", typeTextResult(current = "x", showingHint = false, typed = "abc", replace = true))
        assertEquals("", typeTextResult(current = "x", showingHint = false, typed = "", replace = true))
    }
}
