package se.mach25.offload.app.ui

import androidx.compose.foundation.clickable
import androidx.compose.material3.SingleChoiceSegmentedButtonRow
import androidx.compose.material3.SegmentedButtonDefaults
import androidx.compose.material3.SegmentedButton
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.foundation.verticalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.Send
import androidx.compose.material.icons.filled.ArrowDropDown
import androidx.compose.material.icons.filled.Check
import androidx.compose.material.icons.filled.MoreVert
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.AssistChip
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FilledIconButton
import androidx.compose.material3.FilterChip
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButtonDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Surface
import androidx.compose.material3.Switch
import androidx.compose.material3.SwipeToDismissBox
import androidx.compose.material3.SwipeToDismissBoxValue
import androidx.compose.material3.rememberSwipeToDismissBoxState
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TextField
import androidx.compose.material3.TextFieldDefaults
import androidx.compose.material3.rememberModalBottomSheetState
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
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import se.mach25.offload.client.Daemon
import se.mach25.offload.client.MobileException
import se.mach25.offload.client.RunRow
import se.mach25.offload.client.RunsView
import se.mach25.offload.client.Submitted

/**
 * Service names, URLs and model ids are typed exactly: the keyboard's autocorrect turned `tick`
 * into `tickets` on the emulator, which the fleet would have refused as a service nobody offers.
 */
private val Verbatim = KeyboardOptions(autoCorrectEnabled = false, capitalization = KeyboardCapitalization.None,
    keyboardType = KeyboardType.Uri)

/** Run a blocking client call off the main thread, turning its failure into the daemon's words. */
private suspend fun <T> ask(block: () -> T): Result<T> = withContext(Dispatchers.IO) {
    try {
        Result.success(block())
    } catch (e: MobileException.Daemon) {
        Result.failure(Exception(e.reason))
    } catch (e: Exception) {
        Result.failure(e)
    }
}

/** How many finished runs the home list keeps below the active ones. */
private const val EARLIER = 20

/**
 * The app's first screen, and its main job: tell the fleet what to do, and see what it is doing.
 *
 * A composer at the bottom, as in a messaging app: the prompt first, where it works and the rest
 * as chips, send. The runs above it, active first. The owner found the old dialog behind a `+` on
 * the second tab "feels like a secondary function", for what is the primary one.
 *
 * Which runs count as active is `offload_node::render::is_finished`, carried on each row, so this
 * list and `offload ps` agree about what is still going (ADR-0071 §2).
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun HomeScreen(daemon: Daemon?, waiting: Int, onQuestions: () -> Unit, modifier: Modifier = Modifier) {
    var view by remember { mutableStateOf<RunsView?>(null) }
    var failure by remember { mutableStateOf<String?>(null) }
    var open by remember { mutableStateOf<RunRow?>(null) }
    var message by remember { mutableStateOf<String?>(null) }
    val context = LocalContext.current
    val archive = remember { Archive(context) }
    var archived by remember { mutableStateOf(archive.ids()) }
    var showArchived by remember { mutableStateOf(false) }
    var undo by remember { mutableStateOf<RunRow?>(null) }
    // A pending run taken back to be edited: its prompt and repository, for the composer.
    var prefill by remember { mutableStateOf<Pair<String, String>?>(null) }

    val lifecycle = LocalLifecycleOwner.current.lifecycle

    LaunchedEffect(daemon, lifecycle) {
        // Only while the app is on screen: cancelled in the background, started again on return.
        lifecycle.repeatOnLifecycle(Lifecycle.State.STARTED) {
            while (true) {
                ask { daemon!!.runs() }
                    .onSuccess {
                        view = it
                        failure = null
                        archive.keepOnly(it.runs.map { run -> run.id }.toSet())
                    }
                    .onFailure { failure = it.message }
                delay(3_000)
            }
        }
    }

    Column(modifier.fillMaxSize().imePadding()) {
        val all = view?.runs.orEmpty()
        val active = all.filter { !it.finished }.sortedByDescending { it.startedAtUnix }
        // Not a rule's or a schedule's successful task: they fire every minute or so, and a list of
        // their successes buried the runs somebody started (seen on the walk fleet). A task a person
        // started stays, since the finished row is where its output is (session ninety-four).
        val finished = all.filter { it.finished && !(it.task && it.byRule && it.state != "failed") }
            .sortedByDescending { it.startedAtUnix }
        // Put away by hand, or by itself a day after it started: the list cleans itself up.
        val now = System.currentTimeMillis() / 1000
        fun away(run: RunRow) =
            run.id in archived || now - run.startedAtUnix.toLong() > Archive.AFTER_SECONDS
        val earlier = finished.filterNot(::away).take(EARLIER)
        val putAway = finished.filter(::away)
        LazyColumn(
            modifier = Modifier.weight(1f).fillMaxWidth().padding(horizontal = 16.dp),
            verticalArrangement = Arrangement.spacedBy(8.dp),
            reverseLayout = false,
        ) {
            if (waiting > 0) {
                item { QuestionsBanner(waiting, onQuestions) }
            }
            message?.let { m -> item { Note(m) { message = null } } }
            undo?.let { run ->
                item {
                    Note("Archived “${run.work.take(40)}” — tap to undo") {
                        archive.restore(run.id)
                        archived = archive.ids()
                        undo = null
                    }
                }
            }
            failure?.let { f -> item { Note("Not connected to this device's node: $f") {} } }
            if (view != null && all.isEmpty()) {
                item {
                    Guide()
                }
            }
            items(active, key = { it.id }) { run -> RunItem(run) { open = run } }
            if (earlier.isNotEmpty()) {
                item {
                    Text("Earlier", style = MaterialTheme.typography.labelLarge,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        modifier = Modifier.padding(top = 10.dp, bottom = 2.dp))
                }
                items(earlier, key = { it.id }) { run ->
                    Swipeable(onArchive = {
                        archive.put(run.id)
                        archived = archive.ids()
                        undo = run
                    }) { RunItem(run) { open = run } }
                }
            }
            if (putAway.isNotEmpty()) {
                item {
                    TextButton(onClick = { showArchived = !showArchived }) {
                        Text(if (showArchived) "Hide archived" else "Archived (${putAway.size})")
                    }
                }
                if (showArchived) {
                    items(putAway, key = { "a" + it.id }) { run -> RunItem(run) { open = run } }
                }
            }
            // No note about resuming: this app has no Resume, so the node's sentence about why it
            // would refuse one explained a button that is not here, and on a phone it told the owner
            // to run `offload grant host-runs` on a device they chose not to host on (session
            // ninety-four). A run's way forward in the app is Continue.
            item { Spacer(Modifier.size(8.dp)) }
        }
        Composer(daemon, prefill, onPrefilled = { prefill = null }) { said -> message = said }
    }

    open?.let { opened ->
        // The run as the latest listing has it, not as it was when tapped: a sheet opened on a
        // running run went on saying `running`, with Cancel, after the run had finished.
        val run = view?.runs?.find { it.id == opened.id } ?: opened
        // Fully open: half-open, the log's newest lines — where it follows to — sat below the screen.
        ModalBottomSheet(
            onDismissRequest = { open = null },
            containerColor = Brand.Card,
            sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true),
        ) {
            RunDetail(daemon, run, onEdit = { taken ->
                prefill = taken.work to taken.repo
                message = "Took ${taken.shortId} back — change it below and send"
                open = null
            }) { said ->
                message = said
                open = null
            }
        }
    }
}

/** Swipe a finished run either way to put it away. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun Swipeable(onArchive: () -> Unit, content: @Composable () -> Unit) {
    val state = rememberSwipeToDismissBoxState(confirmValueChange = { value ->
        if (value != SwipeToDismissBoxValue.Settled) onArchive()
        value != SwipeToDismissBoxValue.Settled
    })
    SwipeToDismissBox(
        state = state,
        backgroundContent = {
            Box(Modifier.fillMaxSize().padding(horizontal = 20.dp), contentAlignment = Alignment.CenterEnd) {
                Text("Archive", color = MaterialTheme.colorScheme.onSurfaceVariant)
            }
        },
    ) { content() }
}

/**
 * What the composer is for, where a first-time person looks: the empty list. The owner, on the
 * tablet: "how do I even use the What should it do? thing? … you get no help" (session ninety-two).
 */
@Composable
private fun Guide() {
    Column(Modifier.padding(vertical = 20.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Text("Give your fleet something to do", style = MaterialTheme.typography.titleMedium)
        Text("Ask the agent: write what you want done, in your own words, and send. A device in your " +
            "fleet with an agent (Claude Code) does the work, and the answer appears here when it is done.",
            style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
        Text("For example: “Summarise the README”, with a repository chosen under Empty folder, or " +
            "“Write a short plan for moving house” in an empty folder.",
            style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
        Text("Run a program: one of the programs your devices offer, like a check or a script, " +
            "without the agent. They are listed when you choose it.",
            style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
        Text("The line above the text box says which devices can do it right now.",
            style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
    }
}

@Composable
private fun QuestionsBanner(waiting: Int, onQuestions: () -> Unit) {
    Card(
        modifier = Modifier.fillMaxWidth().clickable(onClick = onQuestions),
        colors = CardDefaults.cardColors(containerColor = Brand.Orange.copy(alpha = 0.18f)),
    ) {
        Row(Modifier.padding(14.dp), verticalAlignment = Alignment.CenterVertically) {
            Text("?", color = Brand.Orange, style = MaterialTheme.typography.titleLarge)
            Spacer(Modifier.width(12.dp))
            Text(
                if (waiting == 1) "An agent is waiting for your answer" else "$waiting agents are waiting for your answer",
                style = MaterialTheme.typography.bodyLarge, modifier = Modifier.weight(1f),
            )
            Text("Answer", color = Brand.Orange, style = MaterialTheme.typography.labelLarge)
        }
    }
}

/** The state as a glyph and a colour, which is what a list is scanned for. */
private fun glyph(state: String): Pair<String, Color> = when (state) {
    "running" -> "●" to Color(0xFF22C55E)
    "completed" -> "✓" to Brand.Cyan
    "failed" -> "✗" to Color(0xFFEF4444)
    "cancelled" -> "⊘" to Color(0xFF8A93B8)
    else -> "◌" to Color(0xFFF59E0B)
}

/** How long ago, in the unit a person would say. */
private fun ago(unix: Long): String {
    if (unix <= 0L) return ""
    val s = (System.currentTimeMillis() / 1000 - unix).coerceAtLeast(0)
    return when {
        s < 60 -> "just now"
        s < 3_600 -> "${s / 60}m ago"
        s < 86_400 -> "${s / 3_600}h ago"
        else -> "${s / 86_400}d ago"
    }
}

/** Where a run works, in the words the composer used for it. */
private fun where(run: RunRow): String = when {
    run.task -> "program"
    run.continues != null -> "continues ${run.continues}"
    run.repo == "scratch:" || run.repo.isBlank() -> "empty folder"
    else -> Remembered.short(run.repo)
}

@Composable
private fun RunItem(run: RunRow, onOpen: () -> Unit) {
    val (mark, colour) = glyph(run.state)
    Card(
        modifier = Modifier.fillMaxWidth().clickable(onClick = onOpen),
        colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surface),
    ) {
        Row(Modifier.padding(horizontal = 14.dp, vertical = 10.dp), verticalAlignment = Alignment.Top) {
            Text(mark, color = colour, fontSize = 18.sp, modifier = Modifier.width(24.dp))
            Column(Modifier.weight(1f)) {
                Text(run.work, style = MaterialTheme.typography.bodyLarge, maxLines = 2, overflow = TextOverflow.Ellipsis)
                val facts = buildList {
                    add(run.state)
                    // Who did the work, and with what, which the owner asked to see (session ninety-four).
                    run.host?.let { add("on $it") }
                    if (!run.task) run.model?.let { add(it) }
                    add(where(run))
                    if (!run.task && run.turns > 0u) add("${run.turns} turn${if (run.turns == 1u) "" else "s"}")
                    ago(run.startedAtUnix.toLong()).takeIf { it.isNotEmpty() }?.let { add(it) }
                    run.due?.let { add("due $it") }
                }
                Text(facts.joinToString(" · "), style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant)
                run.held?.let { Text(it, style = MaterialTheme.typography.bodySmall) }
                run.error?.let {
                    Text(it, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.tertiary,
                        maxLines = 2, overflow = TextOverflow.Ellipsis)
                }
            }
        }
    }
}

@Composable
private fun StateChip(state: String) {
    val (_, colour) = glyph(state)
    AssistChip(onClick = {}, label = { Text(state, color = colour) })
}

/**
 * Where a run starts, and the app's main job. **The choice is in plain sight**: ask the agent, or
 * run one of the programs the fleet offers (a "task", ADR-0019). The owner queued an email summary
 * as an agent run believing it was a task, and never saw the ⋮ that held the other option (session
 * ninety-two). Everything that shapes a run is a chip in one visible row, and the programs are
 * listed from what the fleet's devices offer rather than typed from memory.
 *
 * **It says who can do it** before anything is sent: the devices with an agent, or the ones
 * offering each program, read from `offload nodes`. What a device *has*, not a promise it will
 * take the run: the fleet still decides, and a refusal comes back in its words. "Wait for a
 * device" is then one tap, since the refusal is what names `--queue`.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun Composer(
    daemon: Daemon?,
    prefill: Pair<String, String>?,
    onPrefilled: () -> Unit,
    done: (String) -> Unit,
) {
    val context = LocalContext.current
    val remembered = remember { Remembered(context) }
    var program by remember { mutableStateOf(false) }
    var prompt by remember { mutableStateOf("") }
    var repo by remember { mutableStateOf("") }
    var askMe by remember { mutableStateOf(false) }
    var model by remember { mutableStateOf(remembered.model()) }
    var queue by remember { mutableStateOf(false) }
    var service by remember { mutableStateOf<String?>(null) }
    var args by remember { mutableStateOf("") }
    // `null` until the daemon has answered: unknown is not none, and "no device has an agent" in
    // orange, shown for the second before the first answer, was a false alarm (emulator).
    var nodes by remember { mutableStateOf<List<se.mach25.offload.client.NodeRow>?>(null) }
    var refused by remember { mutableStateOf<String?>(null) }
    var busy by remember { mutableStateOf(false) }
    var picking by remember { mutableStateOf(false) }
    var choosingModel by remember { mutableStateOf(false) }
    var recent by remember { mutableStateOf(remembered.repos()) }
    val scope = rememberCoroutineScope()
    LaunchedEffect(prefill) {
        prefill?.let { (text, where) ->
            program = false
            prompt = text
            repo = if (where == "scratch:") "" else where
            onPrefilled()
        }
    }
    // What the fleet's devices have, from the nodes this one can see now.
    val lifecycle = LocalLifecycleOwner.current.lifecycle
    LaunchedEffect(daemon, lifecycle) {
        // Only while the app is on screen: cancelled in the background, started again on return.
        lifecycle.repeatOnLifecycle(Lifecycle.State.STARTED) {
            while (true) {
                ask { daemon!!.nodes() }.onSuccess { view ->
                    nodes = view.nodes.filter { it.status != se.mach25.offload.client.PeerStatus.DEAD }
                }
                delay(15_000)
            }
        }
    }
    val known = nodes.orEmpty()
    val withAgent = known.filter { n -> n.agent?.let { !it.contains("no auth") } == true }
    // Hosting is decided on each node's certificate (`host-runs`), which the listing now carries:
    // the Mac had an agent and no permission to host, and was listed as if it could take the run.
    val canTake = withAgent.filter { it.mayHost == true }.map { it.name }
    val barred = withAgent.filter { it.mayHost == false }.map { it.name }
    val unsure = withAgent.filter { it.mayHost == null }.map { it.name }
    val agents = canTake + unsure
    val programs = known.flatMap { it.programs }.distinct().sorted()
    val offering = { name: String -> known.filter { name in it.programs }.map { it.name } }
    // Models as the devices that could take the run list them (ADR-0080), each once, in the order
    // the first such device lists it, with who offers it. `default` is not a model to pick: it is
    // what no choice means, so it describes the Default row instead.
    val hosts = withAgent.filter { it.mayHost != false }
    val offered = buildList<Pair<se.mach25.offload.client.ModelRow, List<String>>> {
        hosts.flatMap { it.models }.map { it.value }.distinct().filter { it != "default" }.forEach { value ->
            val first = hosts.firstNotNullOf { h -> h.models.firstOrNull { it.value == value } }
            add(first to hosts.filter { h -> h.models.any { it.value == value } }.map { it.name })
        }
    }
    val defaultAbout = hosts.firstNotNullOfOrNull { h -> h.models.firstOrNull { it.value == "default" }?.about }
    val modelLabel = offered.firstOrNull { it.first.value == model }?.first?.name ?: model

    fun send(pending: Boolean) {
        busy = true
        scope.launch {
            val result = if (program) {
                ask { daemon!!.submitTask(service!!, args.split(' ').filter { it.isNotBlank() }, pending) }
            } else {
                ask { daemon!!.submitAgent(repo, prompt, model.ifBlank { null }, pending, askMe) }
            }
            result
                .onSuccess {
                    if (program) {
                        args = ""
                    } else {
                        remembered.used(repo, model)
                        recent = remembered.repos()
                        prompt = ""
                    }
                    refused = null
                    done(said(it))
                }
                .onFailure { refused = it.message }
            busy = false
        }
    }

    Surface(color = Brand.Ink.copy(alpha = 0.96f), shape = RoundedCornerShape(topStart = 20.dp, topEnd = 20.dp)) {
        Column(
            Modifier.fillMaxWidth().navigationBarsPadding().padding(start = 12.dp, end = 12.dp, top = 10.dp, bottom = 10.dp),
            verticalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            refused?.let { reason ->
                Column(Modifier.fillMaxWidth().padding(horizontal = 4.dp)) {
                    Text("Nobody took it", style = MaterialTheme.typography.labelLarge, color = Brand.Orange)
                    Text(reason, style = MaterialTheme.typography.bodySmall, maxLines = 6, overflow = TextOverflow.Ellipsis)
                    Row {
                        TextButton(enabled = !busy, onClick = { send(pending = true) }) { Text("Wait for a device") }
                        TextButton(onClick = { refused = null }) { Text("Dismiss") }
                    }
                }
            }
            SingleChoiceSegmentedButtonRow(Modifier.fillMaxWidth()) {
                SegmentedButton(selected = !program, onClick = { program = false }, icon = {},
                    shape = SegmentedButtonDefaults.itemShape(0, 2)) { Text("Ask the agent", maxLines = 1) }
                SegmentedButton(selected = program, onClick = { program = true }, icon = {},
                    shape = SegmentedButtonDefaults.itemShape(1, 2)) { Text("Run a program", maxLines = 1) }
            }
            // Nothing offered is the whole answer: no arguments box for a program that cannot be picked.
            val nothingToRun = program && nodes != null && programs.isEmpty()
            // Who can do it, before it is sent.
            val who = when {
                nodes == null -> ""
                program && service != null -> "Offered by: " + offering(service!!).joinToString(", ")
                program && programs.isEmpty() -> "No device in your fleet offers a program right now."
                program -> "Pick one of the programs your devices offer."
                agents.isEmpty() && barred.isNotEmpty() ->
                    barred.joinToString(", ") + " has an agent but isn't allowed to host runs, so nothing can do this yet. It can wait."
                agents.isEmpty() -> "No device in your fleet has a working agent, so nothing can do this yet. It can wait for one."
                // Can, not will: which one takes it is the fleet's decision at the time, and its
                // answer comes back when this is sent.
                canTake.isNotEmpty() -> "Can take agent runs: " + canTake.joinToString(", ") +
                    (if (barred.isNotEmpty()) " · " + barred.joinToString(", ") + " has an agent but isn't allowed to host" else "")
                else -> "Devices with an agent: " + agents.joinToString(", ") + " — the fleet picks one when you send"
            }
            if (who.isNotEmpty()) Text(who, style = MaterialTheme.typography.bodySmall,
                color = if ((!program && canTake.isEmpty() && unsure.isEmpty()) || (program && programs.isEmpty())) Brand.Orange
                else MaterialTheme.colorScheme.onSurfaceVariant,
                modifier = Modifier.padding(horizontal = 4.dp))
            if (!nothingToRun) Row(Modifier.horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                if (program) {
                    programs.forEach { name ->
                        FilterChip(selected = service == name, onClick = { service = if (service == name) null else name },
                            label = { Text(name) })
                    }
                } else {
                    FilterChip(
                        selected = repo.isNotBlank(),
                        onClick = { picking = true },
                        label = { Text(if (repo.isBlank()) "Empty folder" else Remembered.short(repo), maxLines = 1) },
                        trailingIcon = { Icon(Icons.Filled.ArrowDropDown, contentDescription = "Where it works") },
                    )
                    FilterChip(selected = askMe, onClick = { askMe = !askMe }, label = { Text("Ask me first") })
                    FilterChip(
                        selected = model.isNotBlank(),
                        onClick = { choosingModel = true },
                        label = { Text(if (model.isBlank()) "Default model" else modelLabel, maxLines = 1) },
                        trailingIcon = { Icon(Icons.Filled.ArrowDropDown, contentDescription = "Model") },
                    )
                }
                FilterChip(selected = queue, onClick = { queue = !queue }, label = { Text("Wait if nobody's free") })
            }
            if (!nothingToRun) Row(verticalAlignment = Alignment.Bottom) {
                TextField(
                    value = if (program) args else prompt,
                    onValueChange = { if (program) args = it else prompt = it },
                    placeholder = { Text(if (program) "Arguments (optional)" else "What should the agent do?") },
                    modifier = Modifier.weight(1f).heightIn(min = 56.dp, max = 180.dp),
                    shape = RoundedCornerShape(18.dp),
                    colors = TextFieldDefaults.colors(
                        focusedContainerColor = Brand.CardHigh,
                        unfocusedContainerColor = Brand.CardHigh,
                        focusedIndicatorColor = Color.Transparent,
                        unfocusedIndicatorColor = Color.Transparent,
                    ),
                    keyboardOptions = if (program) Verbatim else KeyboardOptions(capitalization = KeyboardCapitalization.Sentences),
                )
                Spacer(Modifier.width(8.dp))
                FilledIconButton(
                    onClick = { send(pending = queue) },
                    enabled = !busy && daemon != null && (if (program) service != null else prompt.isNotBlank()),
                    modifier = Modifier.size(52.dp),
                    shape = CircleShape,
                    colors = IconButtonDefaults.filledIconButtonColors(containerColor = Brand.Cyan, contentColor = Brand.Ink),
                ) { Icon(Icons.AutoMirrored.Filled.Send, contentDescription = if (program) "Run the program" else "Start the run") }
            }
        }
    }

    if (picking) {
        WhereSheet(repo, recent, onDismiss = { picking = false }) { chosen ->
            repo = chosen
            picking = false
        }
    }
    if (choosingModel) {
        ModelDialog(
            current = model,
            offered = offered,
            defaultAbout = defaultAbout,
            onRefresh = {
                scope.launch {
                    ask { daemon!!.refreshModels() }
                    // Each device reads for a couple of seconds and gossips it; then look again
                    // rather than wait for the next quarter-minute poll.
                    delay(5_000)
                    ask { daemon!!.nodes() }.onSuccess { view ->
                        nodes = view.nodes.filter { it.status != se.mach25.offload.client.PeerStatus.DEAD }
                    }
                }
            },
            onDismiss = { choosingModel = false },
        ) { chosen ->
            model = chosen
            choosingModel = false
        }
    }
}

/**
 * The model for this run (ADR-0080): the agent's own default, one the fleet's devices list, or
 * one typed by name for a model no device lists yet. Refresh asks every device to read its
 * agent's list again, for a model released since.
 */
@Composable
private fun ModelDialog(
    current: String,
    offered: List<Pair<se.mach25.offload.client.ModelRow, List<String>>>,
    defaultAbout: String?,
    onRefresh: () -> Unit,
    onDismiss: () -> Unit,
    choose: (String) -> Unit,
) {
    val listed = current.isBlank() || offered.any { it.first.value == current }
    var typed by remember { mutableStateOf(if (listed) "" else current) }
    var refreshed by remember { mutableStateOf(false) }
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text("Model") },
        text = {
            Column(Modifier.verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                // At the top, where it is seen: below eleven models it was below the fold.
                if (refreshed) {
                    Text("Asked every device to read its list again. New models show here within a few seconds.",
                        style = MaterialTheme.typography.bodySmall, color = Brand.Cyan)
                }
                ModelOption("Default", defaultAbout ?: "Whatever the device's agent uses by default",
                    selected = current.isBlank()) { choose("") }
                if (offered.isEmpty()) {
                    Text("No device that can take agent runs lists its models yet.",
                        style = MaterialTheme.typography.bodySmall, color = Brand.Orange)
                }
                offered.forEach { (m, by) ->
                    ModelOption(m.name, listOf(m.about, "on " + by.joinToString(", ")).filter { it.isNotBlank() }.joinToString(" · "),
                        selected = current == m.value) { choose(m.value) }
                }
                OutlinedTextField(typed, { typed = it }, label = { Text("Other") },
                    placeholder = { Text("A model's full name") }, singleLine = true, keyboardOptions = Verbatim,
                    modifier = Modifier.fillMaxWidth().padding(top = 8.dp))
            }
        },
        confirmButton = { Button(enabled = typed.isNotBlank(), onClick = { choose(typed.trim()) }) { Text("Use") } },
        dismissButton = { TextButton(onClick = { refreshed = true; onRefresh() }) { Text("Refresh") } },
    )
}

@Composable
private fun ModelOption(name: String, about: String, selected: Boolean, onClick: () -> Unit) {
    Row(Modifier.fillMaxWidth().clickable(onClick = onClick).padding(vertical = 6.dp),
        verticalAlignment = Alignment.CenterVertically) {
        androidx.compose.material3.RadioButton(selected = selected, onClick = onClick)
        Column(Modifier.padding(start = 4.dp)) {
            Text(name, style = MaterialTheme.typography.bodyLarge)
            if (about.isNotBlank()) Text(about, style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
    }
}

private fun said(s: Submitted): String = buildString {
    append("Started ${s.shortId}")
    append(s.node?.let { " — on $it" } ?: "")
    s.waiting?.let { append(" — $it") }
    s.queued?.let { append(" — pending: $it") }
}

/** Where the run works: an empty folder, a repository used before, or a new one. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun WhereSheet(current: String, recent: List<String>, onDismiss: () -> Unit, choose: (String) -> Unit) {
    var typed by remember { mutableStateOf("") }
    ModalBottomSheet(onDismissRequest = onDismiss, containerColor = Brand.Card) {
        Column(Modifier.fillMaxWidth().padding(horizontal = 16.dp).padding(bottom = 24.dp).imePadding()) {
            Text("Where it works", style = MaterialTheme.typography.titleLarge)
            Spacer(Modifier.size(8.dp))
            WhereRow("Empty folder", "A new, empty workspace on whichever device takes it", current.isBlank()) { choose("") }
            recent.forEach { url ->
                HorizontalDivider(color = Brand.CardHigh)
                WhereRow(Remembered.short(url), url, current == url) { choose(url) }
            }
            Spacer(Modifier.size(12.dp))
            OutlinedTextField(typed, { typed = it }, label = { Text("Another repository URL") },
                singleLine = true, keyboardOptions = Verbatim, modifier = Modifier.fillMaxWidth())
            Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.End) {
                TextButton(enabled = typed.isNotBlank(), onClick = { choose(typed.trim()) }) { Text("Use it") }
            }
        }
    }
}

@Composable
private fun WhereRow(title: String, detail: String, selected: Boolean, onClick: () -> Unit) {
    Row(Modifier.fillMaxWidth().clickable(onClick = onClick).padding(vertical = 12.dp),
        verticalAlignment = Alignment.CenterVertically) {
        Column(Modifier.weight(1f)) {
            Text(title, style = MaterialTheme.typography.bodyLarge)
            Text(detail, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant,
                maxLines = 1, overflow = TextOverflow.Ellipsis)
        }
        if (selected) Icon(Icons.Filled.Check, contentDescription = "Chosen", tint = Brand.Cyan)
    }
}

@Composable
private fun Note(text: String, onDismiss: () -> Unit) {
    Card(
        modifier = Modifier.fillMaxWidth().clickable(onClick = onDismiss),
        colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surfaceVariant),
    ) {
        Text(text, style = MaterialTheme.typography.bodyMedium, modifier = Modifier.padding(14.dp))
    }
}

/**
 * What there is to know about a run at a glance, above its log (session ninety-four: the owner
 * wanted to see who took it and which model it used). The model the agent *used* is read from the
 * log's first line, which the agent's own start event writes (`agent claude-code …, model …`);
 * what was asked for is on the run. A row is left out when there is nothing true to put in it.
 */
@Composable
private fun RunFacts(run: RunRow, lines: List<String>) {
    val started = lines.firstOrNull { it.startsWith("agent ") }
    val used = started?.substringAfter(", model ", "")?.takeIf { it.isNotBlank() }
    val agent = started?.removePrefix("agent")?.substringBefore(", model ")?.trim()
    val facts = buildList {
        add("Ran on" to (run.host ?: if (run.state == "pending") "not taken yet" else "unknown"))
        if (!run.task) {
            val asked = run.model ?: "default"
            add("Model" to (used?.let { if (it == asked) it else "$it (asked for $asked)" } ?: asked))
            agent?.let { add("Agent" to it) }
        }
        add("Started" to java.text.DateFormat.getDateTimeInstance(java.text.DateFormat.MEDIUM, java.text.DateFormat.SHORT)
            .format(java.util.Date(run.startedAtUnix.toLong() * 1000)) + " · " + ago(run.startedAtUnix.toLong()))
        if (!run.task) {
            add("Workspace" to where(run))
            add("Turns" to run.turns.toString())
            if (run.tokens > 0uL) add("Tokens" to "%,d".format(run.tokens.toLong()))
            if (run.costMicroUsd > 0uL) add("Cost" to "$%.3f".format(run.costMicroUsd.toLong() / 1_000_000.0))
        }
        run.continues?.let { add("Continues" to it) }
        add("Id" to run.id)
    }
    Column(Modifier.fillMaxWidth().padding(bottom = 6.dp)) {
        facts.forEach { (label, value) ->
            Row(Modifier.padding(vertical = 1.dp)) {
                Text(label, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant,
                    modifier = Modifier.width(88.dp))
                SelectionContainer { Text(value, style = MaterialTheme.typography.bodySmall) }
            }
        }
    }
}

/** One run: its log as `offload logs` words it, following while the sheet is open. */
@Composable
private fun RunDetail(daemon: Daemon?, run: RunRow, onEdit: (RunRow) -> Unit = {}, done: (String) -> Unit) {
    var lines by remember { mutableStateOf<List<String>>(emptyList()) }
    var failure by remember { mutableStateOf<String?>(null) }
    val scope = rememberCoroutineScope()
    val list = rememberLazyListState()
    val lifecycle = LocalLifecycleOwner.current.lifecycle
    LaunchedEffect(run.id, lifecycle) {
        // Only while the app is on screen: cancelled in the background, started again on return.
        lifecycle.repeatOnLifecycle(Lifecycle.State.STARTED) {
            while (true) {
                ask { daemon!!.log(run.id) }
                    .onSuccess {
                        val grew = it.size != lines.size
                        lines = it
                        failure = null
                        if (grew && it.isNotEmpty()) list.animateScrollToItem(it.size - 1)
                    }
                    .onFailure { failure = it.message }
                delay(2_000)
            }
        }
    }
    Column(Modifier.fillMaxWidth().fillMaxHeight(0.85f).padding(horizontal = 16.dp)) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Text(run.shortId, style = MaterialTheme.typography.titleLarge, fontFamily = FontFamily.Monospace)
            Spacer(Modifier.size(8.dp))
            StateChip(run.state)
        }
        // Its own row: beside a 12-character id and the state, two buttons had no room and
        // "Cancel" was crushed into a column of single letters (emulator, session ninety-two).
        var browsing by remember { mutableStateOf(false) }
        if (browsing) FilesSheet(daemon, run) { browsing = false }
        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.End) {
            // What it wrote, read on the machine that has it (ADR-0075). Not for a program run,
            // which has no workspace, nor before anything was placed.
            if (!run.task && run.state != "pending") {
                OutlinedButton(onClick = { browsing = true }) { Text("Files") }
                Spacer(Modifier.size(8.dp))
            }
            // A run nobody has taken can be changed: withdrawn through the ordinary cancel, which
            // the node arbitrating it decides, and its prompt put back in the composer. Not
            // edited in place: a submitted spec gossips, and only its deadline and priority have
            // an owner and a counter to change it by.
            if (run.state == "pending" && !run.task) {
                OutlinedButton(onClick = {
                    scope.launch {
                        ask { daemon!!.cancel(run.id) }
                            .onSuccess { onEdit(run) }
                            .onFailure { failure = it.message }
                    }
                }) { Text("Edit") }
                Spacer(Modifier.size(8.dp))
            }
            if (!run.finished) {
                OutlinedButton(onClick = {
                    scope.launch {
                        ask { daemon!!.cancel(run.id) }
                            .onSuccess { done(it) }
                            .onFailure { failure = it.message }
                    }
                }) { Text("Cancel") }
            }
        }
        Text(run.work, style = MaterialTheme.typography.bodyMedium, modifier = Modifier.padding(vertical = 6.dp))
        RunFacts(run, lines)
        failure?.let { Text(it, color = MaterialTheme.colorScheme.tertiary) }
        if (lines.isEmpty() && failure == null) {
            // A run nobody has started has nothing to say yet, and a blank sheet reads as broken.
            Text(
                if (run.state == "pending") "No log yet — it has not been placed." else "No log yet.",
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                modifier = Modifier.padding(vertical = 8.dp),
            )
        }
        // The agent's answer first, where it is read, and selectable, so it can be copied out:
        // it is what somebody asked for. The log's own wording marks it (`render::event_lines`,
        // "answer"), so this and `offload logs` agree about where it starts.
        val at = lines.indexOf("answer")
        val answer = if (at >= 0) lines.drop(at + 1).joinToString("\n") else null
        val log = if (at >= 0) lines.take(at) else lines
        answer?.let {
            Card(
                modifier = Modifier.fillMaxWidth().padding(vertical = 6.dp).heightIn(max = 320.dp),
                colors = CardDefaults.cardColors(containerColor = Brand.CardHigh),
            ) {
                Column(Modifier.padding(14.dp).verticalScroll(rememberScrollState())) {
                    Text("Answer", style = MaterialTheme.typography.labelLarge, color = Brand.Cyan)
                    Spacer(Modifier.size(4.dp))
                    SelectionContainer { Text(it, style = MaterialTheme.typography.bodyMedium) }
                }
            }
        }
        if (run.state == "pending") WhyWaiting(daemon, run)
        LazyColumn(state = list, modifier = Modifier.weight(1f).fillMaxWidth()) {
            items(log) { line ->
                Text(line, fontFamily = FontFamily.Monospace, fontSize = 12.sp, lineHeight = 16.sp)
            }
        }
        // Another run on this one's work: the same branch, a new prompt (ADR-0064).
        if (run.finished && !run.task) {
            ContinueBar(daemon, run, done)
        }
    }
}

/**
 * "What next?" under a finished run: `offload continue`, a new run starting on this one's branch,
 * so it sees the files and commits this one left. "Keep the conversation" resumes this run's
 * session; without it a fresh one is handed the prompt and the result (the ADR's default).
 */
@Composable
private fun ContinueBar(daemon: Daemon?, run: RunRow, done: (String) -> Unit) {
    var prompt by remember { mutableStateOf("") }
    var session by remember { mutableStateOf(false) }
    var busy by remember { mutableStateOf(false) }
    var refused by remember { mutableStateOf<String?>(null) }
    val scope = rememberCoroutineScope()
    Column(Modifier.fillMaxWidth().imePadding().padding(top = 8.dp, bottom = 12.dp)) {
        refused?.let { Text(it, color = MaterialTheme.colorScheme.tertiary, style = MaterialTheme.typography.bodySmall) }
        FilterChip(selected = session, onClick = { session = !session }, label = { Text("Keep the conversation") })
        Row(verticalAlignment = Alignment.Bottom) {
            TextField(
                value = prompt,
                onValueChange = { prompt = it },
                placeholder = { Text("What next?") },
                modifier = Modifier.weight(1f).heightIn(min = 52.dp, max = 160.dp),
                shape = RoundedCornerShape(18.dp),
                colors = TextFieldDefaults.colors(
                    focusedContainerColor = Brand.CardHigh,
                    unfocusedContainerColor = Brand.CardHigh,
                    focusedIndicatorColor = Color.Transparent,
                    unfocusedIndicatorColor = Color.Transparent,
                ),
                keyboardOptions = KeyboardOptions(capitalization = KeyboardCapitalization.Sentences),
            )
            Spacer(Modifier.width(8.dp))
            FilledIconButton(
                onClick = {
                    busy = true
                    scope.launch {
                        ask { daemon!!.continueRun(run.id, prompt, session, false) }
                            .onSuccess { done(said(it) + " — continuing ${run.shortId}") }
                            .onFailure { refused = it.message }
                        busy = false
                    }
                },
                enabled = prompt.isNotBlank() && !busy && daemon != null,
                modifier = Modifier.size(48.dp),
                shape = CircleShape,
                colors = IconButtonDefaults.filledIconButtonColors(containerColor = Brand.Cyan, contentColor = Brand.Ink),
            ) { Icon(Icons.AutoMirrored.Filled.Send, contentDescription = "Continue this run") }
        }
    }
}

/**
 * Why a run nobody has taken is still waiting, in the fleet's words: `offload explain`'s verdict
 * and each node's answer. The sheet said "pending" and nothing more, while the tablet knew that it
 * had no agent and the laptop was not allowed to host.
 */
@Composable
private fun WhyWaiting(daemon: Daemon?, run: RunRow) {
    var why by remember { mutableStateOf<se.mach25.offload.client.WhyView?>(null) }
    var failure by remember { mutableStateOf<String?>(null) }
    val lifecycle = LocalLifecycleOwner.current.lifecycle
    LaunchedEffect(run.id, lifecycle) {
        // Only while the app is on screen: cancelled in the background, started again on return.
        lifecycle.repeatOnLifecycle(Lifecycle.State.STARTED) {
            while (true) {
                ask { daemon!!.why(run.id) }
                    .onSuccess { why = it; failure = null }
                    .onFailure { failure = it.message }
                delay(5_000)
            }
        }
    }
    Card(
        modifier = Modifier.fillMaxWidth().padding(vertical = 6.dp),
        colors = CardDefaults.cardColors(containerColor = Brand.Orange.copy(alpha = 0.14f)),
    ) {
        Column(Modifier.padding(14.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
            Text("Why it's waiting", style = MaterialTheme.typography.labelLarge, color = Brand.Orange)
            failure?.let { Text(it, style = MaterialTheme.typography.bodySmall) }
            why?.let { w ->
                Text(w.verdict, style = MaterialTheme.typography.bodyMedium)
                w.notAsked?.let { Text(it, style = MaterialTheme.typography.bodySmall) }
                w.answers.forEach { a ->
                    Row {
                        Text(a.node, style = MaterialTheme.typography.bodySmall,
                            color = if (a.wouldTake) Brand.Cyan else MaterialTheme.colorScheme.onSurfaceVariant,
                            modifier = Modifier.width(96.dp), maxLines = 1, overflow = TextOverflow.Ellipsis)
                        Text(a.says, style = MaterialTheme.typography.bodySmall)
                    }
                }
            }
        }
    }
}

/**
 * A run's workspace, read-only (ADR-0075): folders to go into, files to read. Read on the machine
 * that has it, which the header names, from its checkout (with uncommitted work) or, once that is
 * gone, its branch. The owner asked to "peek into a run's workspace, perhaps view a file".
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun FilesSheet(daemon: Daemon?, run: RunRow, onDismiss: () -> Unit) {
    var path by remember { mutableStateOf("") }
    var screen by remember { mutableStateOf<se.mach25.offload.client.FilesScreen?>(null) }
    var failure by remember { mutableStateOf<String?>(null) }
    LaunchedEffect(path) {
        screen = null
        ask { daemon!!.files(run.id, path) }
            .onSuccess { screen = it; failure = null }
            .onFailure { failure = it.message }
    }
    val up = { path = path.substringBeforeLast('/', "") }
    ModalBottomSheet(
        onDismissRequest = onDismiss,
        containerColor = Brand.Card,
        sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true),
    ) {
        Column(Modifier.fillMaxWidth().fillMaxHeight(0.9f).padding(horizontal = 16.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text("/" + path, style = MaterialTheme.typography.titleMedium, fontFamily = FontFamily.Monospace,
                    modifier = Modifier.weight(1f), maxLines = 2, overflow = TextOverflow.Ellipsis)
                if (path.isNotEmpty()) OutlinedButton(onClick = up) { Text("Up") }
            }
            screen?.let { s ->
                Text(
                    "on ${s.node}, " + if (s.fromCheckout) "from its checkout" else "committed files only (the checkout is gone)",
                    style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant,
                    modifier = Modifier.padding(bottom = 8.dp),
                )
            }
            failure?.let { Text(it, color = MaterialTheme.colorScheme.tertiary, modifier = Modifier.padding(vertical = 8.dp)) }
            val s = screen
            when {
                s == null && failure == null -> Text("Reading…", color = MaterialTheme.colorScheme.onSurfaceVariant)
                s == null -> {}
                s.listing -> LazyColumn(Modifier.fillMaxSize()) {
                    if (s.entries.isEmpty()) item { Text("Empty.", color = MaterialTheme.colorScheme.onSurfaceVariant) }
                    items(s.entries, key = { it.name }) { e ->
                        Row(
                            Modifier.fillMaxWidth().clickable { path = if (path.isEmpty()) e.name else "$path/${e.name}" }
                                .padding(vertical = 12.dp),
                            verticalAlignment = Alignment.CenterVertically,
                        ) {
                            Text(if (e.dir) "▸" else " ", color = Brand.Cyan, modifier = Modifier.width(20.dp))
                            Text(e.name + if (e.dir) "/" else "", style = MaterialTheme.typography.bodyLarge,
                                modifier = Modifier.weight(1f), maxLines = 1, overflow = TextOverflow.Ellipsis)
                            if (!e.dir) Text(size(e.bytes), style = MaterialTheme.typography.bodySmall,
                                color = MaterialTheme.colorScheme.onSurfaceVariant)
                        }
                        HorizontalDivider(color = Brand.CardHigh)
                    }
                    if (s.truncated) item { Text("…and more, not listed", style = MaterialTheme.typography.bodySmall) }
                }
                s.text != null -> Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState())) {
                    SelectionContainer {
                        Text(s.text!!, fontFamily = FontFamily.Monospace, fontSize = 12.sp, lineHeight = 16.sp)
                    }
                    if (s.truncated) Text("…cut here: the whole file is ${size(s.bytes)}",
                        style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                }
                else -> Text("Not a text file (${size(s.bytes)}), so it is not shown.",
                    color = MaterialTheme.colorScheme.onSurfaceVariant)
            }
        }
    }
}

private fun size(bytes: ULong): String = when {
    bytes >= 1_048_576uL -> "%.1f MB".format(bytes.toDouble() / 1_048_576.0)
    bytes >= 1024uL -> "%.1f KB".format(bytes.toDouble() / 1024.0)
    else -> "$bytes bytes"
}
