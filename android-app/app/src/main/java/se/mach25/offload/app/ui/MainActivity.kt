package se.mach25.offload.app.ui

import android.Manifest
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.SystemBarStyle
import androidx.activity.enableEdgeToEdge
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.consumeWindowInsets
import androidx.compose.foundation.layout.isImeVisible
import androidx.compose.foundation.layout.padding
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Home
import androidx.compose.material.icons.filled.List
import androidx.compose.material.icons.filled.Notifications
import androidx.compose.material.icons.filled.Settings
import androidx.compose.material3.Badge
import androidx.compose.material3.BadgedBox
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.runtime.LaunchedEffect
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.compose.LocalLifecycleOwner
import androidx.lifecycle.repeatOnLifecycle
import androidx.compose.runtime.remember
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.withContext
import androidx.compose.material3.NavigationBar
import androidx.compose.material3.NavigationBarItem
import androidx.compose.material3.NavigationBarItemDefaults
import androidx.compose.ui.graphics.Color
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.vector.ImageVector
import se.mach25.offload.app.host.OffloadService
import se.mach25.offload.app.host.Paths
import se.mach25.offload.client.Daemon
import se.mach25.offload.client.Device

/**
 * The four destinations of ADR-0071 §4. Runs first, since starting and following runs is what the
 * app is for; the fleet's status second (the owner, on the phone: "I'm not really interested in that").
 */
enum class Destination(val label: String, val icon: ImageVector) {
    Runs("Runs", Icons.Filled.Home),
    Fleet("Fleet", Icons.Filled.List),
    Questions("Questions", Icons.Filled.Notifications),
    Settings("Settings", Icons.Filled.Settings),
}

@OptIn(ExperimentalLayoutApi::class)
class MainActivity : ComponentActivity() {
    /** An `offload://join?token=…` link the app was opened with, waiting for confirmation. */
    private val link = mutableStateOf<String?>(null)

    /** A tab a notification asked to open. */
    private val opening = mutableStateOf<String?>(null)

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        intent.data?.toString()?.let { link.value = it }
        intent.getStringExtra("open")?.let { opening.value = it }
    }

    // ADR-0079: the daemon runs while the app is on screen, and stops itself a little after it
    // leaves unless a run is held here. So every return to the screen starts it, not only onCreate.
    override fun onStart() {
        super.onStart()
        OffloadService.visible = true
        startForegroundService(Intent(this, OffloadService::class.java))
    }

    override fun onStop() {
        OffloadService.visible = false
        OffloadService.lastSeen = System.currentTimeMillis()
        super.onStop()
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        intent?.data?.toString()?.let { link.value = it }
        intent?.getStringExtra("open")?.let { opening.value = it }
        super.onCreate(savedInstanceState)
        // The app is always the logo's navy, so the system bars' icons are always light: left to the
        // system theme they were dark-on-navy in light mode, the clock all but invisible.
        enableEdgeToEdge(
            statusBarStyle = SystemBarStyle.dark(android.graphics.Color.TRANSPARENT),
            navigationBarStyle = SystemBarStyle.dark(android.graphics.Color.TRANSPARENT),
        )
        if (checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED) {
            requestPermissions(arrayOf(Manifest.permission.POST_NOTIFICATIONS), 1)
        }
        // The app hosts its node: opening it starts the daemon, as the harness does (ADR-0071 §3).
        startForegroundService(Intent(this, OffloadService::class.java))
        // One client for every screen: it holds a small runtime, and each screen asking the same
        // daemon through its own would be four of them.
        val daemon = runCatching { Daemon(Paths.socket(this).path) }.getOrNull()
        val device = Device(Paths.stateDir(this).path, Paths.config(this).path)

        setContent {
            OffloadTheme {
                var current by rememberSaveable { mutableStateOf(Destination.Runs) }
                // A join link lands on Settings, where it is confirmed or dismissed.
                val pending = link.value
                LaunchedEffect(pending) { if (pending != null) current = Destination.Settings }
                val open = opening.value
                LaunchedEffect(open) {
                    when (open) {
                        "questions" -> current = Destination.Questions
                        "runs" -> current = Destination.Runs
                    }
                    opening.value = null
                }
                // Questions waiting, for the tab's badge: the daemon's count, asked every few seconds.
                var waiting by remember { mutableStateOf(0) }
                val lifecycle = LocalLifecycleOwner.current.lifecycle
                LaunchedEffect(daemon, lifecycle) {
                    // Only while the app is on screen: cancelled in the background, started again on return.
                    lifecycle.repeatOnLifecycle(Lifecycle.State.STARTED) {
                        while (true) {
                            waiting = withContext(Dispatchers.IO) {
                                runCatching { daemon?.asks()?.size ?: 0 }.getOrDefault(0)
                            }
                            delay(3_000)
                        }
                    }
                }
                Scaffold(
                    containerColor = Color.Transparent,
                    // Text outside a card takes the content colour, and with a transparent container
                    // there was none: headings drew black on the navy.
                    contentColor = MaterialTheme.colorScheme.onBackground,
                    topBar = { BrandBar() },
                    // Hidden while typing: behind the keyboard it still took its height, and the
                    // composer floated that far above the keys.
                    bottomBar = {
                        if (!WindowInsets.isImeVisible) NavigationBar(containerColor = Brand.Ink.copy(alpha = 0.92f)) {
                            Destination.entries.forEach { d ->
                                NavigationBarItem(
                                    selected = current == d,
                                    onClick = { current = d },
                                    icon = {
                                        if (d == Destination.Questions && waiting > 0) {
                                            BadgedBox(badge = { Badge(containerColor = Brand.Orange) { Text("$waiting") } }) {
                                                Icon(d.icon, contentDescription = d.label)
                                            }
                                        } else {
                                            Icon(d.icon, contentDescription = d.label)
                                        }
                                    },
                                    label = { Text(d.label) },
                                    colors = NavigationBarItemDefaults.colors(
                                        selectedIconColor = Brand.Ink,
                                        selectedTextColor = Brand.Cyan,
                                        indicatorColor = Brand.Cyan,
                                        unselectedIconColor = Color(0xFF8A93B8),
                                        unselectedTextColor = Color(0xFF8A93B8),
                                    ),
                                )
                            }
                        }
                    },
                ) { padding ->
                    // Consumed, so the composer's keyboard padding does not count the bar twice.
                    val modifier = Modifier.padding(padding).consumeWindowInsets(padding)
                    when (current) {
                        Destination.Runs -> HomeScreen(daemon, waiting, { current = Destination.Questions }, modifier)
                        Destination.Fleet -> StatusScreen(daemon, modifier)
                        Destination.Questions -> QuestionsScreen(daemon, modifier)
                        Destination.Settings -> SettingsScreen(device, modifier, pending) { link.value = null }
                    }
                }
            }
        }
    }
}
