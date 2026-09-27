//! Programs this device's owner nominated as work a run may *be* (ADR-0019 §1).
//!
//! The middle of three tiers of graduated cost. A trigger notices that something happened and
//! costs nothing; an agent reasons about it and costs money; a **task** is the cheap tier in
//! between — a nominated program, run on demand, whose answer is its output and its exit
//! status. `docs/ARCHITECTURE.md`'s graduated-cost section is the argument for why that tier
//! has to exist: a fleet that can only run agents costs money while it sleeps.
//!
//! Nominated rather than submitted, which is the whole of the security story and the third
//! application of one rule after sinks (ADR-0010) and resources (ADR-0011). A run says
//! `--task watch-api` and names a **service**; this node's own config says what that runs. So
//! the outbound access is the owner's grant rather than the submitter's, and a submitter cannot
//! execute arbitrary code anywhere in the fleet — not because a check refuses it, but because
//! there is nowhere in the submission to put a command.
//!
//! What a task deliberately does *not* have (ADR-0019 §2): no workspace, no model, no tokens,
//! no session, no turn boundary and so no checkpoint. Both halves of `RunProgress` are
//! legitimately empty for one, rather than zero-that-means-unknown.

use crate::config::{Config, TaskConfig};
use offload_core::{Capability, Role, Service};

/// This node's nominated tasks, as capabilities the fleet can see.
///
/// `Role::Execute` — "runs execute here" — because that is exactly what this is, and inventing
/// a fifth role for it would mean the one vocabulary had grown a synonym. What travels is the
/// **service** and nothing else: a peer learns that this device can be a `watch-api`, never
/// what `watch-api` runs. `Constraint::ServiceAuthenticated` then asks the question that matters
/// at the keyboard — does *anybody* have a working one — without pinning placement to a device
/// (ADR-0019 §1).
///
/// A task whose program is missing is advertised **unauthenticated** rather than dropped, for
/// the probe's usual reason: a device that quietly offers nothing looks identical to a device
/// nobody configured, and the difference is that one of them can be explained. That bit is the
/// whole of what "this device can run it" means for the cheap tier, so everything that decides
/// or reports on a task reads it — see [`nomination`].
#[must_use]
pub fn task_capabilities(config: &Config) -> Vec<Capability> {
    let mut out = Vec::new();
    for cfg in &config.tasks {
        if cfg.id.trim().is_empty() {
            tracing::warn!(
                command = %cfg.command,
                "ignoring a task with no id: the id is what its capability is advertised under"
            );
            continue;
        }
        let service = match cfg.service() {
            Ok(service) => service,
            Err(e) => {
                tracing::warn!(task = %cfg.id, error = %e, "ignoring a task");
                continue;
            }
        };
        let found = crate::resource::lookup(&cfg.command);
        if let Some(why) = found.why() {
            tracing::warn!(
                task = %cfg.id,
                command = %cfg.command,
                %why,
                "this task cannot be run here; runs asking for it will not place here"
            );
        }
        let mut capability = Capability::new(cfg.capability_id(), service, [Role::Execute]);
        capability.authenticated = found.why().is_none();
        // The owner's words or nothing — never the program's path. A peer learns that this
        // device can run the nightly check, and nothing about how; the command is shown in
        // this node's own `offload status`, where it is the owner reading about their own
        // machine.
        capability.description = cfg.description.clone();
        if let Some(why) = found.why() {
            // …and the reason after the label, because `offload probe` prints this clause as
            // *why* and it was printing the label instead. No path, for the reason above — and
            // the reason itself rather than a guess at it, which is why `lookup` answers three
            // ways: *not there* and *there and not executable* are different repairs.
            capability.unusable(why);
        }
        out.push(capability);
    }
    out
}

/// The task this node would run for `service`, or `None` if it nominated none.
///
/// **First match wins, and two entries claiming one service is a config mistake rather than a
/// choice this makes.** It is warned about at startup rather than resolved cleverly: picking
/// between them would be this daemon having an opinion about which of the owner's programs they
/// meant, and the owner is the only one who knows.
#[must_use]
pub fn task_for<'a>(config: &'a Config, service: &Service) -> Option<&'a TaskConfig> {
    config
        .tasks
        .iter()
        .filter(|cfg| !cfg.id.trim().is_empty())
        .find(|cfg| cfg.service().is_ok_and(|declared| &declared == service))
}

/// What this node can do about a task for `service` — three answers, not two.
///
/// A `bool` here is right twice and confidently wrong once, which is the shape that keeps
/// costing: *nominates nothing for it* and *nominates it and cannot run it* are different
/// sentences with different fixes, and asking `task_for(..).is_some()` answers `true` to the
/// second. Measured on one daemon with a `[[tasks]]` block pointing at a path that does not
/// exist: `offload run --task` accepted it and the run failed a millisecond later, and
/// `offload every --task` and `offload when --task` said nothing at all — while the control,
/// a service nobody nominates, got the whole honest paragraph from each.
///
/// **Read from the capability, never from a second look at the config.** `authenticated` is
/// the fact the bid round places on (`Constraint::ServiceAuthenticated`) and the probe measures
/// it every thirty seconds; re-deriving it here with a second `which` would be a report
/// computed from a second copy of the rule, which is how this file's neighbours went wrong.
/// The config is consulted only for the *command to name*, which is this node's own business
/// and is what makes the refusal actionable.
pub(crate) enum Nomination<'a> {
    /// Nothing here is nominated for that service.
    Nothing,
    /// Nominated, and the program is not on this device. Carries the entry, for its command.
    NoProgram(&'a TaskConfig),
    /// Nominated, and this node can run it.
    Ready,
}

pub(crate) fn nomination<'a>(
    capabilities: &offload_core::Capabilities,
    config: &'a Config,
    service: &Service,
) -> Nomination<'a> {
    let offered = capabilities
        .offering(service)
        .any(|c| c.plays(Role::Execute));
    if !offered {
        return Nomination::Nothing;
    }
    if capabilities
        .offering(service)
        .any(|c| c.authenticated && c.plays(Role::Execute))
    {
        return Nomination::Ready;
    }
    match task_for(config, service) {
        Some(cfg) => Nomination::NoProgram(cfg),
        // A capability advertising the service with nothing in `[[tasks]]` behind it: not
        // reachable from this node's own config, and answered rather than unwrapped because
        // the two are separate reads and only one of them is inside this process's control.
        None => Nomination::Nothing,
    }
}

/// Warn once, at startup, about tasks whose configuration cannot mean what it says.
///
/// Separate from [`task_capabilities`] because that runs on every probe and this should be said
/// once: a line repeated every thirty seconds is one nobody reads.
pub fn check(config: &Config) {
    let mut seen: Vec<(Service, &str)> = Vec::new();
    for cfg in &config.tasks {
        let Ok(service) = cfg.service() else { continue };
        if let Some((_, first)) = seen.iter().find(|(s, _)| s == &service) {
            tracing::warn!(
                service = %service,
                first = %first,
                ignored = %cfg.id,
                "two tasks claim one service; the first is what a run asking for it will get"
            );
        } else {
            seen.push((service, &cfg.id));
        }
    }
}

/// A task process this node started, and the lines it has yet to produce.
///
/// Deliberately not a [`offload_agent::claude::RunHandle`] and deliberately not behind a trait
/// shared with one. An agent handle carries a session id, a transcript path and a turn counter;
/// a task has none of the three, and a trait wide enough for both would be a type saying that
/// half its methods might not mean anything — which is the `Option` mistake ADR-0019 §2 refuses,
/// moved up a level. What the two genuinely share is how the *supervisor* treats them, and that
/// is expressed by both being driven from the same fence, registration and terminal discipline.
#[derive(Debug)]
pub struct TaskHandle {
    child: tokio::process::Child,
    /// `None` once that stream has ended. Tracked explicitly because a finished
    /// `Lines::next_line` resolves to `Ok(None)` for ever, so a `select!` over both would spin
    /// on the dead one rather than waiting on the live one.
    lines: Option<tokio::io::Lines<tokio::io::BufReader<tokio::process::ChildStdout>>>,
    errors: Option<tokio::io::Lines<tokio::io::BufReader<tokio::process::ChildStderr>>>,
    group: Option<u32>,
}

enum Stream {
    Out,
    Err,
}

/// Read a line from a stream that may just have ended, retiring it if it has.
fn take<T>(slot: &mut Option<T>, line: std::io::Result<Option<String>>) -> Option<String> {
    match line {
        Ok(Some(line)) => Some(line),
        Ok(None) | Err(_) => {
            *slot = None;
            None
        }
    }
}

/// What a task did, once it has stopped doing it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskOutcome {
    /// Exit status zero.
    Succeeded,
    /// Exit status non-zero, or killed by a signal.
    Failed { status: String },
}

#[derive(Debug, thiserror::Error)]
pub enum TaskError {
    #[error("this node nominates no task for `{service}`")]
    NotNominated { service: String },
    /// Carries the **measured** reason rather than asserting one. This said *was not found* for
    /// both of `Program`'s two failures, and one of them is a file that is right there without
    /// its execute bit — which is exactly the sentence the probe's own over-claim produced one
    /// layer up. Reachable only in the window between the last probe and the spawn, which is why
    /// it survived: the gate refuses the ordinary case before this is reached.
    #[error("the program for task `{task}` cannot be run: {why}")]
    NoProgram { task: String, why: String },
    #[error("starting task `{task}`: {reason}")]
    Spawn { task: String, reason: String },
}

impl TaskHandle {
    /// The process group, so a task that outlives this daemon can still be stopped
    /// (`crate::leftovers`), exactly as an agent's is.
    #[must_use]
    pub fn process_group(&self) -> Option<u32> {
        self.group
    }

    /// The next line of output, from either stream, or `None` once both are done.
    ///
    /// **stderr is output too.** A task's log *is* its output (ADR-0019 §2), and a program that
    /// reports its problem on stderr and its answer on stdout would otherwise have half of
    /// itself thrown away — which is the failure the delivery plane already learned about
    /// capturing only the convenient stream.
    pub async fn next_line(&mut self) -> Option<String> {
        loop {
            match (&mut self.lines, &mut self.errors) {
                (None, None) => return None,
                (Some(out), None) => {
                    let line = out.next_line().await;
                    return take(&mut self.lines, line);
                }
                (None, Some(err)) => {
                    let line = err.next_line().await;
                    return take(&mut self.errors, line);
                }
                (Some(out), Some(err)) => {
                    // Whichever speaks first. An ended stream is set to `None` and the loop
                    // goes round, so the next wait is on the one still talking.
                    let ended = tokio::select! {
                        line = out.next_line() => match line {
                            Ok(Some(line)) => return Some(line),
                            Ok(None) | Err(_) => Stream::Out,
                        },
                        line = err.next_line() => match line {
                            Ok(Some(line)) => return Some(line),
                            Ok(None) | Err(_) => Stream::Err,
                        },
                    };
                    match ended {
                        Stream::Out => self.lines = None,
                        Stream::Err => self.errors = None,
                    }
                }
            }
        }
    }

    /// Wait for the process and say how it ended.
    pub async fn outcome(&mut self) -> TaskOutcome {
        match self.child.wait().await {
            Ok(status) if status.success() => TaskOutcome::Succeeded,
            Ok(status) => TaskOutcome::Failed {
                status: describe_status(&status),
            },
            Err(e) => TaskOutcome::Failed {
                status: format!("could not be waited for: {e}"),
            },
        }
    }

    /// Stop it, the way an agent is stopped: the whole process group, so a shell wrapper does
    /// not leave its children behind.
    pub fn stop(&mut self) {
        if let Some(group) = self.group {
            offload_agent::claude::terminate_process_group(group);
        }
        let _ = self.child.start_kill();
    }
}

fn describe_status(status: &std::process::ExitStatus) -> String {
    if let Some(code) = status.code() {
        return format!("exit status {code}");
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            return format!("killed by signal {signal}");
        }
    }
    "ended for an unknown reason".to_string()
}

/// Start the task this node nominated for `service`, with the run's arguments after the
/// owner's.
///
/// **The owner's arguments come first and a run's are appended**, never the other way round, so
/// a submission cannot displace what the owner said the program is for. And the environment is
/// the one the owner declared rather than the daemon's, for `ResourceConfig`'s reason:
/// inheriting is the ambient-authority mistake this area exists to avoid.
pub async fn spawn_task(
    config: &Config,
    service: &Service,
    args: &[String],
    run: offload_core::RunId,
) -> Result<TaskHandle, TaskError> {
    let cfg = task_for(config, service).ok_or_else(|| TaskError::NotNominated {
        service: service.to_string(),
    })?;
    let found = crate::resource::lookup(&cfg.command);
    let program = found.clone().path().ok_or_else(|| TaskError::NoProgram {
        task: cfg.id.clone(),
        why: found.refusal(&cfg.command),
    })?;

    let mut command = tokio::process::Command::new(&program);
    command
        .args(&cfg.args)
        .args(args)
        .env_clear()
        .envs(cfg.env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    // Its own process group, so stopping the task stops what it started — the same reason the
    // agent gets one, and the same reason `leftovers` can clean up after a daemon that died.
    #[cfg(unix)]
    command.process_group(0);

    tracing::info!(
        run_id = %run,
        task = %cfg.id,
        service = %service,
        program = %program.display(),
        "spawning a task"
    );
    let mut child =
        offload_agent::claude::spawn_when_not_busy(|| command.spawn()).map_err(|e| {
            TaskError::Spawn {
                task: cfg.id.clone(),
                reason: e.to_string(),
            }
        })?;

    let stdout = child.stdout.take().ok_or_else(|| TaskError::Spawn {
        task: cfg.id.clone(),
        reason: "the task's stdout could not be captured".to_string(),
    })?;
    let stderr = child.stderr.take().ok_or_else(|| TaskError::Spawn {
        task: cfg.id.clone(),
        reason: "the task's stderr could not be captured".to_string(),
    })?;
    let group = child.id();
    Ok(TaskHandle {
        lines: Some(tokio::io::AsyncBufReadExt::lines(
            tokio::io::BufReader::new(stdout),
        )),
        errors: Some(tokio::io::AsyncBufReadExt::lines(
            tokio::io::BufReader::new(stderr),
        )),
        child,
        group,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use offload_core::{Arch, DeviceClass, Os};

    fn config(tasks: Vec<TaskConfig>) -> Config {
        Config {
            tasks,
            ..Config::default()
        }
    }

    fn task(id: &str, service: &str, command: &str) -> TaskConfig {
        TaskConfig {
            id: id.into(),
            service: service.into(),
            command: command.into(),
            ..TaskConfig::default()
        }
    }

    /// Three answers, because a `bool` here was right twice and confidently wrong once.
    ///
    /// Measured on one daemon with a `[[tasks]]` block naming a path that does not exist:
    /// `offload run --task` accepted it and the run failed a millisecond later, `offload every
    /// --task` and `offload when --task` said nothing at all, and `offload status` — which has a
    /// line for a broken **resource** — had none for this. Every one of those readers asked
    /// "is it nominated" where the answer it needed was "can this device run it".
    #[test]
    fn a_task_that_is_nominated_and_a_task_that_can_run_are_two_questions() {
        let here = offload_core::Service::Other("here".into());
        let missing = offload_core::Service::Other("missing".into());
        let unknown = offload_core::Service::Other("unknown".into());
        let config = config(vec![
            task("here", "here", "/bin/echo"),
            task("missing", "missing", "/nowhere/at/all"),
        ]);

        let mut caps =
            offload_core::Capabilities::empty(Os::Linux, Arch::X86_64, DeviceClass::Laptop);
        for capability in task_capabilities(&config) {
            caps.add(capability);
        }

        assert!(matches!(
            nomination(&caps, &config, &here),
            Nomination::Ready
        ));
        // Nominated — `task_for` finds it, which is what every caller used to ask — and not
        // runnable, which is what they all needed.
        assert!(task_for(&config, &missing).is_some());
        assert!(matches!(
            nomination(&caps, &config, &missing),
            Nomination::NoProgram(cfg) if cfg.command == "/nowhere/at/all"
        ));
        assert!(matches!(
            nomination(&caps, &config, &unknown),
            Nomination::Nothing
        ));
    }

    #[test]
    fn a_task_is_advertised_by_service_and_never_by_command() {
        // The whole of ADR-0019 §1's security argument, as a test: what leaves this node is the
        // owner's declaration, and the command stays home. If the command ever appears in a
        // capability, a peer has learned how to invoke something on a machine it does not own.
        let config = config(vec![TaskConfig {
            description: "watches the staging API".into(),
            ..task("api", "webhook", "/bin/echo")
        }]);
        let caps = task_capabilities(&config);
        assert_eq!(caps.len(), 1);
        assert_eq!(caps[0].id.0, "task:api");
        assert_eq!(caps[0].service, Service::Webhook);
        assert!(caps[0].plays(Role::Execute), "runs execute here");
        assert!(caps[0].authenticated, "/bin/echo exists");
        assert_eq!(caps[0].description, "watches the staging API");
        let rendered = format!("{:?}", caps[0]);
        assert!(
            !rendered.contains("/bin/echo"),
            "the command must not travel: {rendered}"
        );
    }

    #[test]
    fn a_task_whose_program_is_missing_is_unauthenticated_rather_than_absent() {
        // "Unknown is not none, and unknown is not good news": a device that quietly drops a
        // misconfigured task looks exactly like a device nobody configured, and only one of
        // those can be explained to the person who configured it.
        let caps = task_capabilities(&config(vec![task(
            "api",
            "webhook",
            "/definitely/not/here",
        )]));
        assert_eq!(caps.len(), 1);
        assert!(!caps[0].authenticated);
    }

    #[test]
    fn a_task_with_no_id_or_an_unreadable_service_is_ignored() {
        let caps = task_capabilities(&config(vec![
            task("  ", "webhook", "/bin/echo"),
            task("api", "agent:nonesuch", "/bin/echo"),
        ]));
        assert!(
            caps.is_empty(),
            "neither can be advertised honestly: {caps:?}"
        );
    }

    #[test]
    fn a_service_resolves_to_the_first_task_that_claims_it() {
        let config = config(vec![
            task("first", "webhook", "/bin/echo"),
            task("second", "webhook", "/bin/true"),
            task("mail", "email", "/bin/echo"),
        ]);
        assert_eq!(
            task_for(&config, &Service::Webhook).map(|t| t.id.as_str()),
            Some("first"),
            "first match wins rather than this daemon choosing between the owner's programs"
        );
        assert_eq!(
            task_for(&config, &Service::Email).map(|t| t.id.as_str()),
            Some("mail")
        );
        assert!(task_for(&config, &Service::Push).is_none());
    }

    fn run_id() -> offload_core::RunId {
        offload_core::RunId::from_bytes([3; 16])
    }

    #[tokio::test]
    async fn a_task_runs_and_its_output_is_its_log() {
        // ADR-0019 §2: "a task's log is its output". Stdout and exit status, and no new plane.
        let config = config(vec![TaskConfig {
            args: vec!["one".into(), "two".into()],
            ..task("say", "webhook", "/bin/echo")
        }]);
        let mut handle = spawn_task(&config, &Service::Webhook, &["three".into()], run_id())
            .await
            .expect("spawn");
        let mut lines = Vec::new();
        while let Some(line) = handle.next_line().await {
            lines.push(line);
        }
        // The owner's arguments first and the run's appended — never the other way round, so a
        // submission cannot displace what the owner said the program is for.
        assert_eq!(lines, vec!["one two three"]);
        assert_eq!(handle.outcome().await, TaskOutcome::Succeeded);
    }

    #[tokio::test]
    async fn a_task_that_fails_says_how_rather_than_just_that() {
        let config = config(vec![task("nope", "webhook", "/bin/false")]);
        let mut handle = spawn_task(&config, &Service::Webhook, &[], run_id())
            .await
            .expect("spawn");
        while handle.next_line().await.is_some() {}
        assert_eq!(
            handle.outcome().await,
            TaskOutcome::Failed {
                status: "exit status 1".into()
            }
        );
    }

    #[tokio::test]
    async fn both_streams_are_the_log_and_neither_starves_the_other() {
        // A program that reports its problem on stderr and its answer on stdout would
        // otherwise have half of itself thrown away. Also the regression test for the reader
        // itself: a finished `Lines` resolves to `Ok(None)` for ever, so a naive `select!`
        // over both spins on the dead stream instead of waiting on the live one.
        let config = config(vec![TaskConfig {
            args: vec!["-c".into(), "echo out; echo err 1>&2; echo out2".into()],
            ..task("both", "webhook", "/bin/sh")
        }]);
        let mut handle = spawn_task(&config, &Service::Webhook, &[], run_id())
            .await
            .expect("spawn");
        let mut lines = Vec::new();
        while let Some(line) = handle.next_line().await {
            lines.push(line);
        }
        lines.sort();
        assert_eq!(lines, vec!["err", "out", "out2"]);
        assert_eq!(handle.outcome().await, TaskOutcome::Succeeded);
    }

    #[tokio::test]
    async fn a_task_sees_only_the_environment_its_owner_declared() {
        // Inheriting the daemon's environment is the ambient-authority mistake this whole area
        // exists to avoid, and it is the one a task would inherit *credentials* through.
        std::env::set_var("OFFLOAD_TASK_LEAK_PROBE", "leaked");
        let config = config(vec![TaskConfig {
            args: vec![
                "-c".into(),
                "echo [${OFFLOAD_TASK_LEAK_PROBE:-unset}] [$DECLARED]".into(),
            ],
            env: vec![("DECLARED".into(), "yes".into())],
            ..task("env", "webhook", "/bin/sh")
        }]);
        let mut handle = spawn_task(&config, &Service::Webhook, &[], run_id())
            .await
            .expect("spawn");
        let line = handle.next_line().await.expect("a line");
        assert_eq!(
            line, "[unset] [yes]",
            "the daemon's environment must not leak"
        );
    }

    #[tokio::test]
    async fn asking_for_a_service_this_node_did_not_nominate_says_so() {
        let nominated = config(vec![task("api", "webhook", "/bin/echo")]);
        assert!(matches!(
            spawn_task(&nominated, &Service::Email, &[], run_id()).await,
            Err(TaskError::NotNominated { .. })
        ));
        // And a nominated task whose program is gone is a different sentence, because it is a
        // different thing to go and fix.
        let missing = config(vec![task("api", "webhook", "/definitely/not/here")]);
        assert!(matches!(
            spawn_task(&missing, &Service::Webhook, &[], run_id()).await,
            Err(TaskError::NoProgram { .. })
        ));
    }

    #[test]
    fn a_node_says_what_it_offers_when_it_cannot_offer_what_was_asked() {
        // The walk that found this: `offload run --task push` on a node nominating no such
        // program was **accepted**, started, and failed a millisecond later — a run in the
        // log, a notification, and a person finding out at breakfast what they could have
        // been told at the keyboard (ADR-0014). The refusal itself lives in
        // `server::task_refusal`; what is checked here is the lookup it rests on, because
        // that is the part with a rule in it.
        let config = config(vec![
            task("api", "webhook", "/bin/echo"),
            task("mail", "email", "/bin/echo"),
        ]);
        assert!(task_for(&config, &Service::Push).is_none());
        let offered: Vec<String> = config
            .tasks
            .iter()
            .filter_map(|cfg| cfg.service().ok())
            .map(|s| s.to_string())
            .collect();
        assert_eq!(
            offered,
            vec!["webhook", "email"],
            "the refusal names these, because the commonest cause is a service spelled \
             differently in node.toml"
        );
    }

    #[tokio::test]
    async fn stopping_a_task_takes_its_children_with_it() {
        // A nominated program is usually a shell wrapping something else, so killing only the
        // shell leaves the work running — and a cancelled run whose program is still going is
        // the double-execution hazard wearing a different hat. Its own process group is what
        // makes one signal reach all of it.
        let config = config(vec![TaskConfig {
            args: vec!["-c".into(), "sleep 30 & echo $!; wait".into()],
            ..task("slow", "schedule", "/bin/sh")
        }]);
        let mut handle = spawn_task(&config, &Service::Schedule, &[], run_id())
            .await
            .expect("spawn");
        let child_pid: u32 = handle
            .next_line()
            .await
            .expect("the inner pid")
            .trim()
            .parse()
            .expect("a pid");
        let group = handle.process_group().expect("a process group");
        assert_ne!(group, child_pid, "the inner process is a separate process");

        handle.stop();
        let _ = handle.outcome().await;
        // Give the signal a moment to be delivered to the group.
        for _ in 0..50 {
            if !std::path::Path::new(&format!("/proc/{child_pid}")).exists() {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        panic!("the task's child {child_pid} outlived the task being stopped");
    }
}
