package se.mach25.offload.app.ui

import android.content.Context

/**
 * What the submit dialog remembers on this device: the repositories runs were submitted on, most
 * recent first, and the model last used. A convenience for the person typing, never read by the
 * daemon. A URL is not trivial to type on a phone, and typing it once is enough.
 */
class Remembered(context: Context) {
    private val prefs = context.getSharedPreferences("submit", Context.MODE_PRIVATE)

    fun repos(): List<String> =
        prefs.getString(REPOS, "").orEmpty().lines().filter { it.isNotBlank() }

    fun model(): String = prefs.getString(MODEL, "").orEmpty()

    /** After a submission the fleet accepted: a refused one may have been a typo. */
    fun used(repo: String, model: String) {
        val given = repo.trim()
        val repos = if (given.isEmpty()) repos() else (listOf(given) + repos().filter { it != given }).take(KEEP)
        prefs.edit().putString(REPOS, repos.joinToString("\n")).putString(MODEL, model.trim()).apply()
    }

    companion object {
        private const val REPOS = "repos"
        private const val MODEL = "model"
        private const val KEEP = 8

        /**
         * Enough to tell repositories apart on a chip: `https://github.com/me/api.git` as `me/api`,
         * and a one-part path with its host, `http://nas:8765/demo.git` as `nas/demo`. The port's
         * colon is not a path separator — splitting on it made the second `8765/demo`.
         */
        fun short(url: String): String {
            val bare = url.trim().trimEnd('/').removeSuffix(".git").substringAfter("://")
            // scp-style `git@host:me/api` has its path after the colon; a URL after the first `/`.
            val (host, path) = if ("://" !in url && ':' in bare && '/' !in bare.substringBefore(':')) {
                bare.substringBefore(':').substringAfter('@') to bare.substringAfter(':')
            } else {
                bare.substringBefore('/').substringAfter('@').substringBefore(':') to bare.substringAfter('/', "")
            }
            val parts = path.split('/').filter { it.isNotBlank() }
            return when {
                parts.size >= 2 -> parts.takeLast(2).joinToString("/")
                parts.size == 1 -> "$host/${parts[0]}"
                else -> bare
            }
        }
    }
}
