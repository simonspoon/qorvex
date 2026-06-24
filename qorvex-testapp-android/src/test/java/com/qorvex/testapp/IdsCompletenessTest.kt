package com.qorvex.testapp

import org.junit.Assert.assertTrue
import org.junit.Assert.assertEquals
import org.junit.Test
import java.io.File

/**
 * Pure-JVM test (no emulator) asserting the element-id inventory is complete and
 * internally consistent:
 *
 *  1. Every `Ids.*` string constant has a matching `<item ... type="id"/>` entry
 *     in `res/values/ids.xml`, so each id the UI wires actually exists as a
 *     resource (and thus is reported by `viewIdResourceName` → the agent's
 *     `identifier`).
 *  2. Every Qorvex ActionType-relevant element *kind* is represented at least
 *     once (tappable, value-bearing, text-input, swipeable, long-pressable,
 *     waitable), mirroring the iOS sample's action coverage.
 *
 * This is the Android counterpart of the implicit guarantee the iOS sample makes
 * by having a SwiftUI element per action; here it is asserted mechanically.
 */
class IdsCompletenessTest {

    /**
     * Resolve a file under the `qorvex-testapp-android` module by walking up from
     * the JVM working directory until the module dir (the one containing
     * `build.gradle.kts`) is found. Gradle runs unit tests with the working dir
     * set to the module root, but we don't rely on that: walking up makes the
     * lookup robust if a runner (e.g. an IDE) starts the test from the repo root
     * or a subdirectory instead.
     */
    private fun moduleFile(relativePath: String): File {
        var dir: File? = File("").absoluteFile
        while (dir != null) {
            // The module root is the dir whose build.gradle.kts declares this app.
            val candidateRoot = if (dir.name == "qorvex-testapp-android") dir
                else File(dir, "qorvex-testapp-android").takeIf { it.isDirectory }
            if (candidateRoot != null) {
                val f = File(candidateRoot, relativePath)
                if (f.exists()) return f
            }
            // Also accept the current dir directly being the module root.
            val here = File(dir, relativePath)
            if (here.exists() && File(dir, "build.gradle.kts").exists()) return here
            dir = dir.parentFile
        }
        // Fall back to the documented working-dir assumption so the failure
        // message points at the expected location.
        return File(relativePath)
    }

    private fun idsXml(): String {
        val f = moduleFile("src/main/res/values/ids.xml")
        assertTrue("ids.xml must exist at ${f.absolutePath}", f.exists())
        return f.readText()
    }

    /** The `applicationId` declared in this module's build.gradle.kts. */
    private fun applicationId(): String {
        val gradle = moduleFile("build.gradle.kts")
        assertTrue("build.gradle.kts must exist at ${gradle.absolutePath}", gradle.exists())
        // Match `applicationId = "..."` (the canonical DSL form).
        val match = Regex("""applicationId\s*=\s*"([^"]+)"""").find(gradle.readText())
        assertTrue(
            "build.gradle.kts must declare an applicationId",
            match != null,
        )
        return match!!.groupValues[1]
    }

    /**
     * True iff [xml] contains a full `name="$id"` resource attribute, matching the
     * whole id name (not just a prefix). The closing quote must be followed by
     * whitespace, `type=`, or `/` so a shorter id whose name prefixes a longer
     * one (e.g. `tab` vs `tab_controls`) is not falsely matched.
     */
    private fun hasIdEntry(xml: String, id: String): Boolean =
        Regex("""name="${Regex.escape(id)}"(?=[\s/>]|type=)""").containsMatchIn(xml)

    /** All string id constants declared on the [Ids] object via reflection. */
    private fun declaredIdConstants(): List<String> =
        Ids::class.java.declaredFields
            .filter { it.type == String::class.java }
            .map { it.isAccessible = true; it.get(Ids) as String }
            // Exclude the scroll-item prefix (not itself an id) — its concrete
            // ids scroll_item_1..N are checked separately.
            .filter { it != Ids.SCROLL_ITEM_PREFIX }

    @Test
    fun everyIdConstantHasResourceEntry() {
        val xml = idsXml()
        val missing = declaredIdConstants().filter { id ->
            !hasIdEntry(xml, id)
        }
        assertTrue("ids.xml is missing @id entries for: $missing", missing.isEmpty())
    }

    @Test
    fun allFiftyScrollItemsDeclared() {
        val xml = idsXml()
        for (n in 1..Ids.SCROLL_ITEM_COUNT) {
            val id = Ids.SCROLL_ITEM_PREFIX + n
            assertTrue("ids.xml missing $id", hasIdEntry(xml, id))
        }
    }

    @Test
    fun everyActionKindIsRepresented() {
        // Each Qorvex ActionType maps to at least one element kind below; assert a
        // representative id exists for each so the matrix has a target.
        // tappable (Tap by id/label/type)
        assertHas(Ids.CONTROLS_TAP_BUTTON)
        // tap-coordinate target (TapLocation)
        assertHas(Ids.GESTURE_TAP_AREA)
        // swipeable (Swipe)
        assertHas(Ids.GESTURE_SWIPE_AREA)
        // long-pressable (LongPress)
        assertHas(Ids.GESTURE_LONGPRESS_TARGET)
        // text input (SendKeys)
        assertHas(Ids.TEXT_USERNAME_FIELD)
        // value-bearing (GetValue editable + GetValue label)
        assertHas(Ids.TEXT_USERNAME_FIELD) // editable -> value
        assertHas(Ids.CONTROLS_TAP_COUNT)  // label -> get-value-by-label/screen-info
        // waitable appear (WaitFor)
        assertHas(Ids.DYNAMIC_DELAYED_LABEL)
        // waitable disappear (WaitForNot)
        assertHas(Ids.DYNAMIC_BRIEF_LABEL)
        // target lifecycle is package-level (SetTarget/StartTarget/StopTarget/GetTargetInfo)
        // — covered by the app package com.qorvex.testapp, asserted in the harness.
    }

    private fun assertHas(id: String) {
        assertTrue("ids.xml missing $id", hasIdEntry(idsXml(), id))
    }

    @Test
    fun packageNameMatchesIosBundleId() {
        // The Android applicationId must equal the iOS bundle id so `set-target
        // com.qorvex.testapp` is identical on both platforms. Read the real
        // applicationId from build.gradle.kts so drift is actually caught.
        assertEquals(
            "Android applicationId must equal the iOS bundle id",
            "com.qorvex.testapp",
            applicationId(),
        )
    }
}
