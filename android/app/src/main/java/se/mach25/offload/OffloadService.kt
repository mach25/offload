package se.mach25.offload

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.content.pm.ServiceInfo
import android.net.ConnectivityManager
import android.net.wifi.WifiManager
import android.os.BatteryManager
import android.os.Build
import android.os.IBinder
import android.os.PowerManager
import android.util.Log
import java.io.File

/**
 * Runs `offloadd` and keeps it running (ADR-0066). The daemon is the same Rust binary every other
 * node runs, shipped as `liboffloadd.so` so Android lets it be executed from `nativeLibraryDir`.
 *
 * Three things only an app can do, and nothing else: be a foreground service so it is not
 * killed, hold a multicast lock so mDNS works, and tell the daemon what `BatteryManager` and
 * `ConnectivityManager` say — through `host-facts.json`, which the daemon's probe reads.
 */
class OffloadService : Service() {
    @Volatile private var running = false
    @Volatile private var daemon: Process? = null
    private var multicast: WifiManager.MulticastLock? = null

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        if (running) return START_STICKY
        running = true
        startForeground(NOTIFICATION_ID, notification("starting"), ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE)

        val wifi = applicationContext.getSystemService(Context.WIFI_SERVICE) as WifiManager
        multicast = wifi.createMulticastLock("offload-mdns").apply {
            setReferenceCounted(false)
            acquire()
        }

        Paths.stateDir(this).mkdirs()
        Paths.writeDefaultConfig(this)
        Thread({ reportFacts() }, "offload-facts").start()
        Thread({ watchRequests() }, "offload-approvals").start()
        Thread({ supervise() }, "offload-supervise").start()
        return START_STICKY
    }

    override fun onDestroy() {
        running = false
        daemon?.let { stop(it) }
        multicast?.let { if (it.isHeld) it.release() }
        super.onDestroy()
    }

    /** Start the daemon, wait for it, start it again — with a backoff, so a crash loop is not a spin. */
    private fun supervise() {
        var backoffMs = 2_000L
        while (running) {
            val log = Paths.daemonLog(this)
            val started = System.currentTimeMillis()
            val process = try {
                ProcessBuilder(Paths.offloadd(this).path, "--config", Paths.config(this).path)
                    .apply {
                        environment()["RUST_LOG"] = "info"
                        environment()["HOME"] = filesDir.path
                        environment()["TMPDIR"] = cacheDir.path
                        redirectErrorStream(true)
                        redirectOutput(ProcessBuilder.Redirect.appendTo(log))
                    }
                    .start()
            } catch (e: Exception) {
                log.appendText("offload app: cannot start offloadd: $e\n")
                null
            }
            if (process != null) {
                daemon = process
                updateNotification("running")
                val code = process.waitFor()
                daemon = null
                log.appendText("offload app: offloadd exited with $code\n")
            }
            if (!running) break
            // A daemon that ran for a while and stopped is restarted promptly; one that dies at once
            // is backed off, up to a minute, so a bad config reads as a slow loop and not a hot one.
            backoffMs = if (System.currentTimeMillis() - started > 60_000) 2_000L else minOf(backoffMs * 2, 60_000L)
            updateNotification("restarting in ${backoffMs / 1000}s")
            Thread.sleep(backoffMs)
        }
    }

    /** SIGTERM, so the daemon drains; a kill only if it is still there after its drain deadline. */
    private fun stop(process: Process) {
        try {
            val pid = childPid("liboffloadd.so")
            if (pid != null) android.os.Process.sendSignal(pid, 15) else process.destroy()
            if (!process.waitFor(10, java.util.concurrent.TimeUnit.SECONDS)) process.destroyForcibly()
        } catch (e: Exception) {
            Log.w(TAG, "stopping offloadd", e)
            process.destroyForcibly()
        }
    }

    /**
     * The pid of this app's own child running `binary`, read from `/proc`. `java.lang.Process` on
     * Android exposes no pid, and `destroy()` is a SIGKILL, which would skip the daemon's drain.
     */
    private fun childPid(binary: String): Int? {
        val me = android.os.Process.myPid()
        return File("/proc").listFiles()?.mapNotNull { it.name.toIntOrNull() }?.firstOrNull { pid ->
            try {
                val cmdline = File("/proc/$pid/cmdline").readText()
                // Field 4 of /proc/<pid>/stat is the parent pid; the command name in field 2 may
                // contain spaces, so count from after its closing parenthesis.
                val stat = File("/proc/$pid/stat").readText()
                val ppid = stat.substringAfterLast(')').trim().split(' ')[1].toInt()
                ppid == me && cmdline.contains(binary)
            } catch (e: Exception) {
                false
            }
        }
    }

    /** `host-facts.json` every 15 s: the platform's answer to what `/sys` will not tell an app —
     *  battery, metered (ADR-0066) and thermal status (ADR-0068). */
    private fun reportFacts() {
        val cm = getSystemService(Context.CONNECTIVITY_SERVICE) as ConnectivityManager
        while (running) {
            try {
                val battery = registerReceiver(null, IntentFilter(Intent.ACTION_BATTERY_CHANGED))
                val level = battery?.getIntExtra(BatteryManager.EXTRA_LEVEL, -1) ?: -1
                val scale = battery?.getIntExtra(BatteryManager.EXTRA_SCALE, -1) ?: -1
                val plugged = (battery?.getIntExtra(BatteryManager.EXTRA_PLUGGED, 0) ?: 0) != 0
                val percent = if (level >= 0 && scale > 0) (level * 100 / scale).toString() else "null"
                val metered = if (cm.activeNetwork != null) cm.isActiveNetworkMetered.toString() else "null"
                // ADR-0068: Android's own thermal status, 0 (none) to 6 (shutdown).
                val thermal = getSystemService(PowerManager::class.java).currentThermalStatus
                // Android's own tablet line: smallest screen width of at least 600 dp.
                val form = if (resources.configuration.smallestScreenWidthDp >= 600) "tablet" else "phone"
                val json = """{"battery_percent":$percent,"charging":$plugged,"metered":$metered,"thermal":$thermal,"form":"$form"}"""
                val target = File(Paths.stateDir(this), "host-facts.json")
                val tmp = File(Paths.stateDir(this), "host-facts.json.tmp")
                tmp.writeText(json)
                tmp.renameTo(target)
            } catch (e: Exception) {
                Log.w(TAG, "writing host facts", e)
            }
            Thread.sleep(15_000)
        }
    }

    /**
     * A signing request (ADR-0069 §4) needs a person, and the app may not be on screen: say so with a
     * notification that opens it, where the prompt is shown. Once per request.
     */
    private fun watchRequests() {
        val manager = getSystemService(NotificationManager::class.java)
        manager.createNotificationChannel(
            NotificationChannel(APPROVALS, "Approvals", NotificationManager.IMPORTANCE_HIGH),
        )
        val told = HashSet<String>()
        while (running) {
            val pending = ApprovalKey.pending(this)
            pending.map { it.name }.filterNot { it in told }.forEach { name ->
                told += name
                val open = android.app.PendingIntent.getActivity(
                    this, 0, Intent(this, MainActivity::class.java),
                    android.app.PendingIntent.FLAG_IMMUTABLE or android.app.PendingIntent.FLAG_UPDATE_CURRENT,
                )
                manager.notify(
                    APPROVAL_NOTIFICATION_ID,
                    Notification.Builder(this, APPROVALS)
                        .setContentTitle("Approval requested")
                        .setContentText("Open Offload to confirm or refuse")
                        .setSmallIcon(R.drawable.ic_notification)
                        .setContentIntent(open)
                        .setAutoCancel(true)
                        .build(),
                )
            }
            if (pending.isEmpty()) manager.cancel(APPROVAL_NOTIFICATION_ID)
            Thread.sleep(1_000)
        }
    }

    private fun notification(text: String): Notification {
        val manager = getSystemService(NotificationManager::class.java)
        // The daemon's status is not news: it must not badge the launcher icon as if something
        // were waiting to be read, which it did under the first channel. A channel's settings are
        // fixed once it exists, so the badge-less one has a new id and the old one is removed.
        manager.deleteNotificationChannel("offloadd")
        manager.createNotificationChannel(
            NotificationChannel(CHANNEL, "Offload daemon", NotificationManager.IMPORTANCE_LOW)
                .apply { setShowBadge(false) },
        )
        return Notification.Builder(this, CHANNEL)
            .setContentTitle("Offload")
            .setContentText("offloadd $text")
            .setSmallIcon(R.drawable.ic_notification)
            .setOngoing(true)
            .build()
    }

    private fun updateNotification(text: String) {
        getSystemService(NotificationManager::class.java).notify(NOTIFICATION_ID, notification(text))
    }

    companion object {
        private const val TAG = "offload"
        private const val CHANNEL = "offloadd-status"
        private const val NOTIFICATION_ID = 1
        private const val APPROVALS = "approvals"
        private const val APPROVAL_NOTIFICATION_ID = 2
    }
}

/** Where everything lives. Short on purpose: the control socket path has to fit in `SUN_LEN`. */
object Paths {
    fun stateDir(c: Context) = File(c.filesDir, "s")
    fun config(c: Context) = File(c.filesDir, "node.toml")
    fun daemonLog(c: Context) = File(c.filesDir, "daemon.log")
    fun socket(c: Context) = File(stateDir(c), "offloadd.sock")
    fun offloadd(c: Context) = File(c.applicationInfo.nativeLibraryDir, "liboffloadd.so")
    fun offload(c: Context) = File(c.applicationInfo.nativeLibraryDir, "liboffload.so")

    /** A starting config, written once; after that it is the owner's (or `adb shell run-as`'s). */
    fun writeDefaultConfig(c: Context) {
        val config = config(c)
        if (config.exists()) return
        val name = Build.MODEL.lowercase().replace(Regex("[^a-z0-9-]"), "-")
        config.writeText(
            """
            name = "$name"
            state_dir = "${stateDir(c).path}"

            [cluster]
            listen = "[::]:7433"
            mdns = true
            """.trimIndent() + "\n",
        )
    }
}
