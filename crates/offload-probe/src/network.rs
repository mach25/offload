//! Whether this device's bytes cost money (ADR-0045).
//!
//! The field this fills used to be a `bool` that the probe set to `false` unconditionally, under
//! a comment calling it an assumption — so `Refusal::MeteredNetwork` was a sentence no fleet could
//! produce, and `--require unmetered` matched every node in the world. It is three-valued now, and
//! this module's job is to return `Unknown` honestly rather than to guess.
//!
//! **NetworkManager only.** It is the one thing on a Linux box that actually knows, and it knows
//! more than a bool can hold: `NMMetered` is `UNKNOWN | YES | NO | GUESS_YES | GUESS_NO`, and
//! `nmcli` prints the guesses with a `(guessed)` suffix. That suffix is the whole reason this is
//! not a two-line cast — see [`parse`].
//!
//! No `/sys` heuristic about interface names, and no "does this look like a wwan device". A probe
//! that guesses wins bids and then fails the runs it took; `Unknown` is a thing this type can say
//! now, so there is no reason to invent an answer.

use offload_core::Metered;
use std::process::Command;

/// Ask NetworkManager. `Unknown` when it is not installed, not running, or does not know.
#[must_use]
pub fn detect() -> Metered {
    let Ok(out) = Command::new("nmcli")
        .args(["-t", "-f", "METERED", "general"])
        .output()
    else {
        // No NetworkManager — a server, a container, Termux on a phone. Not an error and not
        // a reason to guess.
        return Metered::Unknown;
    };
    if !out.status.success() {
        return Metered::Unknown;
    }
    parse(&String::from_utf8_lossy(&out.stdout))
}

/// What `nmcli -t -f METERED general` said, as one of the three answers we can hold.
///
/// The mapping has a reason per row rather than being a cast, and the interesting row is the
/// fourth:
///
/// | NetworkManager | ours | why |
/// | --- | --- | --- |
/// | `yes` | `Yes` | stated on the connection profile |
/// | `yes (guessed)` | `Yes` | NM guesses yes only for WWAN and modems, which do cost |
/// | `no` | `No` | stated on the connection profile |
/// | `no (guessed)` | **`Unknown`** | NM guesses no for *every* ethernet and wifi link |
/// | anything else | `Unknown` | nobody could tell |
///
/// **`no (guessed)` is not `No`.** NM cannot see through a tether: a laptop sharing a phone's
/// connection over wifi or USB is ordinary wifi or ordinary ethernet from below, and NM guesses it
/// free. Tethering is the first case the capability's own doc comment names, so promoting that
/// guess to a fact would reimplement the bug this replaced — the probe was already making exactly
/// this guess, and the point is to stop.
#[must_use]
pub fn parse(out: &str) -> Metered {
    let line = out.trim().to_ascii_lowercase();
    match line.as_str() {
        "yes" | "yes (guessed)" => Metered::Yes,
        "no" => Metered::No,
        _ => Metered::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_guess_that_the_link_is_free_is_not_an_answer() {
        // The row this module exists for. It is what NetworkManager says on the machine this was
        // written on, it is what it says about a tethered laptop, and those are different
        // situations — so it is not a fact.
        assert_eq!(parse("no (guessed)"), Metered::Unknown);
        // …while a guess that it *costs* is one, because NM only guesses that for WWAN.
        assert_eq!(parse("yes (guessed)"), Metered::Yes);
    }

    #[test]
    fn a_stated_answer_is_taken_at_its_word() {
        assert_eq!(parse("no"), Metered::No);
        assert_eq!(parse("yes"), Metered::Yes);
    }

    #[test]
    fn nothing_useful_is_unknown_rather_than_convenient() {
        // Every one of these used to be `false` — "this link is free" — which is the claim the
        // whole ADR is about not making.
        for said in ["unknown", "", "\n", "  ", "Segmentation fault"] {
            assert_eq!(parse(said), Metered::Unknown, "{said:?}");
        }
    }

    #[test]
    fn the_answer_survives_nmcli_shouting() {
        // `-t` is terse mode and should give a bare word, but the field is printed with padding
        // in table mode and some versions echo the header. Trim and fold case rather than
        // depending on which.
        assert_eq!(parse("YES\n"), Metered::Yes);
        assert_eq!(parse("  no  \n"), Metered::No);
    }
}
