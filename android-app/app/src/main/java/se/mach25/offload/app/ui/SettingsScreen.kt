package se.mach25.offload.app.ui

import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.TextButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.SegmentedButton
import androidx.compose.material3.SegmentedButtonDefaults
import androidx.compose.material3.SingleChoiceSegmentedButtonRow
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import se.mach25.offload.app.host.ApprovalKey
import se.mach25.offload.app.host.OffloadService
import se.mach25.offload.client.Device
import se.mach25.offload.client.InviteView
import se.mach25.offload.client.MembershipView
import se.mach25.offload.client.MobileException
import se.mach25.offload.client.PolicyView

private suspend fun <T> local(block: () -> T): Result<T> = withContext(Dispatchers.IO) {
    try {
        Result.success(block())
    } catch (e: MobileException.Daemon) {
        Result.failure(Exception(e.reason))
    } catch (e: Exception) {
        Result.failure(e)
    }
}

/** What the owner decides about this device (ADR-0071 slice 4): who it is, which fleet, its
 *  approval key, and the work it accepts. Read from and written to its own files, as the CLI's
 *  `id`, `join` and `fleet` are; a change the daemon reads at start restarts it. */
@Composable
fun SettingsScreen(
    device: Device,
    modifier: Modifier = Modifier,
    link: String? = null,
    linkHandled: () -> Unit = {},
) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    var reload by remember { mutableIntStateOf(0) }
    var nodeId by remember { mutableStateOf("") }
    var membership by remember { mutableStateOf<MembershipView?>(null) }
    var key by remember { mutableStateOf<String?>(null) }
    var policy by remember { mutableStateOf<PolicyView?>(null) }
    var said by remember { mutableStateOf<String?>(null) }
    var failed by remember { mutableStateOf<String?>(null) }

    LaunchedEffect(reload) {
        local { device.nodeId() }.onSuccess { nodeId = it }.onFailure { failed = it.message }
        local { device.membership() }.onSuccess { membership = it }.onFailure { failed = it.message }
        key = withContext(Dispatchers.IO) { device.approvalKey() }
        local { device.policy() }.onSuccess { policy = it }.onFailure { failed = it.message }
    }

    Column(
        modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(horizontal = 16.dp),
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Spacer(Modifier.size(4.dp))
        // A refusal under its own heading: it used to appear under "Done".
        failed?.let { Section("Not done") { Text(it, color = MaterialTheme.colorScheme.tertiary) } }
        said?.let { Section("Done") { Text(it) } }

        Section("This device") {
            Text("Node id — give it to an approver to be invited", style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant)
            Text(nodeId, fontFamily = FontFamily.Monospace, style = MaterialTheme.typography.bodyMedium)
            Spacer(Modifier.size(6.dp))
            OutlinedButton(onClick = {
                val clipboard = context.getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager
                clipboard.setPrimaryClip(ClipData.newPlainText("node id", nodeId))
                said = "Node id copied."
            }) { Text("Copy") }
        }

        val m = membership
        fun join(token: String) {
            scope.launch {
                local { device.join(token) }
                    .onSuccess {
                        // A daemon that started outside a fleet joins the mesh only when it
                        // starts again, so the app starts it again (the CLI says to).
                        OffloadService.restart(context)
                        said = "$it The daemon is restarting to join the mesh."
                        failed = null
                        reload++
                    }
                    .onFailure { failed = it.message; said = null }
            }
        }
        link?.let { JoinConfirm(device, it, onJoin = { token -> linkHandled(); join(token) }, onDismiss = linkHandled) }
        if (m == null) {
            JoinCard(nodeId) { token -> join(token) }
        } else {
            Section("Fleet ${m.fleet.take(8)}") {
                Text("This device is ${m.name}", style = MaterialTheme.typography.bodyMedium)
                Text("Grants: " + m.grants.joinToString(", "), style = MaterialTheme.typography.bodyMedium)
                if (m.approver) {
                    Text(
                        if (m.approvesWithHardwareKey) "An approver, with a key in secure hardware" else "An approver, with its node key",
                        style = MaterialTheme.typography.bodyMedium,
                    )
                }
            }
        }

        Section("Approval key") {
            // Where the key is, as Android verified it — this device's own report, which no peer acts on.
            Text(key?.let { "P-256 $it" } ?: "None on this device.", style = MaterialTheme.typography.bodyMedium)
            m?.let {
                val line = when {
                    it.thisKeyNamed -> "This node approves with it."
                    key != null && it.approvesWithHardwareKey ->
                        "This node's delegation names a different key — one another app made. Naming this one needs the fleet passphrase."
                    key != null -> "Not named by this node's delegation: `offload grant approve --hardware-key` names it (the passphrase)."
                    else -> null
                }
                line?.let { l -> Text(l, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant) }
            }
            if (key == null) {
                Spacer(Modifier.size(8.dp))
                Button(onClick = {
                    scope.launch {
                        local { ApprovalKey.ensure(context) }
                            .onSuccess { said = it; failed = null; reload++ }
                            .onFailure { failed = "Cannot make an approval key: ${it.message}" }
                    }
                }) { Text("Make approval key") }
            }
        }

        BackgroundCard()

        policy?.let { p ->
            PolicyCard(p) { accept, battery, metered ->
                scope.launch {
                    local { device.setPolicy(accept, battery, metered) }
                        .onSuccess {
                            // The daemon reads its config at start.
                            OffloadService.restart(context)
                            said = "Work policy saved. The daemon is restarting with it."
                            failed = null
                            reload++
                        }
                        .onFailure { failed = it.message; said = null }
                }
            }
        }
        Spacer(Modifier.size(16.dp))
    }
}

/**
 * The confirmation a tapped `offload://join?token=…` link opens: which fleet, under what name,
 * with which grants — read from the invitation, before anything is done. A link can come from
 * anybody, so the tap is never the join.
 */
@Composable
private fun JoinConfirm(device: Device, link: String, onJoin: (String) -> Unit, onDismiss: () -> Unit) {
    var invite by remember(link) { mutableStateOf<InviteView?>(null) }
    var problem by remember(link) { mutableStateOf<String?>(null) }
    LaunchedEffect(link) {
        local { device.inspectInvite(link) }.onSuccess { invite = it }.onFailure { problem = it.message }
    }
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text("Join a fleet?") },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
                problem?.let { Text(it, color = MaterialTheme.colorScheme.tertiary) }
                invite?.let { i ->
                    Text("Fleet ${i.fleet.take(8)}", style = MaterialTheme.typography.titleMedium)
                    Text("This device would be ${i.name}, with ${i.grants.joinToString(", ")}.")
                    if (!i.forThisDevice) {
                        Text("This invitation names a different device, so it cannot be taken up here.",
                            color = MaterialTheme.colorScheme.tertiary)
                    }
                    i.currentFleet?.let { f ->
                        Text("This device already belongs to fleet ${f.take(8)}.",
                            color = MaterialTheme.colorScheme.onSurfaceVariant)
                    }
                    Text("Only join a fleet you set up, or one whose owner you trust: its runs would reach this device.",
                        style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                }
            }
        },
        confirmButton = {
            Button(enabled = invite?.forThisDevice == true, onClick = { invite?.let { onJoin(it.token) } }) { Text("Join") }
        },
        dismissButton = { TextButton(onClick = onDismiss) { Text("Not now") } },
    )
}

@Composable
private fun JoinCard(nodeId: String, join: (String) -> Unit) {
    var token by remember { mutableStateOf("") }
    Section("Join a fleet") {
        // The id is on screen, so the instruction spells it rather than bracketing a placeholder.
        Text("Paste the invitation or link an approver made with `offload invite ${nodeId.take(12)}…`, or open the link on this device.",
            style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        Spacer(Modifier.size(6.dp))
        OutlinedTextField(
            token, { token = it }, label = { Text("offload-invite-1.…") }, maxLines = 3,
            modifier = Modifier.fillMaxWidth(),
            keyboardOptions = KeyboardOptions(autoCorrectEnabled = false, keyboardType = KeyboardType.Uri),
        )
        Spacer(Modifier.size(8.dp))
        Button(enabled = token.isNotBlank(), onClick = { join(token.trim()) }) { Text("Join") }
    }
}

/** Keep the daemon running after the app leaves the screen (ADR-0079's owner exception). */
@Composable
private fun BackgroundCard() {
    val context = LocalContext.current
    var keep by remember { mutableStateOf(se.mach25.offload.app.host.Background.keep(context)) }
    Section("In the background") {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Switch(checked = keep, onCheckedChange = {
                keep = it
                se.mach25.offload.app.host.Background.setKeep(context, it)
            })
            Spacer(Modifier.size(10.dp))
            Text("Keep running when the app is closed", style = MaterialTheme.typography.bodyMedium)
        }
        Text(
            if (keep) "This device stays in your fleet and hears news at once, at some cost in battery. " +
                "It still probes less while the screen is off. If Android stops the app, opening it starts it again."
            else "Off: Offload stops about a minute and a half after you leave the app, unless it is running " +
                "something. News waits for you and arrives when you open it.",
            style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
    }
}

@Composable
private fun PolicyCard(current: PolicyView, save: (String?, UByte?, Boolean?) -> Unit) {
    // "default" is the device class's own answer — for a phone, while charging.
    val choices = listOf(null to "Default", "never" to "Never", "when_charging" to "Charging", "always" to "Always")
    var accept by remember(current) { mutableStateOf(current.accept) }
    var battery by remember(current) { mutableStateOf(current.minBatteryPercent?.toString() ?: "") }
    var metered by remember(current) { mutableStateOf(current.allowMetered ?: false) }
    Section("Work policy") {
        Text("When this device takes work", style = MaterialTheme.typography.bodySmall,
            color = MaterialTheme.colorScheme.onSurfaceVariant)
        SingleChoiceSegmentedButtonRow(Modifier.fillMaxWidth()) {
            choices.forEachIndexed { i, (value, label) ->
                SegmentedButton(selected = accept == value, onClick = { accept = value },
                    shape = SegmentedButtonDefaults.itemShape(i, choices.size),
                    // No check mark: the fill says which is chosen, and the mark's width broke
                    // "Charging" across two lines on the phone.
                    icon = {}) { Text(label, maxLines = 1, softWrap = false) }
            }
        }
        Spacer(Modifier.size(10.dp))
        OutlinedTextField(
            battery, { battery = it.filter(Char::isDigit).take(3) },
            label = { Text("Battery floor, % (empty: none)") }, singleLine = true,
            keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Number),
        )
        Spacer(Modifier.size(6.dp))
        Row(verticalAlignment = Alignment.CenterVertically) {
            Switch(checked = metered, onCheckedChange = { metered = it })
            Spacer(Modifier.size(10.dp))
            Text("Work on metered data", style = MaterialTheme.typography.bodyMedium)
        }
        Spacer(Modifier.size(10.dp))
        Button(onClick = {
            save(accept, battery.toIntOrNull()?.coerceIn(0, 100)?.toUByte(), if (metered) true else null)
        }) { Text("Save") }
    }
}
