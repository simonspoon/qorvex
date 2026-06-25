// NodeMapper.kt
// Maps Android AccessibilityNodeInfo trees to the UIElement JSON model and
// resolves selectors agent-side, mirroring the Swift CommandHandler's
// XCUITest-predicate resolution (exact match on identifier/label + optional
// `[N]` index + optional element-type filter).
//
// Mapping per ADR-1 (frozen):
//   identifier (AXUniqueId) = viewIdResourceName bare entry name
//   label      (AXLabel)    = text if non-empty, else contentDescription
//   value      (AXValue)    = text for editable nodes, else null
//   element_type (type)     = short className (last `.`-segment of FQCN)
//   frame                   = boundsInScreen { x=left, y=top, width, height }
//   role                    = full className (FQCN, advisory)
//   hittable                = isEnabled && isVisibleToUser
//   children                = recursive getChild(i)
//
// ADR-1 amendment (task 107): hittable dropped the `isClickable` term. iOS
// `isHittable` means "visible & hit-testable" (true for plain labels), not
// "interactive". The original `isClickable` conflated interactivity with
// hit-testability, so `wait-for` on a non-clickable label (a status TextView)
// passed on iOS but timed out on Android. The field's only consumers
// (executor wait-for / wait-for-not, CLI display) want "is it really present &
// tappable-at-location"; taps go by coordinate regardless of clickability.

package com.qorvex.agent

import android.graphics.Rect
import android.os.Build
import android.view.accessibility.AccessibilityNodeInfo

/** Selector base + optional 0-based index parsed from a trailing `[N]`. */
data class ParsedSelector(val base: String, val index: Int?)

object NodeMapper {

    // Defense-in-depth limits, mirroring the Swift serializer.
    private const val MAX_TREE_DEPTH = 60
    private const val MAX_TREE_ELEMENTS = 5000

    /**
     * Release a node's native handle.
     *
     * `AccessibilityNodeInfo.recycle()` is deprecated and a no-op on API 33+
     * (the platform pools handles internally there), but on API 24..32 it is
     * still required to avoid leaking native node handles across a long-lived
     * `am instrument -w` serve loop. Guarding the call this way silences the
     * deprecation on new APIs while keeping the real release on old ones.
     * `recycle()` itself swallows the no-op; any IllegalState from a
     * double-recycle is defensively ignored.
     */
    @Suppress("DEPRECATION")
    fun recycle(node: AccessibilityNodeInfo?) {
        if (node == null) return
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) return
        try {
            node.recycle()
        } catch (_: IllegalStateException) {
            // Already recycled — nothing to release.
        }
    }

    /**
     * Parse a trailing `[N]` index from a selector. Only trailing `[digits]`
     * triggers indexing; non-numeric content, empty brackets, negative numbers,
     * or no brackets are treated as a literal selector (index = null).
     *
     * Mirrors `parseSelectorIndex` in the Swift agent and `parse_selector_index`
     * in driver.rs.
     */
    fun parseSelectorIndex(selector: String): ParsedSelector {
        val lastBracket = selector.lastIndexOf('[')
        if (lastBracket < 0 || !selector.endsWith("]")) {
            return ParsedSelector(selector, null)
        }
        val digits = selector.substring(lastBracket + 1, selector.length - 1)
        if (digits.isEmpty() || !digits.all { it.isDigit() }) {
            return ParsedSelector(selector, null)
        }
        val n = digits.toIntOrNull()
        if (n == null || n < 0) {
            return ParsedSelector(selector, null)
        }
        return ParsedSelector(selector.substring(0, lastBracket), n)
    }

    /** The bare resource-id entry name (e.g. `pkg:id/login` -> `login`). */
    fun bareResourceId(node: AccessibilityNodeInfo): String? {
        val full = node.viewIdResourceName ?: return null
        if (full.isEmpty()) return null
        val slash = full.lastIndexOf('/')
        return if (slash >= 0 && slash < full.length - 1) full.substring(slash + 1) else full
    }

    /** ADR-1 label: text if non-empty, else contentDescription. */
    fun label(node: AccessibilityNodeInfo): String? {
        val text = node.text?.toString()
        if (!text.isNullOrEmpty()) return text
        val desc = node.contentDescription?.toString()
        if (!desc.isNullOrEmpty()) return desc
        return null
    }

    /** ADR-1 value: text for editable nodes, else null. */
    fun value(node: AccessibilityNodeInfo): String? {
        if (!node.isEditable) return null
        val text = node.text?.toString()
        return if (text.isNullOrEmpty()) null else text
    }

    /** Whether the node is an editable control (input field). */
    fun isEditable(node: AccessibilityNodeInfo): Boolean = node.isEditable

    /** Short className: last `.`-segment of the FQCN (e.g. `android.widget.Button` -> `Button`). */
    fun shortType(node: AccessibilityNodeInfo): String? {
        val cls = node.className?.toString() ?: return null
        if (cls.isEmpty()) return null
        val dot = cls.lastIndexOf('.')
        return if (dot >= 0 && dot < cls.length - 1) cls.substring(dot + 1) else cls
    }

    /** Full className (FQCN), the advisory role. */
    fun role(node: AccessibilityNodeInfo): String? {
        val cls = node.className?.toString()
        return if (cls.isNullOrEmpty()) null else cls
    }

    /**
     * ADR-1 hittable (amended, task 107): isEnabled && isVisibleToUser.
     * Matches iOS `isHittable` ("visible & hit-testable"); `isClickable` was
     * dropped because it conflated interactivity with hit-testability and broke
     * `wait-for` parity on non-clickable labels.
     */
    fun hittable(node: AccessibilityNodeInfo): Boolean =
        node.isEnabled && node.isVisibleToUser

    fun frame(node: AccessibilityNodeInfo): FrameJSON {
        val r = Rect()
        node.getBoundsInScreen(r)
        return FrameJSON(
            x = r.left.toDouble(),
            y = r.top.toDouble(),
            width = (r.right - r.left).toDouble(),
            height = (r.bottom - r.top).toDouble(),
        )
    }

    /**
     * Serialize a node subtree to UIElementJSON. Prunes empty scaffolding nodes
     * (no identity, no area, no surviving children) like the Swift serializer.
     * Returns null when pruned or when depth/element-count limits are exceeded.
     *
     * Ownership: this serializer copies every node's data into the returned
     * JSON, so the caller-supplied `node` is NOT recycled here (the caller owns
     * the node it passed in — typically the active-window root, which must stay
     * alive). Child handles obtained via `getChild(i)` ARE recycled here, since
     * they are created by this walk and not handed back.
     */
    fun serialize(node: AccessibilityNodeInfo?, depth: Int, count: IntArray): UIElementJSON? {
        if (node == null) return null
        if (depth >= MAX_TREE_DEPTH || count[0] >= MAX_TREE_ELEMENTS) return null
        count[0]++

        val frame = frame(node)
        val children = ArrayList<UIElementJSON>()
        for (i in 0 until node.childCount) {
            val child = node.getChild(i) ?: continue
            try {
                val serialized = serialize(child, depth + 1, count)
                if (serialized != null) children.add(serialized)
            } finally {
                recycle(child)
            }
        }

        val identifier = bareResourceId(node)
        val lbl = label(node)
        val v = value(node)

        val hasIdentity = !identifier.isNullOrEmpty() || !lbl.isNullOrEmpty() || !v.isNullOrEmpty()
        val hasArea = frame.width > 0 && frame.height > 0
        if (!hasIdentity && !hasArea && children.isEmpty()) {
            return null
        }

        return UIElementJSON(
            axUniqueId = identifier,
            axLabel = lbl,
            axValue = v,
            type = shortType(node),
            frame = frame,
            children = children,
            role = role(node),
            hittable = hittable(node),
        )
    }

    /**
     * Collect all nodes in the subtree matching the predicate, depth-first.
     *
     * Ownership: the `root` passed by the caller is never recycled here (caller
     * owns it). Every child handle obtained via `getChild(i)` that does NOT end
     * up in `out` is recycled before returning, so no handle leaks. Matched
     * handles are placed in `out` and survive — `resolve` then keeps the one it
     * returns and recycles the rest. Callers must recycle the surviving `out`
     * nodes once done (other than any node they hand back to the caller chain).
     */
    fun collectMatches(
        root: AccessibilityNodeInfo?,
        predicate: (AccessibilityNodeInfo) -> Boolean,
        out: MutableList<AccessibilityNodeInfo>,
    ) {
        if (root == null) return
        if (predicate(root)) out.add(root)
        for (i in 0 until root.childCount) {
            val child = root.getChild(i) ?: continue
            // A child is retained in `out` (by its own recursion) iff it itself
            // matches. Its matched descendants are distinct handles in `out`, so
            // recycling a non-matching child does not touch them. Evaluate the
            // predicate once, before recursing, to decide ownership.
            val childMatched = predicate(child)
            collectMatches(child, predicate, out)
            if (!childMatched) {
                recycle(child)
            }
        }
    }

    /**
     * Resolve a node by selector against a root, mirroring Swift exact-match
     * predicates with `[N]` index. `byLabel` selects label vs identifier;
     * `elementType` (short className) filters when present.
     *
     * Returns the matching node, or null if none. The Rust side handles glob
     * wildcards over the dump-tree output; the agent resolves by exact match.
     *
     * Ownership: the returned node (if any) is handed to the caller and is NOT
     * recycled here — the caller must recycle it once done (unless it is the
     * `root`, which it never is: matches are always descendants or the root
     * itself; if the root matches it is returned and the caller still owns it).
     * Every other matched handle is recycled before returning so the walk leaks
     * nothing. The `root` is never recycled here.
     */
    fun resolve(
        root: AccessibilityNodeInfo?,
        selector: String,
        byLabel: Boolean,
        elementType: String?,
    ): AccessibilityNodeInfo? {
        val (base, index) = parseSelectorIndex(selector)
        val predicate = { node: AccessibilityNodeInfo ->
            val selectorMatches =
                if (byLabel) label(node) == base else bareResourceId(node) == base
            val typeMatches = elementType == null || shortType(node) == elementType
            selectorMatches && typeMatches
        }
        val matches = ArrayList<AccessibilityNodeInfo>()
        collectMatches(root, predicate, matches)
        val chosen = if (index != null) matches.getOrNull(index) else matches.firstOrNull()
        // Release every matched handle we are not returning. Never recycle the
        // `chosen` node (handed to the caller) nor the `root` (caller owns it):
        // root can appear in `matches` when it matched but a `[N]` index selects
        // a descendant instead, and the caller still needs its root afterwards.
        for (m in matches) {
            if (m !== chosen && m !== root) recycle(m)
        }
        return chosen
    }
}
