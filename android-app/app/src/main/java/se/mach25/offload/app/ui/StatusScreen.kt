package se.mach25.offload.app.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.background
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.compose.LocalLifecycleOwner
import androidx.lifecycle.repeatOnLifecycle
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.withContext
import se.mach25.offload.client.Daemon
import se.mach25.offload.client.LightView
import se.mach25.offload.client.MobileException
import se.mach25.offload.client.NodeRow
import se.mach25.offload.client.NodesView
import se.mach25.offload.client.PeerStatus
import se.mach25.offload.client.StatusView

/** What the screen last heard, or why it heard nothing. */
private sealed interface Reading {
    data object Waiting : Reading
    data class Got(val status: StatusView, val nodes: NodesView?) : Reading
    data class Failed(val message: String) : Reading
}

/**
 * This device and its fleet — `offload status` and `offload nodes`, polled.
 *
 * Every sentence about a decision (why it refuses work, the fleet's notes) is the daemon's own,
 * shown as it wrote it (ADR-0071 §2). This screen lays out; it does not judge.
 */
@Composable
fun StatusScreen(daemon: Daemon?, modifier: Modifier = Modifier) {
    var reading by remember { mutableStateOf<Reading>(Reading.Waiting) }
    val lifecycle = LocalLifecycleOwner.current.lifecycle
    LaunchedEffect(daemon, lifecycle) {
        // Only while the app is on screen: cancelled in the background, started again on return.
        lifecycle.repeatOnLifecycle(Lifecycle.State.STARTED) {
            while (true) {
                reading = withContext(Dispatchers.IO) {
                    try {
                        val status = daemon?.status() ?: throw IllegalStateException("no client")
                        Reading.Got(status, runCatching { daemon.nodes() }.getOrNull())
                    } catch (e: MobileException.Daemon) {
                        Reading.Failed(e.reason)
                    } catch (e: Exception) {
                        Reading.Failed(e.toString())
                    }
                }
                delay(3_000)
            }
        }
    }

    LazyColumn(
        modifier = modifier.fillMaxSize().padding(horizontal = 16.dp),
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        item { Spacer(Modifier.size(4.dp)) }
        when (val r = reading) {
            Reading.Waiting -> item { Text("Asking the daemon…") }
            is Reading.Failed -> item {
                Section("Not connected") {
                    // The client's own sentence: "no daemon at …", "nothing is listening", …
                    Text(r.message, style = MaterialTheme.typography.bodyMedium)
                }
            }
            is Reading.Got -> {
                item { ThisNode(r.status) }
                r.status.fleet?.let { fleet ->
                    item {
                        Section("Fleet ${fleet.fleet}") {
                            Line(
                                if (fleet.revokedHere) "This device has been revoked"
                                else "${fleet.membersMet} met · ${fleet.approversMet} approver(s) · certificate ${fleet.certDays} more days",
                            )
                            fleet.expiring.forEach { Line("Renewing soon: $it") }
                            fleet.reapprovalDue.forEach {
                                Line("${it.name}: approval runs out in ${it.days} day(s)" + if (it.decided) " — re-approval decided" else "")
                            }
                        }
                    }
                    items(fleet.notes) { note -> Note(note) }
                }
                r.nodes?.let { nodes ->
                    item {
                        Section("Nodes") {
                            if (!nodes.inMesh) Line("This node is not in a mesh.")
                            nodes.nodes.forEach { NodeLine(it) }
                        }
                    }
                }
            }
        }
        item { Spacer(Modifier.size(8.dp)) }
    }
}

@Composable
private fun ThisNode(s: StatusView) {
    Section(s.name, subtitle = "${s.deviceClass} · ${s.nodeId.take(12)}") {
        // The daemon's decision, in its words: null is "yes".
        val accepting = s.refusal
        Text(
            if (accepting == null) "Accepting work" else "Not accepting work",
            style = MaterialTheme.typography.titleMedium,
            color = if (accepting == null) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.tertiary,
        )
        accepting?.let { Text(it, style = MaterialTheme.typography.bodyMedium) }
        when (val light = s.light) {
            null -> {}
            is LightView.Accepted -> Line("…but light work: yes")
            is LightView.Refused -> Line("…and light work: no — ${light.reason}")
        }
        Spacer(Modifier.size(8.dp))
        Line("Runs ${s.running}/${s.maxConcurrent}" + if (s.asks > 0u) " · ${s.asks} waiting for an answer" else "")
        // Not reported is not idle, and says so.
        Line("CPU " + (s.cpuPercent?.let { "$it%" } ?: "not reported"))
        s.thermal?.let { Line("Thermal $it") }
        Line(
            "Agent " + when {
                s.agent == null -> "none — nothing at ${s.agentBinary}"
                s.agentAuthenticated -> "${s.agent}, authenticated"
                else -> "${s.agent}, NOT authenticated"
            },
        )
        s.approvalKey?.let { Line("Approval key $it") }
    }
}

@Composable
private fun NodeLine(n: NodeRow) {
    Row(
        modifier = Modifier.fillMaxWidth().padding(vertical = 4.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        val colour = when (n.status) {
            PeerStatus.ALIVE -> Color(0xFF22C55E)
            PeerStatus.SUSPECT -> Color(0xFFF59E0B)
            PeerStatus.DRAINING -> Color(0xFF60A5FA)
            PeerStatus.DEAD, PeerStatus.DEPARTED -> Color(0xFFEF4444)
        }
        Spacer(Modifier.size(10.dp).background(colour, CircleShape))
        Spacer(Modifier.size(10.dp))
        Column(Modifier.weight(1f)) {
            Text(
                n.name + if (n.thisNode) "  (this device)" else "",
                fontWeight = if (n.thisNode) FontWeight.SemiBold else FontWeight.Normal,
            )
            Text(
                "${n.status.name.lowercase()} · ${n.deviceClass} · ${n.cpuCores} cores · ${n.memoryMb / 1024u} GB" +
                    (n.agent?.let { " · $it" } ?: ""),
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
        Text(n.shortId, style = MaterialTheme.typography.labelSmall, fontFamily = FontFamily.Monospace)
    }
}

@Composable
fun Section(title: String, subtitle: String? = null, content: @Composable () -> Unit) {
    Card(
        modifier = Modifier.fillMaxWidth(),
        colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surface),
    ) {
        Column(Modifier.padding(16.dp)) {
            Text(title, style = MaterialTheme.typography.titleLarge)
            subtitle?.let {
                Text(it, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
            }
            Spacer(Modifier.size(8.dp))
            content()
        }
    }
}

@Composable
private fun Line(text: String) {
    Text(text, style = MaterialTheme.typography.bodyMedium, modifier = Modifier.padding(vertical = 2.dp))
}

@Composable
private fun Note(text: String) {
    Card(
        modifier = Modifier.fillMaxWidth(),
        colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surfaceVariant),
    ) {
        Text(text, style = MaterialTheme.typography.bodyMedium, modifier = Modifier.padding(16.dp))
    }
}

@Composable
fun NotYet(d: Destination, modifier: Modifier = Modifier) {
    Column(modifier.fillMaxSize().padding(16.dp)) {
        Section(d.label) { Text("Coming in a later slice of ADR-0071.") }
    }
}
