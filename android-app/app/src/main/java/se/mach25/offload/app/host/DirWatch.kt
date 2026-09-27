package se.mach25.offload.app.host

import android.os.FileObserver
import java.io.File
import java.util.concurrent.Semaphore
import java.util.concurrent.TimeUnit

/**
 * Wakes a waiting loop when something lands in [dir] or leaves it, and at most every [fallbackMs]
 * anyway. The service's loops checked their directories every second, day and night, part of the
 * app's 2.5% of a core with nobody looking (the phone, session ninety-two). The fallback is for what
 * an observer cannot see: a directory replaced underneath it, or one that did not exist yet.
 */
class DirWatch(private val dir: File, private val fallbackMs: Long = 30_000) {
    private val signal = Semaphore(0)
    private val observer = object : FileObserver(
        dir.apply { mkdirs() },
        CLOSE_WRITE or MOVED_TO or CREATE or DELETE or MOVED_FROM,
    ) {
        override fun onEvent(event: Int, path: String?) {
            signal.release()
        }
    }.also { it.startWatching() }

    /** Until something changes in [dir], or [fallbackMs] passes. */
    fun await() {
        signal.tryAcquire(fallbackMs, TimeUnit.MILLISECONDS)
        signal.drainPermits()
    }

    fun stop() = observer.stopWatching()
}
