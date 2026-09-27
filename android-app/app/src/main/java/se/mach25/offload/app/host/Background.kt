package se.mach25.offload.app.host

import android.content.Context

/**
 * Whether the owner wants the daemon kept running after the app leaves the screen: the one
 * exception to ADR-0079's "only while something needs it", and theirs to make (session
 * ninety-four). Off by default. On, the phone stays in the fleet with its screen off, which is
 * what lets it be reached and hear news at once; the cost is battery, eased by ADR-0078's quiet
 * mode, which still slows probing and drops the multicast lock while the screen is off.
 */
object Background {
    private const val PREFS = "background"
    private const val KEEP = "keep_running"

    fun keep(context: Context): Boolean =
        context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).getBoolean(KEEP, false)

    fun setKeep(context: Context, keep: Boolean) {
        context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit().putBoolean(KEEP, keep).apply()
    }
}
