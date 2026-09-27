package se.mach25.offload

import android.Manifest
import android.app.Activity
import android.content.Intent
import android.graphics.Typeface
import android.net.Uri
import android.os.Bundle
import android.os.PowerManager
import android.provider.Settings
import android.widget.Button
import android.widget.HorizontalScrollView
import android.widget.LinearLayout
import android.widget.ScrollView
import android.widget.TextView

/**
 * Just enough to see the daemon (ADR-0066 §4): start and stop it, and read `offload status` and
 * the tail of its log. Everything else — config, enrolment, runs — goes through
 * `adb shell run-as se.mach25.offload`.
 */
class MainActivity : Activity() {
    private lateinit var output: TextView

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        if (checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) != android.content.pm.PackageManager.PERMISSION_GRANTED) {
            requestPermissions(arrayOf(Manifest.permission.POST_NOTIFICATIONS), 1)
        }

        output = TextView(this).apply {
            typeface = Typeface.MONOSPACE
            textSize = 11f
            setPadding(24, 24, 24, 24)
        }
        val buttons = LinearLayout(this).apply {
            orientation = LinearLayout.HORIZONTAL
            addView(button("Start") { startForegroundService(Intent(this@MainActivity, OffloadService::class.java)); refreshSoon() })
            addView(button("Stop") { stopService(Intent(this@MainActivity, OffloadService::class.java)); refreshSoon() })
            addView(button("Refresh") { refresh() })
            addView(button("Battery") { askToIgnoreBatteryOptimisation() })
            // ADR-0069 §4: made on purpose, not on install — it does nothing until the owner names
            // it with `offload grant approve --hardware-key`, and then every approval needs a touch.
            addView(button("Key") { makeApprovalKey() })
        }
        setContentView(
            LinearLayout(this).apply {
                orientation = LinearLayout.VERTICAL
                // Scrolls sideways: five buttons are wider than a phone held upright, and the last
                // one ("Key") sat off the phone's screen where nothing could tap it.
                addView(HorizontalScrollView(this@MainActivity).apply { addView(buttons) })
                addView(ScrollView(this@MainActivity).apply { addView(output) })
                // Android 15 draws every app edge to edge, and at targetSdk 35 there is no opting
                // out: without this the button row sat under the status bar, where a tap on "Key"
                // landed on the bar instead (measured on the Samsung tablet).
                setOnApplyWindowInsetsListener { view, insets ->
                    val bars = insets.getInsets(android.view.WindowInsets.Type.systemBars())
                    view.setPadding(bars.left, bars.top, bars.right, bars.bottom)
                    insets
                }
            },
        )
        // Opening the app starts the daemon, so a walk can do it with `adb shell am start` and no tap.
        startForegroundService(Intent(this, OffloadService::class.java))
        refreshSoon()
    }

    @Volatile private var visible = false
    @Volatile private var answering = false

    override fun onResume() {
        super.onResume()
        visible = true
        pollRequests()
    }

    override fun onPause() {
        visible = false
        super.onPause()
    }

    /** One approval at a time, while the app is on screen — a prompt needs somebody looking. */
    private fun pollRequests() {
        if (!visible) return
        if (!answering) {
            ApprovalKey.pending(this).firstOrNull()?.let { request ->
                answering = true
                ApprovalKey.answer(this, request) {
                    answering = false
                    refreshSoon()
                }
            }
        }
        output.postDelayed({ pollRequests() }, 1_000)
    }

    private fun makeApprovalKey() {
        Thread {
            val said = try {
                ApprovalKey.ensure(this)
            } catch (e: Exception) {
                // The commonest cause: a key that needs the person's confirmation cannot exist on a
                // device with no screen lock.
                "cannot make an approval key: $e"
            }
            runOnUiThread { output.text = said }
        }.start()
    }

    private fun button(label: String, onClick: () -> Unit) =
        Button(this).apply {
            text = label
            setOnClickListener { onClick() }
        }

    private fun refreshSoon() = output.postDelayed({ refresh() }, 1_500)

    private fun refresh() {
        Thread {
            val status = run(Paths.offload(this).path, "--socket", Paths.socket(this).path, "status")
            val log = Paths.daemonLog(this).takeIf { it.exists() }?.readLines()?.takeLast(25)?.joinToString("\n") ?: "(no log yet)"
            val text = "== offload status\n$status\n\n== daemon.log (last 25 lines)\n$log"
            runOnUiThread { output.text = text.replace(Regex("\u001B\\[[0-9;]*m"), "") }
        }.start()
    }

    private fun run(vararg command: String): String =
        try {
            val p = ProcessBuilder(*command).redirectErrorStream(true).start()
            val text = p.inputStream.bufferedReader().readText()
            p.waitFor()
            text.trim()
        } catch (e: Exception) {
            "cannot run ${command.first()}: $e"
        }

    /** Doze will stop the daemon's network unless the owner exempts the app; this asks them to. */
    private fun askToIgnoreBatteryOptimisation() {
        val power = getSystemService(PowerManager::class.java)
        if (!power.isIgnoringBatteryOptimizations(packageName)) {
            startActivity(
                Intent(Settings.ACTION_REQUEST_IGNORE_BATTERY_OPTIMIZATIONS).setData(Uri.parse("package:$packageName")),
            )
        }
    }
}
