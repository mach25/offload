package se.mach25.offload.app.ui

import android.content.Context

/**
 * Which finished runs this device has put away. A view of the list, never a change to the fleet:
 * the run's record, its log and its branch stay, and `offload ps --all` still lists it. What the
 * owner asked for was a list that cleans itself up, not a way to delete work.
 */
class Archive(context: Context) {
    private val prefs = context.getSharedPreferences("archive", Context.MODE_PRIVATE)

    fun ids(): Set<String> = prefs.getStringSet(IDS, emptySet()).orEmpty()

    fun put(id: String) = prefs.edit().putStringSet(IDS, ids() + id).apply()

    fun restore(id: String) = prefs.edit().putStringSet(IDS, ids() - id).apply()

    /** Forget ids the fleet no longer lists, so the set does not grow for ever. */
    fun keepOnly(listed: Set<String>) {
        val kept = ids().intersect(listed)
        if (kept.size != ids().size) prefs.edit().putStringSet(IDS, kept).apply()
    }

    companion object {
        private const val IDS = "ids"

        /** A finished run leaves the list by itself this long after it started. */
        const val AFTER_SECONDS = 24L * 60 * 60
    }
}
