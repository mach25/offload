//! A compact constraint syntax for the command line.
//!
//! Constraints are a tree, but typing a tree is miserable, so the CLI accepts a
//! comma-separated list of clauses that are ANDed together. That covers essentially every
//! constraint anyone writes by hand.
//!
//! **What it does not cover has no surface at all, and this used to say otherwise**: "anything
//! genuinely tree-shaped goes in a run spec file", of which there is none — that sentence was the
//! only mention of one anywhere in the CLI. So negation is unexpressible, and
//! `Constraint::Not` is a variant no operator can produce: handled correctly everywhere
//! (`matches`, `explain`, `failures` — session fifteen's property test found a real bug in the
//! last of those, for `Not` specifically), reachable by nobody.
//!
//! Left that way deliberately rather than deleted. `Constraint` is the domain model and a boolean
//! tree is what it is; the missing thing is a *surface*, and inventing one here — `!mains`, or an
//! `any(...)` — is a decision about a syntax people will have to live with, not a patch. What was
//! worth fixing now is the sentence, because naming an escape hatch that does not exist is how a
//! gap stops being visible.
//!
//! ```text
//! agent=claude-code            agent installed and authenticated
//! agent:claude-code>=2.9.0     minimum agent version
//! model=claude-opus-5          agent can reach this model
//! cores>=8                     minimum cores
//! mem>=16G                     minimum memory (K/M/G/T suffixes, default MB)
//! disk>=50G                    minimum free disk
//! os=linux  arch=aarch64       exact match
//! class=desktop                device class
//! stability>=stable            ephemeral | transient | stable
//! toolchain=rust               toolchain present
//! toolchain:rust>=1.80         minimum toolchain version
//! tag=gpu:cuda                 free-form tag
//! label:location=home          operator-assigned label
//! mains                        on mains power
//! unmetered                    not on a metered network
//! here                         the node that takes the submission (ADR-0063 §3)
//! node=desktop                 the member with that name
//! ```
//!
//! The last two are about *which machine*, not about what it has, so they are carried beside the
//! tree as [`Wanted::nodes`] and resolved by the daemon, which is the one that knows the ids. They
//! are what `--prefer` is mostly for; under `--require` they are a pin.

use offload_core::capability::{AgentKind, Arch, DeviceClass, Os, Stability};
use offload_core::{Constraint, NodeRef, Wanted};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ParseError {
    /// Nothing was typed where a constraint was expected.
    ///
    /// **Not a constraint that matches everything**, which is what `parse("")` used to return:
    /// `clauses.is_empty()` answered `Constraint::Always`, so `offload match ""` printed *"this
    /// device satisfies the constraint"* and exited **0** — a gate that opens on an unset shell
    /// variable, in a command whose own help says to compose it in scripts. The same root as
    /// session eighty-two's empty run id, two commands over.
    ///
    /// It was also the variant with no reachable constructor: `clause()` returned it for an empty
    /// string and `parse` filtered every empty clause out before calling `clause`. It has one now.
    #[error("no constraint was given — e.g. `cores>=8, mem>=16G`")]
    Empty,
    #[error("unknown clause `{0}`")]
    Unknown(String),
    #[error("clause `{clause}` needs a value like `{example}`")]
    NeedsValue { clause: String, example: String },
    /// A bare word with no operator, which is either a clause missing its value or not a clause.
    ///
    /// It cannot tell those apart without a second copy of the clause-name list, which lives in
    /// one `match` in `clause()`. So it says what is true of both and names the two clauses that
    /// legitimately stand alone — which is the fact that distinguishes them. It used to answer
    /// *"clause `nonsense` needs a value like `cores>=8`"*, telling somebody to put a value on a
    /// clause that does not exist.
    #[error(
        "`{0}` is not a clause on its own — only `mains`, `unmetered` and `here` are. \
         Give it a value, like `cores>=8` or `os=linux`"
    )]
    Bare(String),
    /// A clause with an operator and nothing after it.
    ///
    /// Its own variant because the operator typed a clause they meant: `toolchain:rust>=` is not
    /// *"unknown"* and does not need an example of the shape — it needs the version they left
    /// off. It was accepted silently for every clause whose value is a string, so
    /// `toolchain:rust>=` quietly meant **any** version and `tag=` meant a tag named nothing,
    /// while `cores>=` was refused by `number()`. One rule now, in `split_op`, where every
    /// clause passes.
    #[error("clause `{0}` was given no value")]
    NoValue(String),
    #[error("`{value}` is not a valid {kind}")]
    BadValue { value: String, kind: &'static str },
    #[error("`{0}` is not a number")]
    NotANumber(String),
}

/// Parse a comma-separated clause list into a conjunction, with any node clauses left for the
/// daemon to look up.
pub fn parse(input: &str) -> Result<Wanted, ParseError> {
    let clauses: Vec<&str> = input
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();

    if clauses.is_empty() {
        return Err(ParseError::Empty);
    }

    let mut tree = Vec::new();
    let mut nodes = Vec::new();
    for c in clauses {
        match node_ref(c)? {
            Some(node) => nodes.push(node),
            None => tree.push(clause(c)?),
        }
    }
    Ok(Wanted {
        // Only node clauses leaves nothing for the tree to say, which is `Always` rather than an
        // empty `All` so that a spec reads the same however the preference was spelled.
        clauses: if tree.is_empty() {
            Constraint::Always
        } else {
            Constraint::All(tree)
        },
        nodes,
    })
}

/// `here` or `node=<name>`, or `None` for a clause about capabilities.
fn node_ref(input: &str) -> Result<Option<NodeRef>, ParseError> {
    if input == "here" {
        return Ok(Some(NodeRef::Here));
    }
    match input.split_once('=') {
        Some(("node", "")) => Err(ParseError::NoValue(input.to_string())),
        Some(("node", name)) => Ok(Some(NodeRef::Named(name.trim().to_string()))),
        _ => Ok(None),
    }
}

fn clause(input: &str) -> Result<Constraint, ParseError> {
    // Bare flags first. (There is no empty-input check here: `parse` filters empty clauses out
    // before calling this, which is what made `ParseError::Empty` unreachable — it belongs to
    // the whole expression, and that is where it is raised.)
    match input {
        "mains" => return Ok(Constraint::OnMains),
        "unmetered" => return Ok(Constraint::UnmeteredNetwork),
        // Reached only if `node_ref` was bypassed, which `parse` never does.
        "here" => return Err(ParseError::Unknown(input.to_string())),
        _ => {}
    }

    // `key:qualifier<op>value` — the two-part forms.
    if let Some((head, rest)) = input.split_once(':') {
        // `tag=` and `label:` both use colons, so disambiguate on the head.
        match head {
            "agent" | "toolchain" | "label" => return qualified(head, rest),
            _ => {}
        }
    }

    let (key, op, value) = split_op(input)?;

    match (key, op) {
        ("cores", Op::AtLeast) => Ok(Constraint::MinCores(number(value)?)),
        ("mem" | "memory", Op::AtLeast) => Ok(Constraint::MinMemoryMb(size_mb(value)?)),
        ("disk", Op::AtLeast) => Ok(Constraint::MinDiskFreeMb(size_mb(value)?)),
        ("stability", Op::AtLeast | Op::Eq) => Ok(Constraint::MinStability(stability(value)?)),
        ("os", Op::Eq) => Ok(Constraint::Os(os(value))),
        ("arch", Op::Eq) => Ok(Constraint::Arch(arch(value))),
        ("class", Op::Eq) => Ok(Constraint::DeviceClass(device_class(value)?)),
        ("agent", Op::Eq) => Ok(Constraint::agent_ready(agent(value), None)),
        ("model", Op::Eq) => Ok(Constraint::HasModel(
            AgentKind::ClaudeCode,
            value.to_string(),
        )),
        ("toolchain", Op::Eq) => Ok(Constraint::HasToolchain(value.to_string())),
        ("tag", Op::Eq) => Ok(Constraint::HasTag(value.to_string())),
        (other, _) => Err(ParseError::Unknown(other.to_string())),
    }
}

/// Handles `agent:claude-code>=2.9`, `toolchain:rust>=1.80`, `label:location=home`.
fn qualified(head: &str, rest: &str) -> Result<Constraint, ParseError> {
    let (name, op, value) = split_op(rest).map_err(|_| ParseError::NeedsValue {
        clause: format!("{head}:{rest}"),
        example: match head {
            "agent" => "agent:claude-code>=2.9.0".to_string(),
            "toolchain" => "toolchain:rust>=1.80".to_string(),
            _ => "label:location=home".to_string(),
        },
    })?;

    match (head, op) {
        ("agent", Op::AtLeast | Op::Eq) => {
            Ok(Constraint::MinAgentVersion(agent(name), value.to_string()))
        }
        ("toolchain", Op::AtLeast | Op::Eq) => Ok(Constraint::MinToolchainVersion(
            name.to_string(),
            value.to_string(),
        )),
        ("label", Op::Eq) => Ok(Constraint::Label(name.to_string(), value.to_string())),
        _ => Err(ParseError::Unknown(format!("{head}:{name}"))),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    Eq,
    AtLeast,
}

fn split_op(input: &str) -> Result<(&str, Op, &str), ParseError> {
    // The value side is checked **here**, where every clause passes, rather than in each arm.
    // `number()` and `size_mb()` refused an empty value and every clause taking a string did not,
    // so `toolchain:rust>=` silently meant *any version*, `tag=` meant a tag named nothing, and
    // `cores>=` was correctly refused — one rule, applied to a third of the clauses.
    fn with_value<'a>(
        input: &str,
        k: &'a str,
        op: Op,
        v: &'a str,
    ) -> Result<(&'a str, Op, &'a str), ParseError> {
        let (k, v) = (k.trim(), v.trim());
        if v.is_empty() {
            return Err(ParseError::NoValue(input.trim().to_string()));
        }
        Ok((k, op, v))
    }
    if let Some((k, v)) = input.split_once(">=") {
        return with_value(input, k, Op::AtLeast, v);
    }
    if let Some((k, v)) = input.split_once('=') {
        return with_value(input, k, Op::Eq, v);
    }
    // No operator at all. See `ParseError::Bare` for why this does not try to say which of the
    // two it is — and note that `qualified` catches its own case first and keeps `NeedsValue`,
    // where the clause *is* known and an example of its shape is the useful thing.
    Err(ParseError::Bare(input.trim().to_string()))
}

fn number(value: &str) -> Result<u32, ParseError> {
    value
        .parse()
        .map_err(|_| ParseError::NotANumber(value.to_string()))
}

/// Parse a size with an optional K/M/G/T suffix, returning megabytes. Bare numbers are MB.
fn size_mb(value: &str) -> Result<u64, ParseError> {
    let value = value.trim();
    let (digits, multiplier) = match value.chars().last() {
        Some('K' | 'k') => (&value[..value.len() - 1], 1u64),
        Some('M' | 'm') => (&value[..value.len() - 1], 1),
        Some('G' | 'g') => (&value[..value.len() - 1], 1024),
        Some('T' | 't') => (&value[..value.len() - 1], 1024 * 1024),
        _ => (value, 1),
    };
    let n: u64 = digits
        .trim()
        .parse()
        .map_err(|_| ParseError::NotANumber(value.to_string()))?;
    // Kilobytes round down to zero megabytes, which is the honest answer.
    Ok(if value.ends_with(['K', 'k']) {
        n / 1024
    } else {
        n * multiplier
    })
}

fn os(value: &str) -> Os {
    match value.to_ascii_lowercase().as_str() {
        "linux" => Os::Linux,
        "macos" | "darwin" | "mac" => Os::MacOs,
        "windows" | "win" => Os::Windows,
        "android" => Os::Android,
        "ios" => Os::Ios,
        other => Os::Other(other.to_string()),
    }
}

fn arch(value: &str) -> Arch {
    match value.to_ascii_lowercase().as_str() {
        "x86_64" | "amd64" => Arch::X86_64,
        "aarch64" | "arm64" => Arch::Aarch64,
        other => Arch::Other(other.to_string()),
    }
}

fn device_class(value: &str) -> Result<DeviceClass, ParseError> {
    Ok(match value.to_ascii_lowercase().as_str() {
        "phone" => DeviceClass::Phone,
        "tablet" => DeviceClass::Tablet,
        "laptop" => DeviceClass::Laptop,
        "desktop" => DeviceClass::Desktop,
        "server" => DeviceClass::Server,
        "vm" => DeviceClass::Vm,
        _ => {
            return Err(ParseError::BadValue {
                value: value.to_string(),
                kind: "device class",
            })
        }
    })
}

fn stability(value: &str) -> Result<Stability, ParseError> {
    Ok(match value.to_ascii_lowercase().as_str() {
        "ephemeral" => Stability::Ephemeral,
        "transient" => Stability::Transient,
        "stable" => Stability::Stable,
        _ => {
            return Err(ParseError::BadValue {
                value: value.to_string(),
                kind: "stability",
            })
        }
    })
}

fn agent(value: &str) -> AgentKind {
    match value.to_ascii_lowercase().as_str() {
        "claude-code" | "claude" | "claudecode" => AgentKind::ClaudeCode,
        other => AgentKind::Other(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The capability half, for the tests written before node clauses existed — and asserting
    /// none was produced, so a clause that started being read as a node would fail them.
    fn parse(input: &str) -> Result<Constraint, ParseError> {
        super::parse(input).map(|wanted| {
            assert!(wanted.nodes.is_empty(), "{input} named a node");
            wanted.clauses
        })
    }

    /// ADR-0063 §3: which machine is not a capability, so it is carried beside the tree for the
    /// daemon to resolve — and a capability clause beside it still lands in the tree.
    #[test]
    fn here_and_a_node_name_are_carried_for_the_daemon_to_resolve() {
        let wanted = super::parse("here").expect("parse");
        assert_eq!(wanted.clauses, Constraint::Always);
        assert_eq!(wanted.nodes, vec![NodeRef::Here]);

        let wanted = super::parse("node=desktop, cores>=8").expect("parse");
        assert_eq!(wanted.nodes, vec![NodeRef::Named("desktop".into())]);
        assert_eq!(
            wanted.clauses,
            Constraint::All(vec![Constraint::MinCores(8)])
        );

        assert_eq!(
            super::parse("node="),
            Err(ParseError::NoValue("node=".into()))
        );
        // `here` is a clause on its own and the refusal of a bare word says so.
        assert!(ParseError::Bare("x".into()).to_string().contains("here"));
    }

    #[test]
    fn parses_a_realistic_conjunction() {
        let c = parse("agent=claude-code, cores>=8, mem>=16G, toolchain:rust>=1.80, mains")
            .expect("should parse");
        let Constraint::All(parts) = c else {
            panic!("expected a conjunction");
        };
        assert_eq!(parts.len(), 5);
        assert!(parts.contains(&Constraint::MinCores(8)));
        assert!(parts.contains(&Constraint::MinMemoryMb(16 * 1024)));
        assert!(parts.contains(&Constraint::OnMains));
        assert!(parts.contains(&Constraint::MinToolchainVersion(
            "rust".into(),
            "1.80".into()
        )));
    }

    /// An empty expression is refused, and this test used to assert the opposite.
    ///
    /// It was `empty_input_matches_everything`, affirming `parse("") == Constraint::Always` — so
    /// the behaviour was deliberate enough to be written down, and changing it is changing a
    /// decision rather than fixing an oversight. What the decision missed is that `parse` has
    /// exactly **one** caller, `offload match`, whose `expr` is a required positional: there is
    /// no caller for whom "matches everything" is the useful answer, and the one there is exits
    /// **0** saying the device satisfies the constraint. Its own help says to compose it in
    /// scripts, so that is a gate that opens on an unset variable.
    ///
    /// The same root as session eighty-two's empty run id, two commands over: an empty argument
    /// is not an abbreviation of everything.
    #[test]
    fn an_empty_expression_is_not_a_constraint_that_matches_everything() {
        assert_eq!(parse(""), Err(ParseError::Empty));
        assert_eq!(parse("  , "), Err(ParseError::Empty));
        // …and a real clause beside an empty one still parses: the filter that drops the empties
        // is what makes `a, b,` legal, and it is not what was wrong.
        assert_eq!(
            parse("cores>=8,").expect("parse"),
            Constraint::All(vec![Constraint::MinCores(8)])
        );
    }

    /// A value is required wherever the syntax has one, not only where it has to be a number.
    ///
    /// `number()` and `size_mb()` refused an empty value and every clause taking a string did
    /// not — so `toolchain:rust>=` quietly meant *any* version and `tag=` meant a tag named
    /// nothing, while `cores>=` was correctly refused. One rule, in `split_op`, where every
    /// clause passes.
    #[test]
    fn a_clause_with_an_operator_and_nothing_after_it_is_refused() {
        assert_eq!(parse("tag="), Err(ParseError::NoValue("tag=".into())));
        assert_eq!(parse("cores>="), Err(ParseError::NoValue("cores>=".into())));
        assert_eq!(parse("model="), Err(ParseError::NoValue("model=".into())));
        // `qualified` catches its own first and keeps `NeedsValue`, where the clause is known and
        // an example of its shape is the more useful thing.
        assert!(matches!(
            parse("toolchain:rust>="),
            Err(ParseError::NeedsValue { .. })
        ));
    }

    /// A bare word is not told it is a clause that needs a value.
    ///
    /// It answered *"clause `nonsense` needs a value like `cores>=8`"* for anything with no
    /// operator, which sends somebody to put a value on a clause that does not exist. It cannot
    /// tell a known clause from an unknown one without a second copy of the clause-name list, so
    /// it says what is true of both and names the two that legitimately stand alone.
    #[test]
    fn a_bare_word_is_not_called_a_clause_that_needs_a_value() {
        assert_eq!(parse("nonsense"), Err(ParseError::Bare("nonsense".into())));
        assert_eq!(parse("cores"), Err(ParseError::Bare("cores".into())));
        let message = ParseError::Bare("nonsense".into()).to_string();
        assert!(message.contains("mains"), "{message}");
        assert!(message.contains("unmetered"), "{message}");
        // The two that really do stand alone still parse.
        assert_eq!(
            parse("mains").expect("parse"),
            Constraint::All(vec![Constraint::OnMains])
        );
        assert_eq!(
            parse("unmetered").expect("parse"),
            Constraint::All(vec![Constraint::UnmeteredNetwork])
        );
    }

    #[test]
    fn sizes_take_suffixes() {
        assert_eq!(size_mb("512").expect("parse"), 512);
        assert_eq!(size_mb("16G").expect("parse"), 16 * 1024);
        assert_eq!(size_mb("1T").expect("parse"), 1024 * 1024);
    }

    #[test]
    fn agent_shorthand_expands_to_installed_and_authenticated() {
        let c = parse("agent=claude").expect("parse");
        let Constraint::All(parts) = c else {
            panic!("expected conjunction")
        };
        let Constraint::All(inner) = &parts[0] else {
            panic!("expected agent_ready to be a conjunction")
        };
        assert!(inner.contains(&Constraint::HasAgent(AgentKind::ClaudeCode)));
        assert!(inner.contains(&Constraint::AgentAuthenticated(AgentKind::ClaudeCode)));
    }

    #[test]
    fn labels_and_tags_survive_their_colons() {
        assert_eq!(
            parse("tag=gpu:cuda").expect("parse"),
            Constraint::All(vec![Constraint::HasTag("gpu:cuda".into())])
        );
        assert_eq!(
            parse("label:location=home").expect("parse"),
            Constraint::All(vec![Constraint::Label("location".into(), "home".into())])
        );
    }

    #[test]
    fn bad_input_says_what_was_wrong() {
        // `Bare`, not `NeedsValue`: a word with no operator gets a sentence that does not claim
        // it is a clause — see `a_bare_word_is_not_called_a_clause_that_needs_a_value`.
        assert!(matches!(parse("cores"), Err(ParseError::Bare(_))));
        assert!(matches!(parse("nonsense=1"), Err(ParseError::Unknown(_))));
        assert!(matches!(
            parse("cores>=many"),
            Err(ParseError::NotANumber(_))
        ));
        assert!(matches!(
            parse("class=toaster"),
            Err(ParseError::BadValue { .. })
        ));
    }
}
