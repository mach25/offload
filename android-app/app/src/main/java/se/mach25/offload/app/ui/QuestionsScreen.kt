package se.mach25.offload.app.ui

import android.app.Activity
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
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.compose.LocalLifecycleOwner
import androidx.lifecycle.repeatOnLifecycle
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import se.mach25.offload.app.host.ApprovalKey
import se.mach25.offload.client.AskRow
import se.mach25.offload.client.Daemon
import se.mach25.offload.client.MobileException
import java.io.File

private suspend fun <T> call(block: () -> T): Result<T> = withContext(Dispatchers.IO) {
    try {
        Result.success(block())
    } catch (e: MobileException.Daemon) {
        Result.failure(Exception(e.reason))
    } catch (e: Exception) {
        Result.failure(e)
    }
}

/**
 * What is waiting for a person: agents stopped mid-tool-call (ADR-0017), and approvals this device
 * holds the hardware key for (ADR-0069 §4).
 *
 * Questions are asked of the daemon every few seconds and never remembered here — one answered on
 * the laptop must leave this screen, not linger as if it were still open.
 */
@Composable
fun QuestionsScreen(daemon: Daemon?, modifier: Modifier = Modifier) {
    val context = LocalContext.current
    var asks by remember { mutableStateOf<List<AskRow>>(emptyList()) }
    var approvals by remember { mutableStateOf<List<File>>(emptyList()) }
    var failure by remember { mutableStateOf<String?>(null) }
    var said by remember { mutableStateOf<String?>(null) }
    var answering by remember { mutableStateOf(false) }
    val scope = rememberCoroutineScope()

    val lifecycle = LocalLifecycleOwner.current.lifecycle

    LaunchedEffect(daemon, lifecycle) {
        // Only while the app is on screen: cancelled in the background, started again on return.
        lifecycle.repeatOnLifecycle(Lifecycle.State.STARTED) {
            while (true) {
                call { daemon!!.asks() }
                    .onSuccess { asks = it; failure = null }
                    .onFailure { failure = it.message }
                approvals = withContext(Dispatchers.IO) { ApprovalKey.pending(context) }
                delay(3_000)
            }
        }
    }

    LazyColumn(
        modifier = modifier.fillMaxSize().padding(horizontal = 16.dp),
        verticalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        item { Spacer(Modifier.size(4.dp)) }
        said?.let { item { Section("Answered") { Text(it) } } }
        failure?.let { item { Section("Not connected") { Text(it) } } }

        if (approvals.isNotEmpty()) {
            item { Heading("Approvals") }
            items(approvals, key = { it.name }) { request ->
                Card(
                    modifier = Modifier.fillMaxWidth(),
                    colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surface),
                ) {
                    Column(Modifier.padding(16.dp)) {
                        Text("A device is waiting to be approved", style = MaterialTheme.typography.titleMedium)
                        Text(
                            "The system prompt shows who and what, read from the certificate itself.",
                            style = MaterialTheme.typography.bodySmall,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                        Spacer(Modifier.size(10.dp))
                        Button(
                            enabled = !answering,
                            onClick = {
                                answering = true
                                ApprovalKey.answer(context as Activity, request) { answering = false }
                            },
                        ) { Text("Review") }
                    }
                }
            }
        }

        item { Heading(if (asks.isEmpty()) "No questions waiting" else "Questions") }
        if (asks.isEmpty() && failure == null) {
            item {
                Text(
                    "An agent submitted with “ask me” stops here before a call it is not allowed to make.",
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
        }
        items(asks, key = { it.run + it.toolUseId }) { ask ->
            AskCard(ask, enabled = !answering) { allow ->
                answering = true
                scope.launch {
                    call { daemon!!.answer(ask.run, ask.toolUseId, allow) }
                        .onSuccess { said = it; asks = asks - ask }
                        .onFailure { failure = it.message }
                    answering = false
                }
            }
        }
        item { Spacer(Modifier.size(16.dp)) }
    }
}

@Composable
private fun AskCard(ask: AskRow, enabled: Boolean, answer: (Boolean) -> Unit) {
    Card(
        modifier = Modifier.fillMaxWidth(),
        colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surface),
    ) {
        Column(Modifier.padding(16.dp)) {
            Text("May it use ${ask.tool}?", style = MaterialTheme.typography.titleMedium)
            Spacer(Modifier.size(6.dp))
            Text(ask.detail, fontFamily = FontFamily.Monospace, style = MaterialTheme.typography.bodyMedium)
            Spacer(Modifier.size(6.dp))
            Text(
                "run ${ask.shortRun}" + (ask.node?.let { " on $it" } ?: "") +
                    " · waiting ${ask.waiting} · decided by nobody answering in ${ask.left}",
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            Spacer(Modifier.size(12.dp))
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                Button(enabled = enabled, onClick = { answer(true) }) { Text("Allow") }
                OutlinedButton(
                    enabled = enabled,
                    onClick = { answer(false) },
                    colors = ButtonDefaults.outlinedButtonColors(contentColor = MaterialTheme.colorScheme.tertiary),
                ) { Text("Deny") }
            }
        }
    }
}

@Composable
private fun Heading(text: String) {
    Text(text, style = MaterialTheme.typography.titleLarge, modifier = Modifier.padding(top = 6.dp))
}
