//! What a run has *cost so far*, as distinct from what it is.
//!
//! [`Run`](crate::run::Run) is the domain object: spec, state, epoch, lease. None of that
//! should grow a `cost_micro_usd` field, which is why turns and money have always lived
//! beside it rather than in it. But "beside it" meant "on one machine": the node that
//! submitted a run watched it reach `completed` and reported *0 turns, $0*, because the only
//! node that ever knew the numbers was the one that ran the agent — and after a migration,
//! neither node knew the whole of them.
//!
//! So progress travels, and like every other gossiped fact it needs an owner and a rule
//! (ADR-0005). It turned out to need **two**, because there are two kinds of number here and
//! they were sharing one rule:
//!
//! * **Position** — the turn count and the worktree summary — says where the run *is*. It
//!   belongs to a leg: one node, holding the run under one epoch. So it is settled the way the
//!   record beside it is settled, by [`RunProgress::absorb`] — highest epoch, and at equal epoch
//!   the lowest author id, which is `ClusterView::merge_run`'s tiebreak and has to stay identical
//!   to it.
//! * **Position** also carries the conversation's token totals (ADR-0040), because they are
//!   derived from the transcript the surviving leg holds rather than from what any leg spent.
//! * **Spend** — cost, denials, and questions put to a person — says what the run has used up.
//!   Forward only, and deliberately regardless of which leg spent it: the money left the account
//!   and the person was interrupted whether or not that leg went on to win.
//!
//! The rule used to be forward-only for all five, justified by the numbers having "a single
//! author by construction". That construction is exactly what a second grant breaks, and it
//! breaks without a partition: ADR-0006's silence-is-a-decline means a node can take a grant, run
//! under it, and have its answer lost, so two legs of one run publish two positions. Forward-only
//! then keeps the **larger** one — which is the loser's whenever the loser got further, and the
//! survivor can never correct it because a smaller number is refused. A run at turn 3 reporting
//! turn 19, with a worktree summary describing a checkout on a machine that is not running it,
//! and no tick that ever fixes it.
//!
//! The alternative for spend — last writer wins — is not a smaller version of this. It is a run
//! whose cost flickers between two numbers depending on which node gossiped most recently.

use crate::run::Epoch;
use crate::time::Millis;
use crate::NodeId;
use serde::{Deserialize, Serialize};

/// The bookkeeping for one run: what it has done, and what it has cost.
///
/// Cumulative for the **run**, not for the current agent process and not for the current
/// node. A resumed agent numbers its turns from one because it is a new process; the run does
/// not go back to turn one, and neither does its bill.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunProgress {
    pub turns: u32,
    /// Denied permission requests. Surfaced because a silently hobbled run that looks like a
    /// bad model is the failure mode ADR-0008 has to keep visible.
    pub denials: u32,
    /// Questions this run has put to a person (ADR-0017), over its whole life rather than over
    /// this leg of it — which is the point of it being here rather than in the holder's own
    /// registry. A run that migrates continues spending the budget its operator agreed to,
    /// instead of being handed a fresh one by every node it lands on.
    #[serde(default)]
    pub asks: u32,
    pub cost_micro_usd: u64,
    /// The holder's summary of the run's worktree — "2 modified, 1 new". A fact about the
    /// machine the run is on, which is exactly why it is worth carrying to the machine it
    /// is not on.
    pub workspace: String,
    /// When the node that produced these numbers last wrote them.
    ///
    /// The tiebreak *within one leg*, and the author's clock rather than the receiver's: a node
    /// relaying somebody else's record passes this through untouched, so a stale copy doing the
    /// rounds can never look fresher than the newer one it would overwrite. Comparing it across
    /// nodes is therefore always comparing one node's clock with itself — which is also why it
    /// cannot settle two *different* legs, and why the two fields below exist.
    #[serde(default)]
    pub at: Millis,
    /// The leg that produced these numbers: the node, and the epoch it held the run under.
    ///
    /// Written by the writer (`Supervisor::update_stats`) and passed through untouched by every
    /// relay, exactly like `at`. What it buys is that a position can be *ranked* against another
    /// leg's instead of merely being larger than it — see [`RunProgress::absorb`].
    ///
    /// The leg's **own** epoch, not the record's current one: a leg that has been superseded and
    /// emits one more turn boundary has to stamp what it actually held, or its numbers claim to
    /// be the current leg's and the ranking is back to a coin flip. Which is also why this is
    /// cheaper than the guard it replaces — the epoch is in memory beside the agent handle, where
    /// asking "do I still hold this" is a store read on the one hot path in the daemon.
    ///
    /// `None`, with epoch 0, for a record written before this existed — a stored row from an
    /// older build, since a fleet is one wire version throughout. Two unstamped records fall back
    /// to the forward-only comparison that used to be the whole rule, and an unstamped one loses
    /// to any stamped one, which is the safe direction: the stamped record is the only one whose
    /// leg is known.
    #[serde(default)]
    pub by: Option<NodeId>,
    #[serde(default)]
    pub epoch: Epoch,
    /// What the conversation has consumed, in tokens (ADR-0040).
    ///
    /// **Position, not spend**, which is the opposite of where a reader would first put it.
    /// Spend is folded in from whichever leg reports it because the money left the account
    /// either way; this is derived from the *transcript*, which is the conversation the
    /// surviving leg holds — a losing leg's extra turns are in a transcript nobody resumed and
    /// are not part of what this run went on to be. So it travels with `turns` and the worktree
    /// summary, under the same epoch-then-lowest-id arithmetic, and it **replaces** rather than
    /// accumulating.
    ///
    /// That is the distinction to keep hold of, because the two numbers beside each other merge
    /// in opposite directions: `cost_micro_usd` comes from a `result` event and is *per leg*, so
    /// it adds; this comes from a transcript and is *already cumulative*, so adding it would
    /// count every earlier leg again on every checkpoint. Measured, and written down in
    /// ADR-0040 §3.
    ///
    /// Zero for a run whose holder has taken no checkpoint yet — the transcript is read when one
    /// is captured — and for a record from a build that has never heard of the field.
    #[serde(default)]
    pub tokens: TokenUse,
    /// What each leg spent, keyed by the leg (ADR-0067).
    ///
    /// **Spend, per leg**, beside the position's `tokens` rather than instead of them. `tokens` is
    /// the conversation the surviving leg holds, which is right along a chain of resumed legs and
    /// wrong for two legs that both started from nothing: a partition ran WARMUP on the Mac and
    /// then on the laptop, and the fleet said 110 tokens when 220 were spent. Each leg's entry is
    /// written by that leg alone and only grows; [`RunProgress::spent`] sums them.
    #[serde(default)]
    pub legs: Vec<LegSpend>,
}

/// One leg's own spend (ADR-0067 §1): the transcript it started from, how far it got, and the
/// dollars its own `result` events added.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LegSpend {
    pub by: NodeId,
    pub epoch: Epoch,
    /// Tokens already in the transcript when the leg began: zero for a fresh start, the restored
    /// checkpoint's count for a resume. What the leg did not spend.
    #[serde(default)]
    pub base: TokenUse,
    /// The leg's transcript as last captured — cumulative, like the position's `tokens`.
    #[serde(default)]
    pub tokens: TokenUse,
    #[serde(default)]
    pub cost_micro_usd: u64,
}

impl LegSpend {
    /// What this leg itself consumed.
    #[must_use]
    pub fn own(&self) -> TokenUse {
        TokenUse {
            input: self.tokens.input.saturating_sub(self.base.input),
            output: self.tokens.output.saturating_sub(self.base.output),
            cache_creation: self
                .tokens
                .cache_creation
                .saturating_sub(self.base.cache_creation),
            cache_read: self.tokens.cache_read.saturating_sub(self.base.cache_read),
            messages: self.tokens.messages.saturating_sub(self.base.messages),
        }
    }

    /// The order two copies of one leg's entry are settled in: total, so every node agrees
    /// (`pitfalls/gossip-and-merge.md`), and "larger" is "later" because only the author writes it.
    fn rank(&self) -> (u64, u64, u64) {
        (self.tokens.total(), self.cost_micro_usd, self.base.total())
    }
}

/// Tokens a conversation has consumed, by the four counters the agent bills separately.
///
/// Separate rather than one total, because they are not interchangeable: a cache read is a
/// tenth of an input token and a cache write is more than one, so a single number could not be
/// converted to anything later even by somebody holding a price list.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(default)]
pub struct TokenUse {
    pub input: u64,
    pub output: u64,
    pub cache_creation: u64,
    pub cache_read: u64,
    /// Assistant messages counted, after deduplication. Carried because it is the cheap check
    /// that this parsed anything at all: a transcript whose shape has changed reports four
    /// zeros, which is indistinguishable from a conversation that has not started.
    pub messages: u64,
}

impl TokenUse {
    /// Every counter added. For a display that has room for one number, never for pricing.
    #[must_use]
    pub fn total(&self) -> u64 {
        self.input
            .saturating_add(self.output)
            .saturating_add(self.cache_creation)
            .saturating_add(self.cache_read)
    }

    /// Did this parse find anything?
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.messages == 0
    }
}

impl RunProgress {
    /// Fold a gossiped copy of a run's numbers into ours, and say whether anything moved.
    ///
    /// **Position first.** The turn count and the worktree summary name where the surviving
    /// branch of the run is, and they belong to a leg, so they are settled by the same arithmetic
    /// the record beside them is settled by (`ClusterView::merge_run`): a higher epoch is a
    /// decision already made, and two legs at one epoch are ordered by the **lowest author id**.
    /// That tiebreak has to stay identical to the record's or the run's numbers will describe one
    /// leg while its record names another — which is the whole failure, arrived at from the other
    /// side.
    ///
    /// Within one leg it is forward only, which is what the old rule was for all of this: a stale
    /// copy of our own numbers doing the rounds must not undo a newer one, and `at` settles the
    /// case where the counters agree and only the commentary has moved on — a worktree summary
    /// does that all day.
    ///
    /// **Spend separately, and monotonically.** Cost, denials and questions are folded in from
    /// *whichever* leg reports them, winner or loser, because the money left the account and the
    /// person was interrupted either way. Field-wise, so a run does not forget one leg's denials
    /// because another leg cost more. Reported as the highest any single leg reached rather than
    /// the sum: a relay repeats what it heard, so adding would count the same dollar every gossip
    /// tick. That understates a forked run's bill and is the honest answer an idempotent merge can
    /// give.
    ///
    /// So a merged record can be a *combination* of two legs — this leg's position, the fleet's
    /// high-water spend. That is ADR-0005's rule applied per field rather than per record: the
    /// position has an owner and the counters are a max-register with none.
    pub fn absorb(&mut self, incoming: &RunProgress) -> bool {
        let before = self.clone();
        if self.superseded_by(incoming) {
            self.turns = incoming.turns;
            self.workspace = incoming.workspace.clone();
            self.at = incoming.at;
            self.by = incoming.by;
            self.epoch = incoming.epoch;
            // With the position and not beside the counters below, deliberately — see the
            // field's own note. It is already cumulative, so `max` would be merely wrong in a
            // way that looks safe, and addition would count every earlier leg again.
            self.tokens = incoming.tokens;
        }
        self.cost_micro_usd = self.cost_micro_usd.max(incoming.cost_micro_usd);
        self.denials = self.denials.max(incoming.denials);
        self.asks = self.asks.max(incoming.asks);
        // Per-leg spend: a union, because a leg that lost still spent (ADR-0067 §3). Never part of
        // the position arithmetic above, and never a reason to change it.
        for leg in &incoming.legs {
            self.absorb_leg(*leg);
        }
        *self != before
    }

    /// Fold one leg's entry in: added if new, replaced if the incoming copy ranks higher.
    pub fn absorb_leg(&mut self, leg: LegSpend) {
        match self
            .legs
            .iter_mut()
            .find(|l| l.by == leg.by && l.epoch == leg.epoch)
        {
            Some(existing) if leg.rank() > existing.rank() => *existing = leg,
            Some(_) => {}
            None => {
                self.legs.push(leg);
                self.legs.sort_by_key(|l| (l.epoch, l.by));
            }
        }
    }

    /// What the run spent across every leg, surviving or not (ADR-0067 §2): tokens and dollars.
    /// Along a chain of resumed legs the tokens equal the position's; for a fork they are more.
    #[must_use]
    pub fn spent(&self) -> (TokenUse, u64) {
        self.legs
            .iter()
            .fold((TokenUse::default(), 0u64), |(t, c), leg| {
                let own = leg.own();
                (
                    TokenUse {
                        input: t.input.saturating_add(own.input),
                        output: t.output.saturating_add(own.output),
                        cache_creation: t.cache_creation.saturating_add(own.cache_creation),
                        cache_read: t.cache_read.saturating_add(own.cache_read),
                        messages: t.messages.saturating_add(own.messages),
                    },
                    c.saturating_add(leg.cost_micro_usd),
                )
            })
    }

    /// Tokens spent by legs that are not in the surviving conversation: `spent − tokens`, or zero.
    #[must_use]
    pub fn lost_tokens(&self) -> u64 {
        self.spent().0.total().saturating_sub(self.tokens.total())
    }

    /// Does `incoming` describe a leg entitled to say where the run is, over ours?
    fn superseded_by(&self, incoming: &RunProgress) -> bool {
        match incoming.epoch.cmp(&self.epoch) {
            std::cmp::Ordering::Greater => true,
            std::cmp::Ordering::Less => false,
            // One epoch. Two authors is a run granted twice, and every node has to pick the same
            // survivor from the record alone, offline, with no message — so: the lowest id, the
            // same one `merge_run` uses. One author, or neither stamped, is the old rule.
            std::cmp::Ordering::Equal => match (self.by, incoming.by) {
                (Some(ours), Some(theirs)) if ours != theirs => theirs < ours,
                _ => incoming.position() > self.position(),
            },
        }
    }

    /// Where this record says the run has got to, in the order the parts are compared.
    ///
    /// Turns first: it is the coarsest measure of work and the one a human reads. `at` next, which
    /// is not work at all and settles the case where the turn agrees and only the commentary has
    /// moved on — what a worktree summary does all day. The summary itself last, and only because
    /// the comparison has to be **total**: two writes in one millisecond with different summaries
    /// are otherwise ordered by which gossip arrived first, which is a fleet that disagrees for
    /// ever about a string. Arbitrary, like the lowest-id tiebreak above, and agreed for the same
    /// reason.
    fn position(&self) -> (u32, Millis, &str) {
        (self.turns, self.at, &self.workspace)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn progress(turns: u32, cost: u64) -> RunProgress {
        RunProgress {
            turns,
            cost_micro_usd: cost,
            at: Millis(1_000),
            ..RunProgress::default()
        }
    }

    fn node(seed: u8) -> NodeId {
        NodeId::from_bytes([seed; 32])
    }

    /// The same numbers, stamped with the leg that produced them.
    fn leg(seed: u8, epoch: u64, p: RunProgress) -> RunProgress {
        RunProgress {
            by: Some(node(seed)),
            epoch: Epoch(epoch),
            workspace: format!("worked on by {}", node(seed).short()),
            ..p
        }
    }

    /// What this node would report after hearing `incoming`.
    fn folded(existing: &RunProgress, incoming: &RunProgress) -> RunProgress {
        let mut out = existing.clone();
        out.absorb(incoming);
        out
    }

    fn used(total_input: u64) -> TokenUse {
        TokenUse {
            input: total_input,
            messages: 1,
            ..TokenUse::default()
        }
    }

    /// ADR-0067: two legs from nothing are both counted; a chain of resumed legs is not
    /// double-counted; and the merge is a union that every node settles the same way.
    #[test]
    fn a_leg_that_lost_still_spent_and_a_chain_is_counted_once() {
        let mac = LegSpend {
            by: node(1),
            epoch: Epoch(1),
            base: TokenUse::default(),
            tokens: used(110),
            cost_micro_usd: 0,
        };
        let laptop = LegSpend {
            by: node(2),
            epoch: Epoch(2),
            ..mac
        };
        // WARMUP: the laptop's leg won the position; the Mac's arrives later by gossip.
        let mut fleet = RunProgress {
            tokens: used(110),
            by: Some(node(2)),
            epoch: Epoch(2),
            legs: vec![laptop],
            ..RunProgress::default()
        };
        let from_mac = RunProgress {
            tokens: used(110),
            by: Some(node(1)),
            epoch: Epoch(1),
            legs: vec![mac],
            ..RunProgress::default()
        };
        assert!(fleet.absorb(&from_mac), "a new leg is news");
        assert_eq!(
            fleet.tokens.total(),
            110,
            "the position is the survivor's, as before"
        );
        assert_eq!(fleet.spent().0.total(), 220, "both legs spent");
        assert_eq!(fleet.lost_tokens(), 110);
        assert!(
            !fleet.clone().absorb(&from_mac),
            "the same leg twice is not news"
        );

        // A chain: the second leg resumed from the first's checkpoint, so its base is 100.
        let chain = RunProgress {
            tokens: used(150),
            legs: vec![
                LegSpend {
                    tokens: used(100),
                    ..mac
                },
                LegSpend {
                    base: used(100),
                    tokens: used(150),
                    ..laptop
                },
            ],
            ..RunProgress::default()
        };
        assert_eq!(chain.spent().0.total(), 150, "a chain is counted once");
        assert_eq!(chain.lost_tokens(), 0);

        // Within one leg, the larger copy wins whichever arrives first.
        let early = LegSpend {
            tokens: used(40),
            ..mac
        };
        let mut a = RunProgress {
            legs: vec![early],
            ..RunProgress::default()
        };
        let mut b = RunProgress {
            legs: vec![mac],
            ..RunProgress::default()
        };
        a.absorb_leg(mac);
        b.absorb_leg(early);
        assert_eq!(a.legs, b.legs);
        assert_eq!(a.legs[0].tokens.total(), 110);
    }

    #[test]
    fn the_node_that_submitted_a_run_cannot_report_it_back_to_zero() {
        // The bug this exists for. Every node gossips what it knows about runs, including
        // ones placed elsewhere, and the submitting node's copy has never had a number in it.
        // Last-writer-wins makes `ps` flicker between the truth and zero.
        let real = progress(8, 83_000);
        let submitters = RunProgress {
            at: Millis(9_999),
            ..progress(0, 0)
        };

        assert_eq!(
            folded(&real, &submitters).turns,
            8,
            "and being the more recent write does not help"
        );
        assert_eq!(folded(&RunProgress::default(), &submitters).turns, 0);
    }

    #[test]
    fn a_previous_holder_does_not_undo_the_new_ones_count() {
        // After a migration both nodes are honest and one of them is behind: A ran turns 1-7
        // and B is on turn 12. A keeps gossiping what it saw until the run ages out.
        let b = progress(12, 120_000);
        let a = progress(7, 70_000);
        assert_eq!(folded(&b, &a).turns, 12);
        assert_eq!(folded(&a, &b).turns, 12);
    }

    #[test]
    fn cost_moves_within_a_turn() {
        // A turn is minutes of model time. Waiting for the boundary to update the bill would
        // show $0 for the whole of the first one.
        let before = progress(3, 10_000);
        let after = progress(3, 40_000);
        assert_eq!(folded(&before, &after).cost_micro_usd, 40_000);
        assert_eq!(folded(&after, &before).cost_micro_usd, 40_000);
    }

    #[test]
    fn a_finished_runs_last_word_is_its_worktree_and_it_still_travels() {
        // Found on real daemons: the final `git status` summary lands *after* the last turn,
        // so it moves no counter — and a rule that asked the run's holder to settle that tie
        // had nobody to ask, because the run had finished. A peer showed "preparing" for a
        // completed run for ever.
        let during = RunProgress {
            workspace: "preparing".into(),
            at: Millis(1_000),
            ..progress(4, 99_000)
        };
        let after = RunProgress {
            workspace: "3 new".into(),
            at: Millis(2_000),
            ..progress(4, 99_000)
        };

        assert_eq!(folded(&during, &after).workspace, "3 new");
        // And the stale copy still doing the rounds does not undo it, because it carries the
        // author's timestamp rather than the relayer's.
        assert_eq!(folded(&after, &during).workspace, "3 new");
    }

    #[test]
    fn a_record_from_before_the_timestamp_existed_loses_to_one_with_it() {
        // `at` defaults to zero for anything written by an older node, which is the right way
        // round: same numbers, and the one that can say when wins.
        let old = RunProgress {
            at: Millis(0),
            ..progress(6, 60_000)
        };
        let new = progress(6, 60_000);
        assert_eq!(folded(&old, &new).at, Millis(1_000));
        assert_eq!(folded(&new, &old).at, Millis(1_000));
    }

    #[test]
    fn a_leg_that_lost_does_not_freeze_the_run_where_it_got_to() {
        // The bug the whole two-rule split exists for, and it needs no partition: a node takes
        // a grant, its answer is lost (ADR-0006 makes that the ordinary case), the arbiter grants
        // the next bidder at the next epoch, and now two legs of one run are publishing. The
        // lost leg gets further — it started first — so forward-only kept *its* turn count and
        // its worktree summary, and the surviving leg could never correct either, because a
        // smaller number is refused for ever.
        let lost = leg(3, 4, progress(19, 400_000));
        let survived = leg(1, 5, progress(3, 30_000));

        let fleet = folded(&lost, &survived);
        assert_eq!(fleet.turns, 3, "the run is at turn 3, whoever got further");
        assert_eq!(fleet.by, Some(node(1)));
        assert_eq!(
            fleet.workspace,
            format!("worked on by {}", node(1).short()),
            "and the worktree described is the one being worked in"
        );
        // Symmetric, which is the point of settling it from the record rather than from arrival
        // order: every node reaches the same answer with no message.
        assert_eq!(folded(&survived, &lost).turns, 3);
    }

    #[test]
    fn but_what_the_losing_leg_spent_is_still_spent() {
        // The other half of the split. Turns are a position and are the survivor's; money and
        // interruptions are gone whoever spent them, so a run that forked and lost a leg still
        // reports the bill and still counts the questions against its operator's budget.
        let lost = RunProgress {
            denials: 2,
            asks: 6,
            ..leg(3, 4, progress(19, 400_000))
        };
        let survived = leg(1, 5, progress(3, 30_000));

        let fleet = folded(&survived, &lost);
        assert_eq!(fleet.cost_micro_usd, 400_000);
        assert_eq!(fleet.asks, 6, "and the ask budget is not handed back");
        assert_eq!(fleet.denials, 2);
        assert_eq!(fleet.turns, 3, "without the position moving with it");
    }

    #[test]
    fn two_legs_at_one_epoch_are_settled_the_way_the_record_is() {
        // Two arbiters both compute `next()` from the same record and issue the same number, so
        // the epoch cannot order these (ADR-0002's amendment). `merge_run` breaks that tie on the
        // **lowest holder id** so that every node picks the same survivor offline; this has to be
        // the identical rule, or a run's numbers describe one leg while its record names another.
        let low = leg(1, 5, progress(2, 20_000));
        let high = leg(9, 5, progress(30, 300_000));

        assert_eq!(folded(&high, &low).by, Some(node(1)));
        assert_eq!(folded(&low, &high).by, Some(node(1)));
        assert_eq!(folded(&high, &low).turns, 2);
        assert_eq!(folded(&low, &high).turns, 2);
    }

    #[test]
    fn a_record_from_before_legs_were_stamped_loses_to_one_that_has_them() {
        // A stored row from an older build decodes with no author and epoch 0. It loses, which
        // is the safe direction: the stamped record is the only one whose leg is known. And two
        // unstamped records still settle by the forward-only rule that used to be all of this.
        let unstamped = progress(11, 110_000);
        let stamped = leg(2, 3, progress(4, 40_000));

        assert_eq!(folded(&unstamped, &stamped).turns, 4);
        assert_eq!(folded(&stamped, &unstamped).turns, 4);
        assert_eq!(
            folded(&unstamped, &stamped).cost_micro_usd,
            110_000,
            "and the older record's spend is still spend"
        );
    }

    #[test]
    fn absorbing_says_whether_anything_moved() {
        // The return value is what the view and the store both key their writes on, and a merge
        // that reported a change it had not made would gossip a record nobody had changed, for
        // ever.
        let mine = leg(1, 5, progress(3, 30_000));
        assert!(!folded(&mine, &mine.clone()).absorb(&mine));
        let mut same = mine.clone();
        assert!(!same.absorb(&mine));
        let mut behind = leg(1, 5, progress(2, 20_000));
        assert!(behind.absorb(&mine));
    }
}
