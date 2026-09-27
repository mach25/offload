//! `offload init`, `join`, `verify`, `grant`, `revoke`, `fleet` — the front door.
//!
//! All of it works with no daemon and no network, because ADR-0012's whole point is that
//! membership is verifiable from the fleet public key alone. Founding a fleet, enrolling a
//! device and revoking one are local operations that happen to produce facts other nodes will
//! later gossip.
//!
//! The passphrase never leaves this module: it is read into a zeroizing buffer, derived once,
//! and dropped. Nothing writes it anywhere, and no command takes it as an argument — an
//! argument would be in the shell history of the machine holding the fleet's root secret.

use anyhow::{bail, Context, Result};
use offload_core::fleet::passphrase::ENTROPY_BYTES;
use offload_core::fleet::{FleetKey, Passphrase, PROBATION, REAPPROVAL_WINDOW};
use offload_core::{Grant, Millis, NodeId};
use offload_node::config::Config;
use offload_node::fleet::{self, FleetState, PassphraseUse, Revoked};
use offload_node::identity;
use rand::RngCore;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};

/// Where this device's state lives, honouring `OFFLOAD_STATE_DIR` the same way the daemon
/// does — otherwise `offload init` would found a fleet the daemon cannot find.
#[must_use]
pub fn state_dir(explicit: Option<PathBuf>) -> PathBuf {
    explicit.unwrap_or_else(|| Config::default().state_dir)
}

fn now() -> Millis {
    Millis(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX)),
    )
}

/// Found a fleet on this device.
pub fn init(dir: Option<PathBuf>, name: Option<String>, hardware_key: bool) -> Result<()> {
    let dir = state_dir(dir);
    if let Some(existing) = fleet::load(&dir)? {
        bail!(
            "this node already belongs to fleet {} — founding another would abandon it. \
             Use a separate OFFLOAD_STATE_DIR for a second fleet.",
            existing.fleet.short()
        );
    }
    // Read first: a fleet founded on the wrong key is a rekey to undo.
    let issuer_key = if hardware_key {
        let Some(info) = offload_node::approval_key::read(&dir) else {
            bail!(
                "no approval key in {} — the host app writes {} when it has made one",
                dir.display(),
                offload_node::approval_key::KEY_FILE
            );
        };
        println!("Approval key {}.", info.level());
        Some(info.issuer_key().map_err(|e| anyhow::anyhow!(e))?)
    } else {
        None
    };
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("creating state dir {}", dir.display()))?;

    let identity = identity::load_or_create(&dir).context("node identity")?;
    let name = name.unwrap_or_else(|| Config::default().name);

    // Generated, never chosen (ADR-0012): the fleet public key travels in every certificate,
    // so anything a human would invent is grindable offline.
    let mut entropy = [0u8; ENTROPY_BYTES];
    rand::rngs::OsRng.fill_bytes(&mut entropy);
    let passphrase = Passphrase::generate(&entropy)?;
    let key = passphrase.derive()?;

    let state = match issuer_key {
        Some(issuer_key) => {
            FleetState::found_with_key(&key, identity.id(), &name, now(), issuer_key)
        }
        None => FleetState::found(&key, identity.id(), &name, now()),
    };
    fleet::save(&dir, &state)?;
    record_event(
        &dir,
        offload_core::FleetEvent::Enrolled {
            node: identity.id(),
            name: name.clone(),
            grants: state.membership.grants.clone(),
            how: offload_core::Enrolment::Founded,
        },
    );

    println!("Founded fleet {}", state.fleet);
    println!("  this node   {} ({name})", identity.id());
    println!();
    println!("  Write this down. It is shown once and stored nowhere:");
    println!();
    println!("      {}", passphrase.expose());
    println!();
    println!("  It is the fleet. Anything holding it can enrol a device, revoke one, and act");
    println!("  with your authority — and losing it means re-founding the fleet and running");
    println!("  `offload join` on every other device.");
    println!();
    println!("  Put it somewhere safe now, then check you copied it right:");
    println!();
    println!("      offload verify");
    println!();
    println!("  This device is your only approver, so it is the one that enrols the others.");
    println!("  Grant `approve` to a second device once you have one — while there is only");
    println!("  one, losing it means opening the drawer again.");
    first_fleet_note();
    Ok(())
}

/// What to call this device, and who it is, before it belongs to anything.
///
/// Printed so it can be pasted into `offload invite` on a device that is already a member. The
/// identity is created if this is a fresh state directory, because a node id that changes
/// between "tell me who you are" and "here is your certificate" would produce an invitation for
/// a device that no longer exists.
pub fn id(dir: Option<PathBuf>) -> Result<()> {
    let dir = state_dir(dir);
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("creating state dir {}", dir.display()))?;
    let identity = identity::load_or_create(&dir).context("node identity")?;
    println!("{}", identity.id());
    Ok(())
}

/// Issue a certificate for another device, for somebody to carry to it (ADR-0012).
///
/// The path for a machine where confirming interactively is awkward — a headless box over SSH,
/// which is the case the ADR keeps this command for. Two steps because an approval is an
/// *offline artifact*: the joining device says who it is, and what comes back is a certificate
/// naming that key.
pub fn invite(
    dir: Option<PathBuf>,
    node: &str,
    name: Option<String>,
    grants: Vec<Grant>,
) -> Result<()> {
    let dir = state_dir(dir);
    let state = fleet::require(&dir)?;
    let identity = identity::load_or_create(&dir).context("node identity")?;
    let member: NodeId = node.parse().with_context(|| {
        format!("{node} is not a node id (64 hex characters) — run `offload id` on that device")
    })?;
    let name = name.unwrap_or_else(|| member.short());
    let at = now();

    let wanted: std::collections::BTreeSet<Grant> = grants.into_iter().collect();
    let beyond: Vec<&Grant> = wanted
        .iter()
        .filter(|g| !offload_core::default_grants().contains(g))
        .collect();

    let credentials = if beyond.is_empty() {
        // An approver's own delegation is enough for what a device gets at the door, which is
        // what keeps the passphrase in the drawer.
        // ADR-0069 §4: an approver whose delegation names a hardware key signs there, and a
        // person confirms on this device's screen. Said before the wait, or it reads as a hang.
        let hardware = offload_node::approval_key::RequestFiles::new(
            &dir,
            std::time::Duration::from_secs(120),
        );
        let in_hardware = state
            .approver
            .as_ref()
            .is_some_and(|d| !d.issuer_key.is_node());
        if in_hardware {
            println!(
                "This device approves with a key in secure hardware. Confirm on its screen: \
                 approve {name} ({}) into the fleet.",
                member.short()
            );
        }
        state
            .invite(
                identity.signing_key(),
                in_hardware.then_some(&hardware as &dyn offload_node::approval_key::HardwareSigner),
                member,
                &name,
                at,
            )
            .map_err(|e| anyhow::anyhow!(e))?
    } else {
        // Anything wider needs the root (ADR-0012 mitigation 1): `host-runs` is the grant that
        // turns membership into "runs agents on your repositories with your credentials", and
        // an approver being able to hand it out would make the least-privilege door decorative.
        let list: Vec<String> = beyond.iter().map(ToString::to_string).collect();
        println!(
            "{} is beyond what an approver may issue, so this needs the fleet passphrase.",
            list.join(" and ")
        );
        let key = ask("Fleet passphrase: ")?;
        let mut grants = offload_core::default_grants();
        grants.extend(wanted);
        let credentials = state.issue_for(&key, member, &name, grants, at)?;
        let mut state = state;
        state.record(PassphraseUse::Grant, at);
        fleet::save(&dir, &state)?;
        let (node, node_name) = me(&state);
        record_event(
            &dir,
            offload_core::FleetEvent::PassphraseUsed {
                what: format!("issue a certificate for {}", member.short()),
                node,
                name: node_name,
            },
        );
        credentials
    };

    // Recorded on the machine that issued it, not on the one that will use it: this is the
    // node that *witnessed* the enrolment, and it is also the one most likely to be somewhere
    // the fleet's delivery routes can be reached from.
    record_event(
        &dir,
        offload_core::FleetEvent::Enrolled {
            node: member,
            name: name.clone(),
            grants: credentials.membership.grants.clone(),
            how: offload_core::Enrolment::Invited,
        },
    );

    println!();
    let token = fleet::encode_invite(&fleet::Invitation::new(credentials));
    println!("Invitation for {member} ({name}):");
    println!();
    println!("    {}", token);
    println!();
    println!("  On that device:  offload join --token <the line above>");
    println!("  On a phone with the Offload app, open this link there instead:");
    println!();
    println!("    {}", fleet::join_link(&token));
    println!();
    println!("  Safe to paste anywhere. It is a certificate naming that device's key, not a");
    println!("  password: nobody else can use it, and it grants nothing on this machine.");
    Ok(())
}

/// Enrol this device into an existing fleet.
pub fn join(
    dir: Option<PathBuf>,
    name: Option<String>,
    with_passphrase: bool,
    token: Option<String>,
) -> Result<()> {
    if let Some(token) = token {
        return join_with_token(dir, &token);
    }
    if !with_passphrase {
        // The ordinary path asks an enrolled approver over the network and waits for somebody
        // to confirm on a device they are holding. Saying so beats silently falling back to the
        // recovery path, which grants differently and is meant to be rare enough to notice.
        bail!(
            "asking an approver over the network is not built yet. Use `offload invite` on a \
             device that is already a member and `offload join --token`, or \
             `offload join --passphrase` to enrol this device directly."
        );
    }

    let dir = state_dir(dir);
    if let Some(existing) = fleet::load(&dir)? {
        bail!(
            "this node already belongs to fleet {} — joining another would abandon it. \
             Use a separate OFFLOAD_STATE_DIR for a second fleet.",
            existing.fleet.short()
        );
    }
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("creating state dir {}", dir.display()))?;

    let identity = identity::load_or_create(&dir).context("node identity")?;
    let name = name.unwrap_or_else(|| Config::default().name);
    let key = ask("Fleet passphrase: ")?;

    let at = now();
    let state = FleetState::join(&key, identity.id(), &name, at);
    fleet::save(&dir, &state)?;
    record_event(
        &dir,
        offload_core::FleetEvent::Enrolled {
            node: identity.id(),
            name: name.clone(),
            grants: state.membership.grants.clone(),
            how: offload_core::Enrolment::Passphrase,
        },
    );
    record_event(
        &dir,
        offload_core::FleetEvent::PassphraseUsed {
            what: "enrol this device".into(),
            node: identity.id(),
            name: name.clone(),
        },
    );

    println!("Joined fleet {}", state.fleet);
    println!("  this node   {} ({name})", identity.id());
    println!("  granted     {}", grant_list(&state, at));
    println!();
    println!("  `host-runs` was not granted: joining a fleet is not permission to run agents");
    println!("  on its repositories. Grant it deliberately with `offload grant host-runs`.");
    first_fleet_note();
    Ok(())
}

/// Take up an invitation an existing member issued.
///
/// Also how a device that is *already* a member is handed a wider certificate — `offload invite
/// <node> --grant host-runs` on the machine with the passphrase, and this on the machine that
/// is getting the grant. That is ADR-0012's "`offload grant` gains a `<node>` argument", by the
/// route that does not need the two devices to be on speaking terms.
fn join_with_token(dir: Option<PathBuf>, token: &str) -> Result<()> {
    let dir = state_dir(dir);
    let at = now();
    // The decisions are `offload_node::fleet::take_up_invitation`'s, shared with the app
    // (ADR-0071); what is said about them stays here.
    let held_before = fleet::load(&dir)?.map(|s| grant_list(&s, at));
    match fleet::take_up_invitation(&dir, token, at).map_err(|e| anyhow::anyhow!(e))? {
        fleet::TakenUp::Moved { from, state } => {
            let existing_fleet = from;
            println!("Moved to fleet {}", state.fleet);
            println!("  was         {}", existing_fleet);
            println!(
                "  this node   {} ({})",
                state.membership.member, state.membership.name
            );
            println!("  granted     {}", grant_list(&state, at));
            println!();
            println!("  Runs, repositories and checkpoints are untouched: the fleet key");
            println!("  authenticates and never encrypts, so replacing it costs membership and");
            println!("  nothing else.");
        }
        fleet::TakenUp::NewCertificate(existing) => {
            let held = held_before.unwrap_or_default();
            println!("Took up a new certificate in fleet {}.", existing.fleet);
            println!("  grants      {}", grant_list(&existing, at));
            // What changed, not just what is now true. The two renderings are identical whenever a
            // certificate is merely renewed, and a person who cannot tell "nothing moved" from
            // "something moved" has to go and diff `fleet.json` to find out.
            let now_held = grant_list(&existing, at);
            if now_held != held {
                println!("  was         {held}");
            }
            if let Some(until) = existing.membership.probation_until(at) {
                println!(
                    "  host-runs   dormant for {} (probation)",
                    offload_core::fleet::dormant_for(until.saturating_sub(at))
                );
            }
            println!();
            println!("  A daemon running on this device picks it up within a second. No restart.");
        }
        fleet::TakenUp::Joined(state) => {
            println!("Joined fleet {}", state.fleet);
            println!(
                "  this node   {} ({})",
                state.membership.member, state.membership.name
            );
            println!("  granted     {}", grant_list(&state, at));
            // The certificate's text, not its effect: probation suppresses `host-runs` for fifteen
            // minutes, and telling somebody it "was not granted" because of that would send them to
            // grant it again.
            if let Some(until) = state.membership.probation_until(at) {
                println!(
                    "  host-runs   dormant for {} (probation)",
                    offload_core::fleet::dormant_for(until.saturating_sub(at))
                );
            } else if !state
                .membership
                .grants
                .contains(&offload_core::Grant::HostRuns)
            {
                println!();
                println!("  `host-runs` was not granted. It is the one grant that needs the fleet");
                println!(
                    "  passphrase — `offload grant host-runs` here, or `offload invite <this node>"
                );
                println!("  --grant host-runs` on the device that has it.");
            }
            first_fleet_note();
        }
    }
    Ok(())
}

/// The one change to membership a running daemon does *not* pick up: its first fleet. A daemon
/// decides at startup whether it has a mesh, and one that started outside any fleet has no mesh
/// to hand the certificate to — while every later change (a grant, a renewal, a revocation)
/// arrives within a second. Said here because nothing else did: `offload nodes` went on telling
/// a device that had just joined to run `offload init` (the iOS Simulator, session ninety-two).
fn first_fleet_note() {
    println!();
    println!("  If offloadd is already running on this device, restart it: a daemon that");
    println!("  started outside a fleet joins the mesh only when it starts again.");
}

/// Re-found the fleet under a new passphrase, optionally leaving somebody out.
///
/// ADR-0012 mitigation 5, and the reason it exists is that ordinary revocation is eventually
/// consistent and, in the worst case, outrunnable: it has to *reach* every node. This does not
/// have to reach anybody. The evicted device is not revoked, it is simply not in the new fleet.
///
/// One command, because the alternative — a documented multi-step procedure — is a thing nobody
/// does at 2am. What it cannot do is push the new certificates: the other devices are in a fleet
/// this one has just left, so there is no authenticated channel to them any more. So it prints
/// an invitation each, which is the honest cost the ADR already names — walk to each device once.
pub fn rekey(dir: Option<PathBuf>, evict: Vec<String>) -> Result<()> {
    let dir = state_dir(dir);
    let old = fleet::require(&dir)?;
    let identity = identity::load_or_create(&dir).context("node identity")?;

    // Resolved the same way `revoke` resolves, and for the same reason: `--evict` took 64 hex
    // characters and the two commands that list peers print twelve.
    let evicted: Result<Vec<NodeId>> = evict
        .iter()
        .map(|node| resolve_member(&dir, &old, node))
        .collect();
    let evicted = evicted?;
    if evicted.contains(&identity.id()) {
        bail!(
            "that is this device. Rekeying keeps the machine you run it on — to leave the fleet \
             from here, delete this state directory."
        );
    }

    // The current passphrase, which is the authority check: rekeying is the most destructive
    // thing this program can do to a fleet, and a stolen unlocked laptop must not be able to
    // re-found around its owner.
    println!("Rekeying replaces the fleet. Every other device has to be re-enrolled.");
    let key = ask("Current fleet passphrase: ")?;
    if !key.is(old.fleet) {
        bail!(
            "that is not this fleet's passphrase — it derives {}, this node belongs to {}",
            key.id().short(),
            old.fleet.short()
        );
    }

    let known: Vec<KnownMember> = offload_store::Store::open(&dir)
        .and_then(|store| store.known_members())
        .unwrap_or_else(|e| {
            eprintln!("warning: could not read the fleet log ({e}); re-issuing to nobody");
            Vec::new()
        })
        .into_iter()
        .filter(|(node, _, _)| *node != identity.id() && !evicted.contains(node))
        .collect();
    let (revoked, members) = split_revoked(&old, known);

    let mut entropy = [0u8; ENTROPY_BYTES];
    rand::rngs::OsRng.fill_bytes(&mut entropy);
    let passphrase = Passphrase::generate(&entropy)?;
    let new_key = passphrase.derive()?;

    let at = now();
    let state = old.refound(&new_key, at);
    fleet::save(&dir, &state)?;
    record_event(
        &dir,
        offload_core::FleetEvent::PassphraseUsed {
            what: format!("re-found the fleet as {}", state.fleet.short()),
            node: identity.id(),
            name: state.membership.name.clone(),
        },
    );
    record_event(
        &dir,
        offload_core::FleetEvent::Enrolled {
            node: identity.id(),
            name: state.membership.name.clone(),
            grants: state.membership.grants.clone(),
            how: offload_core::Enrolment::Founded,
        },
    );

    println!();
    println!("Re-founded as fleet {}", state.fleet);
    println!(
        "  this node   {} ({})",
        identity.id(),
        state.membership.name
    );
    println!();
    println!("  Write this down. It is shown once and stored nowhere:");
    println!();
    println!("      {}", passphrase.expose());
    println!();
    println!("  The old passphrase is now the passphrase of a fleet with no members. Runs,");
    println!("  repositories, checkpoints and this device's identity are untouched — the fleet");
    println!("  key authenticates and never encrypts, exactly so that this costs membership and");
    println!("  nothing else.");

    for (node, name, grants) in &members {
        let mut wanted = offload_core::default_grants();
        for grant in grants {
            if let Ok(grant) = parse_grant(grant) {
                wanted.insert(grant);
            }
        }
        let credentials = state.issue_for(&new_key, *node, name, wanted, at)?;
        // Signed by the key being *replaced*, which every one of these devices still holds.
        // Without it they would have no way to tell this from somebody else's fleet inviting
        // them, and the only safe answer to that is no.
        let invitation = fleet::Invitation::succeeding(
            credentials,
            offload_core::Succession::issue(key.signing_key(), old.fleet, state.fleet, at, at.0),
        );
        record_event(
            &dir,
            offload_core::FleetEvent::Enrolled {
                node: *node,
                name: name.clone(),
                grants: invitation.credentials.membership.grants.clone(),
                how: offload_core::Enrolment::Invited,
            },
        );
        println!();
        println!("  {name} ({}) — on that device:", node.short());
        println!();
        println!(
            "      offload join --token {}",
            fleet::encode_invite(&invitation)
        );
    }

    println!();
    for node in &evicted {
        println!(
            "  {} is not in the new fleet. Nothing had to reach it, and nothing",
            node.short()
        );
        println!("  it still holds is a membership — which is the difference between this and");
        println!("  `offload revoke`.");
    }
    for (node, name, _) in &revoked {
        println!(
            "  {name} ({}) was revoked, so it is not re-invited either. A rekey is a",
            node.short()
        );
        println!("  new fleet and this one starts with nobody thrown out of it — re-invite that");
        println!("  device deliberately if the revocation is no longer what you want.");
    }
    if members.is_empty() {
        println!("  No other members were on record here, so nothing was re-issued. This device");
        println!("  only knows the devices it has met — check `offload nodes --history` on the");
        println!("  others, or re-invite them with `offload invite`.");
    } else {
        println!("  Until a device takes up its invitation it is in the old fleet, which no");
        println!("  longer has anybody to talk to. Nothing is lost by taking a week over it.");
    }
    Ok(())
}

/// Check a passphrase against the fleet this node belongs to.
pub fn verify(dir: Option<PathBuf>) -> Result<()> {
    let dir = state_dir(dir);
    let mut state = fleet::require(&dir)?;

    let key = ask("Fleet passphrase: ")?;
    if !key.is(state.fleet) {
        // Deliberately says nothing about which part was wrong: there is nothing useful to
        // say, and the only honest answer is that this phrase is a different fleet.
        bail!(
            "that is not this fleet's passphrase — it derives {}, this node belongs to {}",
            key.id().short(),
            state.fleet.short()
        );
    }

    let at = now();
    state.record(PassphraseUse::Verify, at);
    fleet::save(&dir, &state)?;
    let (node, name) = me(&state);
    record_event(
        &dir,
        offload_core::FleetEvent::PassphraseUsed {
            what: "check it against the fleet".into(),
            node,
            name,
        },
    );

    println!("That is the passphrase for fleet {}.", state.fleet);
    println!("Verified, and nothing else changed.");
    Ok(())
}

/// Add a grant to this node's own certificate.
pub fn grant(dir: Option<PathBuf>, grant: Grant, hardware_key: bool) -> Result<()> {
    let dir = state_dir(dir);
    let mut state = fleet::require(&dir)?;

    // Read before the passphrase is asked for, so a missing key costs nobody a trip to the drawer.
    let issuer_key = if hardware_key {
        if grant != Grant::Approve {
            bail!(
                "--hardware-key names the key an approver approves with, so it goes with `approve`"
            );
        }
        let Some(info) = offload_node::approval_key::read(&dir) else {
            bail!(
                "no approval key in {} — the host app writes {} when it has made one",
                dir.display(),
                offload_node::approval_key::KEY_FILE
            );
        };
        println!("Approval key {}.", info.level());
        Some(info.issuer_key().map_err(|e| anyhow::anyhow!(e))?)
    } else {
        None
    };
    let key = ask("Fleet passphrase: ")?;
    let at = now();
    state.add_grant(&key, grant, at, issuer_key)?;
    fleet::save(&dir, &state)?;
    let (node, name) = me(&state);
    record_event(
        &dir,
        offload_core::FleetEvent::PassphraseUsed {
            what: format!("grant {grant} to this device"),
            node,
            name,
        },
    );

    println!("Granted {grant} to {}.", state.membership.member.short());
    println!("  grants      {}", grant_list(&state, at));
    if let Some(delegation) = state
        .approver
        .as_ref()
        .filter(|d| state.membership.grants.contains(&Grant::Approve) && d.issued_at == at)
    {
        // Said because it is the half the grant alone never did: the delegation is what lets
        // this device actually enrol and re-approve, and its year is renewed the same way.
        println!(
            "  approver    may enrol and re-approve others for {} days{} — `offload grant approve`",
            days(delegation.expires_at.saturating_sub(at)),
            if delegation.issuer_key.is_node() {
                ", with this node's own key"
            } else {
                ", with the key in secure hardware"
            }
        );
        println!("              again, with the passphrase, renews that and this node's approval");
    }
    if let Some(until) = state.membership.probation_until(at) {
        // Probation is a delay, not a refusal, and it is worth explaining rather than
        // leaving somebody to wonder why the node still refuses runs.
        println!(
            "  host-runs   dormant for {} (probation)",
            offload_core::fleet::dormant_for(until.saturating_sub(at))
        );
    }
    // Worth saying because the opposite was true until recently, and a restart somebody does
    // once out of superstition is a thing they go on doing for years.
    println!();
    println!("  A daemon running on this device picks the new certificate up within a second.");
    println!("  No restart.");
    Ok(())
}

/// Revoke a member. Fleet-signed, and immediate on this device.
pub fn revoke(dir: Option<PathBuf>, node: &str) -> Result<()> {
    let dir = state_dir(dir);
    let mut state = fleet::require(&dir)?;
    let member = resolve_member(&dir, &state, node)?;
    // Before the passphrase, not after: a prefix is a convenience and the operator is entitled
    // to see which device it landed on while there is still nothing to undo.
    if node.trim().len() < 64 {
        println!("{member}");
        if let Some(name) = name_of(&dir, &state, member) {
            println!("  that is {name}");
        }
    }

    // Asked before the revocation is filed, because filing it *is* a record of that device:
    // `witnessed` reads the revocation list too, so a stranger revoked a moment ago has a name
    // here and the sentence below went unprinted. Found by running the arm rather than by
    // reading the diff.
    let met_before = name_of(&dir, &state, member).is_some();

    let key = ask("Fleet passphrase: ")?;
    let at = now();
    let outcome = state.revoke(&key, member, at)?;

    // Exit 0 either way — the device is out, which is what was typed — but only a call that
    // wrote something may describe what it wrote. A second `offload revoke` used to reprint the
    // whole paragraph about the daemon hanging up and the fleet being told, having done neither.
    if let Revoked::Already(existing) = &outcome {
        println!("{member} was already revoked, {} ago.", ago(at, existing));
        if member == state.membership.member {
            println!("  That is this node.");
        }
        println!();
        println!("  Nothing was written and nothing was sent: the revocation the fleet holds is");
        println!("  the first one, and it is still in force. `offload fleet` lists it.");
        return Ok(());
    }

    fleet::save(&dir, &state)?;
    let (node, name) = me(&state);
    record_event(
        &dir,
        offload_core::FleetEvent::PassphraseUsed {
            what: format!("revoke {}", member.short()),
            node,
            name,
        },
    );

    println!("Revoked {member}.");
    if member == state.membership.member {
        println!("  That was this node. It has just removed itself from its own fleet.");
    } else if !met_before {
        // Legal, and worth a sentence rather than silence: an id typed from somewhere other
        // than this machine's own records is exactly how somebody revokes a device they cannot
        // reach — and it is also what a mistyped id looks like. `offload when` has warned on the
        // same predicate since ADR-0032, and `offload every` was found doing it in silence.
        println!("  This node has no record of ever meeting that device. That is a fine thing");
        println!("  to revoke — an id can come from the device itself — but check it: nothing");
        println!("  here could have told you the id was wrong.");
    }
    println!();
    println!("  Recorded here and refused immediately: a daemon running on this device picks");
    println!("  it up within a second and hangs up on that node. Other machines learn of it by");
    println!("  gossip, or when that device's certificate lapses.");
    println!();
    println!("  Revocation stops future participation. It does not un-give what that device");
    println!("  already holds: repositories, blobs and any credentials of its own. Rotate");
    println!("  those separately.");
    Ok(())
}

/// How long ago a revocation was filed, for the sentence that says nothing happened this time.
fn ago(now: offload_core::Millis, revocation: &offload_core::Revocation) -> String {
    let span = now.saturating_sub(revocation.revoked_at);
    // The same rounding `fleet` uses for a certificate: under a day, say the duration.
    if span.as_secs() < 86_400 {
        format!("{span}")
    } else {
        format!("{} days", (span.as_secs() + 43_200) / 86_400)
    }
}

/// What this node knows about its fleet.
pub fn show(dir: Option<PathBuf>) -> Result<()> {
    let dir = state_dir(dir);
    let Some(state) = fleet::load(&dir)? else {
        println!("This node has not joined a fleet.");
        println!("  `offload init` founds one here, `offload join --passphrase` enrols into one.");
        println!("  Until then it runs as a fleet of one: local runs work, nothing is shared.");
        return Ok(());
    };
    let at = now();

    println!("fleet    {}", state.fleet);
    println!(
        "node     {} ({})",
        state.membership.member, state.membership.name
    );
    println!("grants   {}", grant_list(&state, at));
    let usable = state.check(state.membership.member, at);
    match &usable {
        Ok(()) => println!(
            "cert     valid, expires in {} days (serial {})",
            days(state.membership.expires_at.saturating_sub(at)),
            state.membership.serial
        ),
        Err(e) => println!("cert     UNUSABLE: {e}"),
    }
    // Only under a certificate that is going to start working. Probation is a wait with an end,
    // and printing its countdown directly beneath `UNUSABLE: … has been revoked` promises that
    // host-runs comes back in fourteen minutes, which nothing will make happen. Same rule as the
    // removed schedule whose `home` line still said it fired from here: a line computed from one
    // field has to be told what the line above it decided.
    if usable.is_ok() {
        if let Some(until) = state.membership.probation_until(at) {
            println!(
                "         host-runs dormant for {} (probation of {} minutes)",
                offload_core::fleet::dormant_for(until.saturating_sub(at)),
                PROBATION.as_secs() / 60
            );
        }
    }
    // The approval's year, beside the certificate's month (ADR-0069 §3): renewal moves the second
    // and never the first, so a device renewed yesterday can still be days from needing a person.
    if usable.is_ok() {
        println!(
            "approved {} days ago, lasts until a person re-approves it or {} more days pass",
            days(at.saturating_sub(state.membership.approved_at())),
            days(state.membership.approval_expires_at().saturating_sub(at))
        );
    }
    // Both halves, in the order `FleetState::invite` asks for them — the grant in the
    // certificate, then the delegation that proves it. This line used to read `approver`
    // alone, which is neither what the door checks nor what `health` counts, and the three
    // answers agreed only because nothing had ever desynchronised them. Something can: a
    // certificate whose `approve` grant is gone leaves the delegation behind, and this screen
    // then said `this node may enrol others` four lines above a note saying it is not an
    // approver, with `offload invite` refusing. The line that is wrong is the one that reads
    // like the answer.
    println!(
        "approver {}",
        match (
            state.membership.granted(Grant::Approve, at),
            state.approver.is_some(),
        ) {
            (true, true) => "this node may enrol others".to_string(),
            (true, false) =>
                "no — this node is granted `approve` with no delegation to prove it".to_string(),
            (false, true) => format!(
                "no — this node holds an approver delegation its certificate no longer grants \
                 (`offload grant approve` on {} restores it)",
                state.membership.member.short()
            ),
            (false, false) => "no — this node cannot enrol others".to_string(),
        }
    );
    match (state.last_verified(), state.passphrase_uses.is_empty()) {
        (Some(when), _) => println!(
            "verified passphrase last checked {} days ago",
            days(at.saturating_sub(when))
        ),
        (None, false) => {
            println!("verified never — run `offload verify` while you still have the phrase");
        }
        // An invited device has never seen the passphrase and has no business being told to go
        // and find it: the whole point of the invite path is that the drawer stayed shut.
        (None, true) => println!("verified n/a — this device was invited, not enrolled by phrase"),
    }
    if !state.revocations.is_empty() {
        println!("revoked  {}", state.revocations.len());
        for revocation in &state.revocations {
            println!("         {}", revocation.member);
        }
    }

    // The nags ADR-0012 asks for. Silent drift is how a fleet ends up with one approver and a
    // recovery secret nobody has ever read back.
    //
    // Counted from this node's own certificate alone, because that is all this command can see:
    // it runs with no daemon and no network on purpose, and a peer's grants live on a
    // certificate that arrives at a handshake. `offload status` asks the daemon and can count
    // the ones it has met — which is why the sentence here points at it rather than guessing.
    // `Met::NotAsked`, which is the whole of what this command may claim: it runs with no daemon
    // and no network on purpose, so it has met nobody and knows nothing about whether anybody
    // else is an approver. Passing `&[]` said the opposite — a laptop gossiping with an approver
    // every second was told there was none in the fleet and sent to fetch the passphrase, and the
    // caveat that would have softened it was printed only on nodes that *are* approvers, which is
    // exactly the wrong half. The distinction lives in `health` now, so the CLI cannot re-make it.
    for note in fleet::health(&state, fleet::Met::NotAsked, at).notes {
        println!();
        println!("note: {note}");
    }
    Ok(())
}

fn grant_list(state: &FleetState, at: Millis) -> String {
    let effective = state.grants(at);
    if effective.is_empty() {
        return "none".to_string();
    }
    effective
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

/// Write down something that happened to this fleet, so somebody hears about it.
///
/// ADR-0012 mitigation 4: every enrolment is announced and recorded, and with no per-join
/// approval that announcement is the compensating control the whole posture rests on. The
/// daemon's delivery plane fans it out; this is the durable half, and it is the half that
/// survives a missed notification.
///
/// Written *here*, in the CLI, because these commands run with no daemon by design — an alarm
/// that only fired when a daemon happened to be up would be missing on exactly the machine
/// somebody has just walked up to. SQLite is multi-process, so a running daemon picks the row
/// up on its next pass.
///
/// Best effort, and loudly so. A membership command must not fail because a log could not be
/// written: refusing to enrol a device over a database error would be a worse outcome than an
/// enrolment nobody was told about, and the person is standing right there either way.
fn record_event(dir: &std::path::Path, kind: offload_core::FleetEvent) {
    let at = now();
    match offload_store::Store::open(dir)
        .and_then(|store| store.append_fleet_event(&offload_core::FleetLogEvent::new(at.0, kind)))
    {
        Ok(_) => {}
        Err(e) => eprintln!(
            "warning: this happened, but could not be written to the fleet log ({e}). \
             Nothing will be notified about it."
        ),
    }
}

/// This node's own identity for a fleet event, which is a pair: the id is what a command takes
/// and the name is what a person reads at three in the morning.
fn me(state: &FleetState) -> (NodeId, String) {
    (state.membership.member, state.membership.name.clone())
}

/// Read a passphrase and derive the fleet key from it.
///
/// No echo on a terminal, one line from stdin otherwise — so scripts and tests can drive it
/// without a pty. Deriving here rather than returning the phrase keeps the secret's lifetime
/// as short as the function.
fn ask(prompt: &str) -> Result<FleetKey> {
    let raw = zeroize::Zeroizing::new(if std::io::stdin().is_terminal() {
        rpassword::prompt_password(prompt).context("reading passphrase")?
    } else {
        let mut line = String::new();
        std::io::BufRead::read_line(&mut std::io::stdin().lock(), &mut line)
            .context("reading passphrase from stdin")?;
        line
    });
    let passphrase = Passphrase::typed(&raw)?;
    if std::io::stdin().is_terminal() {
        // A second of silence with no explanation reads as a hang.
        eprintln!("deriving the fleet key…");
    }
    Ok(passphrase.derive()?)
}

/// Parse `--grant`'s value. Kept next to the commands rather than in `offload-core`: these
/// are CLI spellings, and the domain type is not obliged to know them.
pub fn parse_grant(text: &str) -> Result<Grant, String> {
    match text.trim().to_lowercase().replace('_', "-").as_str() {
        "submit" => Ok(Grant::Submit),
        "deliver" => Ok(Grant::Deliver),
        "host-runs" | "hostruns" => Ok(Grant::HostRuns),
        "approve" => Ok(Grant::Approve),
        other => Err(format!(
            "unknown grant `{other}` — expected submit, deliver, host-runs or approve"
        )),
    }
}

/// Rounded to the nearest day rather than truncated: a certificate with 29 days and 23 hours
/// left has 30 days left, and saying 29 makes a fresh one look a day old.
fn days(span: Millis) -> u64 {
    (span.as_secs() + 43_200) / 86_400
}

/// A device this node has recorded as a member: who it is, what to call it, what it was granted.
type KnownMember = (NodeId, String, Vec<String>);

/// Turn what somebody typed into the node it names, accepting a prefix.
///
/// **The command that evicts a device asked for a node id nothing prints.** `offload revoke` and
/// `offload rekey --evict` both take 64 hex characters; `offload nodes` and `offload nodes
/// --history` — the two commands that list peers — print **twelve**, and the only places a
/// peer's id appears in full are `offload id` *on that device* and, afterwards, the `revoked`
/// list. So revoking a device meant going to it, and the device you cannot go to is the one
/// revocation exists for. Measured on two meshed daemons: no command on the node doing the
/// revoking showed the other's full id until it had already been revoked.
///
/// Same rule as the ambiguous-run-id refusal one tier over — **an error that tells somebody to
/// try harder must be satisfiable from the screen** — reached from the other side: here it was
/// the *argument* that the screen could not supply.
///
/// A full 64 characters is still taken as an id and is not resolved against anything, so a
/// device this machine has never met can be revoked from an id read off its own screen. Anything
/// shorter is matched against what this device **witnessed** — the enrolment log `offload nodes
/// --history` prints, plus this node itself and anybody already revoked. No match and more than
/// one match are both refused, and the ambiguous refusal names the candidates in full, because
/// naming them is the whole point of the rule above.
fn resolve_member(dir: &std::path::Path, state: &FleetState, typed: &str) -> Result<NodeId> {
    // The store read is the only part of this that needs a filesystem, so it is the only part
    // outside `resolve_against` — which is where every rule lives and is therefore what a test
    // can reach. A resolver whose refusals are only observable by running the command is a
    // resolver whose refusals nothing checks.
    let witnessed = |_: ()| witnessed(dir, state);
    resolve_against(typed, witnessed)
}

/// The whole of `resolve_member` except reading the fleet log, which is handed in.
///
/// The candidate list is behind a closure so the store is not opened for a full 64-character id,
/// which needs no candidates at all — and so a test can supply one.
fn resolve_against(
    typed: &str,
    candidates: impl FnOnce(()) -> (Vec<(NodeId, String)>, bool),
) -> Result<NodeId> {
    let needle = typed.trim().to_ascii_lowercase();
    if needle.is_empty() {
        bail!("name the device to act on — a node id, or enough of one to be unambiguous");
    }
    if !needle.chars().all(|c| c.is_ascii_hexdigit()) {
        bail!("{typed} is not a node id — node ids are hex, and `offload nodes` lists them");
    }
    if needle.len() > 64 {
        bail!("{typed} is longer than a node id (64 hex characters)");
    }
    if needle.len() == 64 {
        return needle
            .parse::<NodeId>()
            .with_context(|| format!("{typed} is not a node id (64 hex characters)"));
    }

    let (candidates, log_read) = candidates(());
    let matched: Vec<&(NodeId, String)> = candidates
        .iter()
        .filter(|(node, _)| node.to_string().starts_with(&needle))
        .collect();
    match matched.as_slice() {
        [(node, _)] => Ok(*node),
        [] if log_read => bail!(
            "no device this node has a record of starts with `{needle}` — `offload nodes --history` \
             lists what it met. A device this one never met has to be named by its whole id, \
             which `offload id` prints on the device itself."
        ),
        [] => bail!(
            "could not read this node's fleet log, so `{needle}` cannot be matched against the \
             devices it met. Name the device by its whole id — `offload id` prints it there."
        ),
        _ => {
            let mut lines = String::new();
            for (node, name) in &matched {
                lines.push_str(&format!("\n  {node}  {name}"));
            }
            bail!("`{needle}` names more than one device this node has a record of:{lines}");
        }
    }
}

/// What this machine calls a device, if it has a name for it.
fn name_of(dir: &std::path::Path, state: &FleetState, node: NodeId) -> Option<String> {
    witnessed(dir, state)
        .0
        .into_iter()
        .find(|(known, _)| *known == node)
        .map(|(_, name)| name)
}

/// Every device this machine has a record of, with something to call it.
///
/// The enrolment log first — that is what `offload nodes --history` reads and it is a record of
/// what this machine *witnessed* — then this node itself and anybody already revoked, neither of
/// which is necessarily in it. The `bool` says whether the log could be read at all, because
/// "nothing matched" and "there was nothing to match against" are two different things to tell
/// somebody, and a resolver that conflated them would send them to look for a typo in an id that
/// was right.
fn witnessed(dir: &std::path::Path, state: &FleetState) -> (Vec<(NodeId, String)>, bool) {
    let (mut out, log_read) = match offload_store::Store::open(dir).and_then(|s| s.known_members())
    {
        Ok(known) => (
            known
                .into_iter()
                .map(|(node, name, _)| (node, name))
                .collect::<Vec<_>>(),
            true,
        ),
        Err(_) => (Vec::new(), false),
    };
    let mut add = |node: NodeId, name: String| {
        if !out.iter().any(|(known, _)| *known == node) {
            out.push((node, name));
        }
    };
    add(state.membership.member, state.membership.name.clone());
    for revocation in &state.revocations {
        add(revocation.member, "revoked already".to_string());
    }
    (out, log_read)
}

/// Split the devices a rekey would re-invite from the ones it must not.
///
/// `known_members` reads the *enrolment* log, which is a record of what this machine witnessed
/// and has no opinion about what happened afterwards — so a rekey handed a revoked device a fresh
/// certificate, into a new fleet whose revocation list starts empty. The strongest eviction this
/// program has, silently undoing the weaker one. The rule is `renew_for`'s, one command over: a
/// revoked member is not handed fresh papers.
///
/// A function of its own because the path around it needs a passphrase from a terminal, and this
/// is the whole of what there is to get wrong.
fn split_revoked(
    state: &FleetState,
    known: Vec<KnownMember>,
) -> (Vec<KnownMember>, Vec<KnownMember>) {
    known
        .into_iter()
        .partition(|(node, _, _)| state.is_revoked(*node))
}

/// ADR-0069 §3's `offload reapprove`: a person's decision, on an approver, to start another year
/// for the members named.
///
/// A decision rather than a certificate. The member is re-approved when it next asks, from the
/// certificate it carries then — so nobody has to visit it, which is the whole reason for the
/// command: twenty machines are one command here and nothing on any of them. With no names it
/// lists what is due, from the daemon (which holds every peer's certificate from its handshakes);
/// `--due` decides for exactly that list, printed as it is decided.
pub async fn reapprove(
    dir: Option<PathBuf>,
    socket: &Path,
    names: Vec<String>,
    due: bool,
) -> Result<()> {
    let dir = state_dir(dir);
    let Some(mut state) = fleet::load(&dir)? else {
        anyhow::bail!("this node has not joined a fleet, so it has nobody to re-approve");
    };
    let at = now();
    let me = state.membership.member;

    let listed: Option<Vec<fleet::ApprovalDue>> = if names.is_empty() {
        match crate::client::request(socket, &offload_node::api::Request::Status).await {
            Ok(responses) => match crate::client::expect_one(responses)? {
                offload_node::api::Response::Status(s) => {
                    Some(s.fleet.map(|f| f.reapproval_due).unwrap_or_default())
                }
                _ => anyhow::bail!("unexpected response to status"),
            },
            Err(e) => anyhow::bail!(
                "the daemon holds the peers' certificates and could not be asked ({e}). Name the \
                 devices instead: `offload reapprove <node>…`"
            ),
        }
    } else {
        None
    };

    let targets: Vec<(NodeId, String)> = match (&listed, due) {
        (Some(list), false) => {
            if list.is_empty() {
                println!(
                    "No membership this node has met is within {} days of its approval's year.",
                    days(REAPPROVAL_WINDOW)
                );
                return Ok(());
            }
            println!(
                "Approvals running out within {} days, among the devices this node has met:",
                days(REAPPROVAL_WINDOW)
            );
            println!();
            for row in list {
                println!(
                    "  {:<20} {}  in {} day(s){}",
                    row.name,
                    row.node.short(),
                    row.days,
                    if row.node == me {
                        "  — this node: another approver has to do it"
                    } else if row.decided {
                        "  — decided here, waiting for it to ask"
                    } else {
                        ""
                    }
                );
            }
            println!();
            let others: Vec<String> = list
                .iter()
                .filter(|row| row.node != me)
                // Whole ids: a prefix resolves against this node's enrolment log, and a device
                // invited from elsewhere is not in it, so a short id printed here could be one
                // the command it suggests then refuses.
                .map(|row| row.node.to_string())
                .collect();
            if !others.is_empty() {
                println!(
                    "  `offload reapprove --due` decides for every one of these but this node —"
                );
                println!("  the same as `offload reapprove {}`.", others.join(" "));
            }
            return Ok(());
        }
        (Some(list), true) => list
            .iter()
            .filter(|row| row.node != me)
            .map(|row| (row.node, row.name.clone()))
            .collect(),
        (None, _) => {
            let mut out = Vec::new();
            for typed in &names {
                let node = resolve_member(&dir, &state, typed)?;
                let name = name_of(&dir, &state, node).unwrap_or_else(|| typed.clone());
                out.push((node, name));
            }
            out
        }
    };
    if targets.is_empty() {
        println!(
            "Nothing to decide: the only approval running out is this node's own, and no node"
        );
        println!("re-approves itself — run this on another approver, or use the passphrase.");
        return Ok(());
    }

    let mut decided = Vec::new();
    for (node, name) in &targets {
        match state.decide_reapproval(me, *node, name, at) {
            Ok(()) => decided.push((*node, name.clone())),
            Err(reason) => println!("not decided  {name} ({}): {reason}", node.short()),
        }
    }
    if decided.is_empty() {
        anyhow::bail!("nothing was decided");
    }
    fleet::save(&dir, &state)?;
    for (node, name) in &decided {
        println!("re-approve   {name} ({})", node.short());
    }
    println!();
    // From the constants the daemon decides by, not restated: a walk build that shortened them
    // printed "every fifteen minutes" and "30 days" beside a 350-minute window.
    println!("  Each is re-approved the next time it asks this node, which a device in its");
    println!(
        "  approval's last {} days does every {} minutes while it is up. The decision",
        days(REAPPROVAL_WINDOW),
        offload_node::mesh::RENEW_RETRY.as_secs() / 60
    );
    println!(
        "  stands for {} days. A daemon running here picks it up within a second.",
        days(REAPPROVAL_WINDOW)
    );
    Ok(())
}

/// The bytes a certificate's signer signs, for a host holding a hardware approval key (ADR-0069
/// §4): the unsigned certificate on stdin, hex on stdout. The host computes what it signs with
/// this program rather than a second implementation of the format, and builds its prompt from
/// the same certificate, so what a person confirms and what is signed cannot differ.
pub fn signing_bytes() -> Result<()> {
    let mut text = String::new();
    std::io::Read::read_to_string(&mut std::io::stdin(), &mut text)?;
    let cert: offload_core::MembershipCert =
        serde_json::from_str(&text).context("reading a certificate from stdin")?;
    println!("{}", hex::encode(cert.signing_bytes()));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fleet_of(ids: &[(u8, &str)]) -> (Vec<(NodeId, String)>, bool) {
        (
            ids.iter()
                .map(|(b, name)| (NodeId::from_bytes([*b; 32]), (*name).to_string()))
                .collect(),
            true,
        )
    }

    #[test]
    fn a_device_is_named_by_as_much_of_its_id_as_the_screen_shows() {
        // `offload nodes` prints twelve characters and `offload revoke` wanted sixty-four, so
        // the one command that evicts a device took an argument nothing on the operator's screen
        // could supply. Measured on two meshed daemons: until bravo had already been revoked, no
        // command on alpha printed bravo's id in full.
        let bravo = NodeId::from_bytes([0x75; 32]);
        let known = |_| fleet_of(&[(0x75, "bravo"), (0xcc, "alpha")]);
        assert_eq!(
            resolve_against(&bravo.to_string()[..12], known).expect("a prefix from the screen"),
            bravo
        );
        // …and the whole id still works without consulting anything, which is how a device this
        // machine has never met is named at all.
        let stranger = NodeId::from_bytes([0x11; 32]);
        assert_eq!(
            resolve_against(&stranger.to_string(), |_| fleet_of(&[])).expect("a full id"),
            stranger
        );
        // Case is not part of an id.
        assert_eq!(
            resolve_against(&bravo.to_string()[..12].to_uppercase(), known).expect("uppercase"),
            bravo
        );
    }

    #[test]
    fn an_ambiguous_prefix_names_the_candidates_it_could_not_choose_between() {
        // The rule this is here for: an error telling somebody to be more specific has to be
        // satisfiable from the screen, so it prints the ids in full rather than sending them
        // back to a listing that shows twelve characters.
        let a = NodeId::from_bytes([0x75; 32]);
        let mut bytes = [0x75; 32];
        bytes[1] = 0x2f;
        let b = NodeId::from_bytes(bytes);
        let known = |_| {
            (
                vec![(a, "bravo".to_string()), (b, "charlie".to_string())],
                true,
            )
        };
        let err = resolve_against("75", known).expect_err("two devices start with 75");
        let text = format!("{err}");
        assert!(text.contains(&a.to_string()), "{text}");
        assert!(text.contains(&b.to_string()), "{text}");
        assert!(text.contains("bravo") && text.contains("charlie"), "{text}");
    }

    #[test]
    fn a_prefix_matching_nothing_is_refused_rather_than_taken_literally() {
        // The reason a prefix may not fall through to "revoke whatever that is": a typo in a
        // long id would otherwise file a revocation against a device that does not exist, and
        // the only sign would be a count going up.
        let known = |_| fleet_of(&[(0x75, "bravo")]);
        let err = resolve_against("abcdef", known).expect_err("nothing starts with abcdef");
        assert!(format!("{err}").contains("offload nodes --history"));

        // And "nothing matched" must not be said when the truth is "nothing to match against".
        let unreadable = |_| (Vec::new(), false);
        let err = resolve_against("abcdef", unreadable).expect_err("no log");
        assert!(format!("{err}").contains("could not read this node's fleet log"));
    }

    #[test]
    fn what_is_not_a_node_id_is_refused_before_anything_is_consulted() {
        let nothing = |_| -> (Vec<(NodeId, String)>, bool) { panic!("must not be consulted") };
        for typed in ["", "   ", "bravo", "75%", "75_", &"0".repeat(70)] {
            assert!(
                resolve_against(typed, nothing).is_err(),
                "accepted `{typed}`"
            );
        }
    }

    #[test]
    fn a_rekey_does_not_re_invite_the_device_it_threw_out() {
        // Measured: `offload revoke <gamma>` and then `offload rekey` printed gamma a fresh
        // invitation, grants and all — because the member list comes from the enrolment log,
        // which records what this machine saw and nothing about what happened next. The new
        // fleet's revocation list starts empty, so the eviction was gone with it.
        let key = FleetKey::derive("abacus zoom yo-yo").expect("derive");
        let me = NodeId::from_bytes([1; 32]);
        let out = NodeId::from_bytes([2; 32]);
        let staying = NodeId::from_bytes([3; 32]);
        let mut state = FleetState::found(&key, me, "desktop", offload_core::Millis(1));
        state
            .revoke(&key, out, offload_core::Millis(2))
            .expect("revoke");

        let (revoked, members) = split_revoked(
            &state,
            vec![
                (out, "gamma".into(), vec!["submit".into()]),
                (staying, "phone".into(), vec!["submit".into()]),
            ],
        );
        assert_eq!(revoked.len(), 1);
        assert_eq!(revoked[0].0, out);
        assert_eq!(members.len(), 1);
        assert_eq!(members[0].0, staying, "everybody else still gets papers");
    }

    #[test]
    fn grants_parse_the_way_they_print() {
        // `offload fleet` prints `host-runs`, so `offload grant host-runs` must take it —
        // an asymmetry here is the kind of thing nobody notices until they are pasting.
        for grant in [
            Grant::Submit,
            Grant::Deliver,
            Grant::HostRuns,
            Grant::Approve,
        ] {
            assert_eq!(parse_grant(&grant.to_string()), Ok(grant));
        }
        assert_eq!(parse_grant("HOST_RUNS"), Ok(Grant::HostRuns));
        assert!(parse_grant("root").is_err());
    }
}
