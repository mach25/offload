package se.mach25.offload.app.host

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.Service
import android.content.BroadcastReceiver
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
import se.mach25.offload.app.R
import se.mach25.offload.client.Daemon
import java.io.File

/**
 * Runs `offloadd` and keeps it running (ADR-0066). The daemon is the same Rust binary every other
 * node runs, shipped as `liboffloadd.so` so Android lets it be executed from `nativeLibraryDir`.
 *
 * Three things only an app can do, and nothing else: be a foreground service so it is not
 * killed, hold a multicast lock so mDNS works, and tell the daemon what `BatteryManager` and
 * `ConnectivityManager` say — through `host-facts.json`, which the daemon's probe reads.
 *
 * And, since ADR-0079, **it runs only while something needs it**: the app on screen, or a run held
 * here. Otherwise it stops itself, and the phone is simply away from the fleet, which the fleet was
 * built to tolerate: news for it waits in the holder's outbox and arrives the next time the app is
 * opened. There is deliberately no push: the owner would rather go without notifications than
 * install another app to receive them.
 */
class OffloadService : Service() {
    @Volatile private var running = false
    @Volatile private var daemon: Process? = null
    private var multicast: WifiManager.MulticastLock? = null
    private var screen: BroadcastReceiver? = null

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        if (running) return START_NOT_STICKY
        running = true
        startedAt = System.currentTimeMillis()
        startForeground(
            NOTIFICATION_ID,
            notification("Starting"),
            ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE,
        )

        val wifi = applicationContext.getSystemService(Context.WIFI_SERVICE) as WifiManager
        // ADR-0078: the lock only while the screen is on. Held, it keeps the Wi-Fi radio passing
        // every multicast packet on the network to the CPU, and one night of it was most of the
        // the phone's battery. Discovery with the screen off is not needed: peers already known are
        // dialled directly, and a new one is found the next time somebody looks at the phone.
        multicast = wifi.createMulticastLock("offload-mdns").apply { setReferenceCounted(false) }
        val power = getSystemService(PowerManager::class.java)
        screenChanged(power.isInteractive)
        screen = object : BroadcastReceiver() {
            override fun onReceive(context: Context, intent: Intent) {
                screenChanged(intent.action == Intent.ACTION_SCREEN_ON)
            }
        }.also {
            registerReceiver(it, IntentFilter().apply {
                addAction(Intent.ACTION_SCREEN_ON)
                addAction(Intent.ACTION_SCREEN_OFF)
            })
        }

        Paths.stateDir(this).mkdirs()
        Paths.writeDefaultConfig(this)
        // This phone's own delivery route (ADR-0010), added before the daemon starts so it needs
        // no restart: the fleet's news — a question above all — arrives as a notification with
        // the app closed.
        try {
            se.mach25.offload.client.Device(Paths.stateDir(this).path, Paths.config(this).path)
                .ensureAppSink(Paths.notifications(this).path)
        } catch (e: Exception) {
            Log.w(TAG, "adding the app's notification route", e)
        }
        Thread({ reportFacts() }, "offload-facts").start()
        Thread({ watchRequests() }, "offload-approvals").start()
        Thread({ watchNotifications() }, "offload-notifications").start()
        Thread({ supervise() }, "offload-supervise").start()
        Thread({ lifecycle() }, "offload-lifecycle").start()
        // Not sticky: a service the system killed is not restarted behind the owner's back. The
        // app opening starts it again.
        return START_NOT_STICKY
    }

    /**
     * ADR-0079: keep the daemon only while something needs it, and say what that is in the ongoing
     * notification — the runs held here by name, not "Offload is running".
     *
     * Asked every 10 s. A daemon that does not answer is not idle (unknown is not none), so it is
     * kept, but only for [UNANSWERED_LIMIT_MS] with nobody looking: a daemon that never answers
     * must not keep a phone awake all night either.
     */
    private fun lifecycle() {
        var daemon: Daemon? = null
        var answeredAt = System.currentTimeMillis()
        var shown = ""
        while (running) {
            Thread.sleep(10_000)
            if (!running) break
            val now = System.currentTimeMillis()
            if (daemon == null) daemon = runCatching { Daemon(Paths.socket(this).path) }.getOrNull()
            val holding = daemon?.let { d -> runCatching { d.status().holding }.getOrNull() }
            if (holding != null) answeredAt = now
            val quietSince = maxOf(lastSeen, startedAt)
            val text = when {
                holding != null && holding.size == 1 -> "Running: ${holding[0]}"
                holding != null && holding.size > 1 -> "Running ${holding.size} runs: ${holding[0]}, …"
                visible -> "Connected to your fleet"
                // The owner's word in Settings, asked each time so turning it off takes effect.
                Background.keep(this) -> "Connected to your fleet"
                now - quietSince < GRACE_MS -> "Connected to your fleet"
                holding == null && now - answeredAt < UNANSWERED_LIMIT_MS -> "Starting"
                else -> null
            }
            if (text == null) {
                Log.i(TAG, "nothing needs the daemon: no run held and nobody looking")
                stopSelf()
                return
            }
            if (text != shown) {
                updateNotification(text)
                shown = text
            }
        }
    }

    override fun onDestroy() {
        running = false
        daemon?.let { stop(it) }
        screen?.let { unregisterReceiver(it) }
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
                val code = process.waitFor()
                daemon = null
                log.appendText("offload app: offloadd exited with $code\n")
            }
            if (!running) break
            // A daemon that ran for a while and stopped is restarted promptly; one that dies at once
            // is backed off, up to a minute, so a bad config reads as a slow loop and not a hot one.
            backoffMs = if (System.currentTimeMillis() - started > 60_000) 2_000L else minOf(backoffMs * 2, 60_000L)
            updateNotification("Restarting in ${backoffMs / 1000}s")
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

    /** The screen came on or went off: the multicast lock with it, and the facts at once, so the
     *  daemon leaves quiet (ADR-0078) without waiting out the 15 s writer. */
    private fun screenChanged(on: Boolean) {
        multicast?.let { if (on && !it.isHeld) it.acquire() else if (!on && it.isHeld) it.release() }
        Thread({ writeFacts() }, "offload-facts-now").start()
    }

    /** `host-facts.json` every 15 s: the platform's answer to what `/sys` will not tell an app —
     *  battery, metered (ADR-0066), thermal status (ADR-0068) and whether the screen is on
     *  (ADR-0078). */
    private fun reportFacts() {
        while (running) {
            writeFacts()
            Thread.sleep(15_000)
        }
    }

    @Synchronized
    private fun writeFacts() {
        val cm = getSystemService(Context.CONNECTIVITY_SERVICE) as ConnectivityManager
        run {
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
                val interactive = getSystemService(PowerManager::class.java).isInteractive
                val json = """{"battery_percent":$percent,"charging":$plugged,"metered":$metered,"thermal":$thermal,"form":"$form","interactive":$interactive}"""
                val target = File(Paths.stateDir(this), "host-facts.json")
                val tmp = File(Paths.stateDir(this), "host-facts.json.tmp")
                tmp.writeText(json)
                tmp.renameTo(target)
            } catch (e: Exception) {
                Log.w(TAG, "writing host facts", e)
            }
        }
    }

    /**
     * A signing request (ADR-0069 §4) needs a person, and the app may not be on screen: say so with a
     * notification that opens it, where the prompt is shown. Once per request.
     */
    private fun watchRequests() {
        val manager = getSystemService(NotificationManager::class.java)
        // Vibrates for the reason the Questions channel does: the first one did not, and on a
        // locked tablet the request expired unnoticed. A new id, since a channel's settings are fixed.
        manager.deleteNotificationChannel("approvals")
        manager.createNotificationChannel(
            NotificationChannel(APPROVALS, "Approvals", NotificationManager.IMPORTANCE_HIGH).apply {
                enableVibration(true)
                vibrationPattern = longArrayOf(0, 250, 150, 250)
                enableLights(true)
            },
        )
        val told = HashSet<String>()
        val watch = DirWatch(ApprovalKey.requestsDir(this))
        while (running) {
            val pending = ApprovalKey.pending(this)
            pending.map { it.name }.filterNot { it in told }.forEach { name ->
                told += name
                // To the Questions tab, where the Review button that raises the prompt is.
                val open = android.app.PendingIntent.getActivity(
                    this, 0,
                    Intent(this, se.mach25.offload.app.ui.MainActivity::class.java)
                        .putExtra("open", "questions")
                        .addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP),
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
            watch.await()
        }
        watch.stop()
    }

    /**
     * Show what the app's sink filed: one notification per file, then the file is gone. The daemon's
     * outbox has already deduplicated and retried (ADR-0010); this only turns a delivery into
     * something a person sees. A question is urgent and opens the Questions tab; the rest is news.
     */
    private fun watchNotifications() {
        val manager = getSystemService(NotificationManager::class.java)
        // Vibration asked for explicitly: a question is the one notification worth feeling in a
        // pocket, and left to the default it was easy to miss. A channel's settings are fixed once
        // it exists, so this one has a new id and the first is removed. (A phone in mute mode
        // stays silent whatever a channel asks — that is the owner's setting, rightly.)
        manager.deleteNotificationChannel("questions")
        manager.createNotificationChannel(
            NotificationChannel(QUESTIONS, "Questions from agents", NotificationManager.IMPORTANCE_HIGH).apply {
                enableVibration(true)
                vibrationPattern = longArrayOf(0, 250, 150, 250)
                enableLights(true)
            },
        )
        manager.createNotificationChannel(
            NotificationChannel(NEWS, "Fleet news", NotificationManager.IMPORTANCE_DEFAULT),
        )
        var next = 100
        val watch = DirWatch(Paths.notifications(this))
        while (running) {
            val filed = Paths.notifications(this)
                .listFiles { f -> f.name.endsWith(".json") && !f.name.startsWith(".") }
                ?.sortedBy { it.name }
                .orEmpty()
            for (file in filed) {
                try {
                    val note = org.json.JSONObject(file.readText())
                    val asked = note.optString("kind") == "asked"
                    val open = Intent(this, se.mach25.offload.app.ui.MainActivity::class.java)
                        .putExtra("open", if (asked) "questions" else "runs")
                        .addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP)
                    val id = next++
                    manager.notify(
                        id,
                        Notification.Builder(this, if (asked) QUESTIONS else NEWS)
                            .setContentTitle(note.optString("title", "Offload"))
                            .setContentText(note.optString("summary"))
                            .setStyle(Notification.BigTextStyle().bigText(note.optString("summary")))
                            .setSmallIcon(R.drawable.ic_notification)
                            .setContentIntent(
                                android.app.PendingIntent.getActivity(
                                    this, id, open,
                                    android.app.PendingIntent.FLAG_IMMUTABLE or android.app.PendingIntent.FLAG_UPDATE_CURRENT,
                                ),
                            )
                            .setAutoCancel(true)
                            .build(),
                    )
                } catch (e: Exception) {
                    Log.w(TAG, "showing ${file.name}", e)
                }
                file.delete()
            }
            watch.await()
        }
        watch.stop()
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
            .setContentText(text)
            .setSmallIcon(R.drawable.ic_notification)
            .setOngoing(true)
            .build()
    }

    private fun updateNotification(text: String) {
        getSystemService(NotificationManager::class.java).notify(NOTIFICATION_ID, notification(text))
    }

    companion object {
        /**
         * Stop the daemon and start it again, for a change it reads only at start: its first
         * fleet, or its config. Stopped through the service, so it drains; started after a pause,
         * so the new start is not folded into the stop still in progress.
         */
        fun restart(context: Context) {
            context.stopService(Intent(context, OffloadService::class.java))
            android.os.Handler(android.os.Looper.getMainLooper()).postDelayed({
                context.startForegroundService(Intent(context, OffloadService::class.java))
            }, 3_000)
        }

        /** Whether the app is on screen, set by `MainActivity`. */
        @Volatile var visible = false
        /** When the app was last on screen. */
        @Volatile var lastSeen = 0L
        @Volatile private var startedAt = 0L

        /** How long after the app leaves the screen the daemon stays: long enough that switching to
         *  another app and back does not stop and restart it, and for news already on its way. */
        private const val GRACE_MS = 90_000L
        private const val UNANSWERED_LIMIT_MS = 5 * 60_000L

        private const val TAG = "offload"
        private const val CHANNEL = "offloadd-status"
        private const val NOTIFICATION_ID = 1
        private const val APPROVALS = "approvals-v2"
        private const val QUESTIONS = "questions-v2"
        private const val NEWS = "news"
        private const val APPROVAL_NOTIFICATION_ID = 2
    }
}

/** Where everything lives. Short on purpose: the control socket path has to fit in `SUN_LEN`. */
object Paths {
    fun stateDir(c: Context) = File(c.filesDir, "s")
    fun config(c: Context) = File(c.filesDir, "node.toml")
    fun daemonLog(c: Context) = File(c.filesDir, "daemon.log")
    /** Where the app's sink files the fleet's notifications for the service to show. */
    fun notifications(c: Context) = File(stateDir(c), "notifications")
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
