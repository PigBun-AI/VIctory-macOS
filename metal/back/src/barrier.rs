//! Deferred barriers: the part of `Record` that decides where a barrier
//! lands. Pure bookkeeping, no Metal objects, so it is tested exhaustively
//! without a GPU.
//!
//! A barrier arrives as "stages `after` must finish before stages `before`
//! may start" (our own stage bits, [`crate::stages`]). Metal 4 has three
//! encodings and which one is right depends on the *next command*, which is
//! why the barrier is held back until that command is known:
//!
//! * **in the encoder** (`barrierAfterEncoderStages:beforeEncoderStages:`)
//!   when producer and consumer sit in the same encoder;
//! * **consumer** form (`barrierAfterQueueStages:beforeStages:`), encoded at
//!   the start of the encoder that contains the consumer, when the producer
//!   work is in earlier encoders;
//! * **producer** form (`barrierAfterStages:beforeQueueStages:`), encoded at
//!   the end of the current encoder, used only when nothing follows in the
//!   command list (the barrier's consumer is in a later list).
//!
//! Metal 4 runs encoders (and dispatches inside a compute encoder)
//! concurrently unless told otherwise, so a barrier that is not encoded is
//! a barrier that is not there. A barrier stays pending until a command
//! whose stage is in its `before` mask arrives; commands that cannot be
//! affected do not pay for it.
//!
//! **Ordering after earlier lists.** The game's API runs the lists of a queue
//! in order; Metal 4 would overlap them. A tracker made with
//! `prior_lists_covered` puts one consumer-form barrier, every earlier queue
//! stage to this encoder's stages, at the *start of every encoder it opens*
//! (the "head barrier"). Nothing is encoded for a list that opens no encoder
//! (an empty list costs no encoder, which is the point), and nothing at the
//! end of a list. Every encoder gets one, not just the first, because Metal 4
//! may overlap two encoders of one list, so the second would not be ordered
//! after earlier lists by the first one's barrier. The head barrier covers
//! every device-visibility consumer barrier and every device-visibility
//! producer barrier of a list, which are therefore not encoded.
//!
//! The tracker's output is a list of [`Action`]s the executor performs in
//! order, each *before* the command that triggered it.

use crate::stages::{
    ALL, COMPUTE_ENCODER_STAGES, FRAGMENT, RENDER_ENCODER_STAGES, TILE, VIS_DEVICE,
};

/// Which Metal 4 encoder a command needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EncoderKind {
    /// `MTL4ComputeCommandEncoder`: dispatches, copies, fills.
    Compute,
    /// `MTL4RenderCommandEncoder` (M3; the tracker already knows its rules).
    Render,
}

impl EncoderKind {
    /// The stages commands of this encoder have.
    pub const fn stages(self) -> u16 {
        match self {
            EncoderKind::Compute => COMPUTE_ENCODER_STAGES,
            EncoderKind::Render => RENDER_ENCODER_STAGES,
        }
    }

    /// The encoder that can encode a command of `stage` (one bit).
    pub fn of_stage(stage: u16) -> Option<EncoderKind> {
        if stage.count_ones() != 1 {
            None
        } else if stage & COMPUTE_ENCODER_STAGES != 0 {
            Some(EncoderKind::Compute)
        } else if stage & RENDER_ENCODER_STAGES != 0 {
            Some(EncoderKind::Render)
        } else {
            None
        }
    }
}

/// One step for the executor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// Begin an encoder of this kind.
    Open(EncoderKind),
    /// End the open encoder.
    Close,
    /// `barrierAfterEncoderStages:beforeEncoderStages:visibilityOptions:`
    Encoder { after: u16, before: u16, vis: u8 },
    /// `barrierAfterQueueStages:beforeStages:visibilityOptions:`
    Consumer { after: u16, before: u16, vis: u8 },
    /// `barrierAfterStages:beforeQueueStages:visibilityOptions:`
    Producer { after: u16, before: u16, vis: u8 },
}

#[derive(Debug)]
struct OpenEncoder {
    id: u32,
    kind: EncoderKind,
    /// Stages of the commands encoded so far.
    used: u16,
}

#[derive(Debug, Clone, Copy)]
struct Pending {
    after: u16,
    before: u16,
    vis: u8,
    /// The encoder that was open when the barrier arrived, if any.
    declared_in: Option<u32>,
    /// Stages of that encoder's commands before the barrier.
    in_encoder: u16,
    /// Stages of the commands in encoders that were already closed.
    earlier: u16,
}

/// Where barriers go in one command list.
#[derive(Debug)]
pub struct Tracker {
    open: Option<OpenEncoder>,
    next_id: u32,
    closed: u16,
    pending: Vec<Pending>,
    /// Earlier lists are ordered before this one by a queue-level barrier.
    covered: bool,
    /// Every encoder starts with that barrier.
    head_barrier: bool,
}

impl Tracker {
    /// A tracker for a fresh command list.
    ///
    /// `prior_lists_covered` asks for the head barrier (see the module
    /// docs): every encoder starts with a queue-level barrier from all
    /// earlier work, so this list is ordered after everything committed
    /// before it, and a barrier that only concerns work from earlier lists
    /// needs nothing. With `false`, nothing orders the list after earlier
    /// ones and such a barrier is encoded as a consumer barrier.
    pub fn new(prior_lists_covered: bool) -> Tracker {
        Tracker {
            open: None,
            next_id: 0,
            closed: 0,
            pending: Vec::new(),
            covered: prior_lists_covered,
            head_barrier: prior_lists_covered,
        }
    }

    /// Tests of where barriers land: a tracker that knows earlier lists are
    /// covered but does not encode the head barrier, so the placement of
    /// the barriers themselves is all that shows.
    #[cfg(test)]
    fn covered_without_head() -> Tracker {
        Tracker {
            covered: true,
            ..Tracker::new(false)
        }
    }

    /// The kind of the open encoder.
    pub fn open_kind(&self) -> Option<EncoderKind> {
        self.open.as_ref().map(|o| o.kind)
    }

    /// Barriers waiting for a consumer.
    pub fn pending(&self) -> usize {
        self.pending.len()
    }

    /// A barrier arrives. Nothing is encoded yet.
    pub fn barrier(&mut self, after: u16, before: u16, vis: u8) {
        if after == 0 || before == 0 {
            let reason = match (after == 0, before == 0) {
                (true, true) => "both",
                (true, false) => "after",
                _ => "before",
            };
            crate::dropstats::silent("barrier.empty_mask", reason);
            return;
        }
        let (declared_in, in_encoder) = match &self.open {
            Some(o) => (Some(o.id), o.used),
            None => (None, 0),
        };
        let candidate = Pending {
            after,
            before,
            vis,
            declared_in,
            in_encoder,
            earlier: self.closed,
        };
        // Barriers that arrive with no command in between and the same
        // visibility are one barrier with both masks.
        if let Some(p) = self.pending.iter_mut().find(|p| {
            p.vis == vis
                && p.declared_in == declared_in
                && p.in_encoder == in_encoder
                && p.earlier == candidate.earlier
        }) {
            p.after |= after;
            p.before |= before;
        } else {
            self.pending.push(candidate);
        }
    }

    /// A command of `stage` (one bit) is about to be encoded. Returns what
    /// to do first. `None` for a stage no encoder can encode.
    pub fn command(&mut self, stage: u16) -> Option<Vec<Action>> {
        let kind = EncoderKind::of_stage(stage)?;
        let mut out = Vec::new();
        if self.open_kind() != Some(kind) {
            self.close(&mut out);
            self.open(kind, &mut out);
        }
        // A render pass cannot wait on its own fragment or tile work from
        // inside (tile-based GPU): such a barrier ends the pass; what it
        // waits for is then in an earlier encoder and goes the consumer way.
        if kind == EncoderKind::Render {
            let needs_split = self.open.as_ref().is_some_and(|o| {
                self.pending.iter().any(|p| {
                    p.before & stage != 0
                        && p.declared_in == Some(o.id)
                        && p.after & p.in_encoder & (FRAGMENT | TILE) != 0
                })
            });
            if needs_split {
                self.close(&mut out);
                self.open(kind, &mut out);
            }
        }
        let (id, encoder_stages) = {
            let o = self.open.as_ref().expect("encoder opened above");
            (o.id, o.kind.stages())
        };
        let mut keep = Vec::with_capacity(self.pending.len());
        for p in std::mem::take(&mut self.pending) {
            if p.before & stage == 0 {
                keep.push(p);
                continue;
            }
            let before = p.before & encoder_stages;
            let in_encoder_after = if p.declared_in == Some(id) {
                p.after & p.in_encoder
            } else {
                0
            };
            if in_encoder_after != 0 {
                out.push(Action::Encoder {
                    after: in_encoder_after,
                    before,
                    vis: p.vis,
                });
            }
            // The consumer form is for work in other encoders, and, when no
            // queue-level barrier orders earlier lists before this one, for
            // work of earlier lists too (any stage the barrier names).
            let consumer_after = if self.covered {
                p.after & p.earlier
            } else {
                p.after
            };
            // The head barrier of this encoder already waits for all earlier
            // queue work with device visibility.
            let covered_by_head = self.head_barrier && p.vis == VIS_DEVICE;
            if consumer_after != 0 && !covered_by_head {
                out.push(Action::Consumer {
                    after: consumer_after,
                    before,
                    vis: p.vis,
                });
            }
        }
        self.pending = keep;
        if let Some(o) = self.open.as_mut() {
            o.used |= stage;
        }
        Some(out)
    }

    /// Forces a new encoder for the next command (for example to make a
    /// cross-encoder dependency, or at a render-pass boundary).
    pub fn split(&mut self) -> Vec<Action> {
        let mut out = Vec::new();
        self.close(&mut out);
        out
    }

    /// The list ends. Pending barriers whose consumer is in a later list
    /// become producer barriers at the end of the last encoder, except
    /// (with the head barrier) device-visibility ones, which the next list's
    /// head barrier covers.
    pub fn finish(&mut self) -> Vec<Action> {
        let mut out = Vec::new();
        let mut producers = Vec::new();
        for p in std::mem::take(&mut self.pending) {
            let known = p.earlier | p.in_encoder;
            if self.head_barrier && p.vis == VIS_DEVICE {
                continue;
            }
            let after = if self.covered {
                p.after & known
            } else {
                p.after
            };
            if after != 0 {
                producers.push(Action::Producer {
                    after,
                    before: p.before,
                    vis: p.vis,
                });
            }
        }
        if !producers.is_empty() {
            if self.open.is_none() {
                self.open(EncoderKind::Compute, &mut out);
            }
            out.extend(producers);
        }
        self.close(&mut out);
        out
    }

    fn open(&mut self, kind: EncoderKind, out: &mut Vec<Action>) {
        debug_assert!(self.open.is_none());
        self.open = Some(OpenEncoder {
            id: self.next_id,
            kind,
            used: 0,
        });
        self.next_id += 1;
        out.push(Action::Open(kind));
        if self.head_barrier {
            out.push(Action::Consumer {
                after: ALL,
                before: kind.stages(),
                vis: VIS_DEVICE,
            });
        }
    }

    fn close(&mut self, out: &mut Vec<Action>) {
        let Some(o) = self.open.take() else {
            return;
        };
        for p in &mut self.pending {
            if p.declared_in == Some(o.id) {
                p.earlier |= p.in_encoder;
                p.in_encoder = 0;
                p.declared_in = None;
            }
        }
        self.closed |= o.used;
        out.push(Action::Close);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stages::{ALL, BLIT, DISPATCH, MESH, VERTEX, VIS_DEVICE, VIS_RESOURCE_ALIAS};

    fn cmd(t: &mut Tracker, stage: u16) -> Vec<Action> {
        t.command(stage).expect("encodable stage")
    }

    #[test]
    fn first_command_opens_a_compute_encoder() {
        let mut t = Tracker::covered_without_head();
        assert_eq!(cmd(&mut t, DISPATCH), [Action::Open(EncoderKind::Compute)]);
        assert_eq!(cmd(&mut t, BLIT), []);
        assert_eq!(cmd(&mut t, DISPATCH), []);
        assert_eq!(t.open_kind(), Some(EncoderKind::Compute));
    }

    #[test]
    fn barrier_between_two_dispatches_lands_in_the_encoder() {
        let mut t = Tracker::covered_without_head();
        cmd(&mut t, DISPATCH);
        t.barrier(DISPATCH, DISPATCH, VIS_DEVICE);
        assert_eq!(t.pending(), 1);
        assert_eq!(
            cmd(&mut t, DISPATCH),
            [Action::Encoder {
                after: DISPATCH,
                before: DISPATCH,
                vis: VIS_DEVICE
            }]
        );
        assert_eq!(t.pending(), 0);
        // Not repeated for the next command.
        assert_eq!(cmd(&mut t, DISPATCH), []);
    }

    #[test]
    fn copy_then_dispatch_uses_blit_to_dispatch() {
        let mut t = Tracker::covered_without_head();
        cmd(&mut t, BLIT);
        t.barrier(BLIT, DISPATCH, VIS_DEVICE);
        assert_eq!(
            cmd(&mut t, DISPATCH),
            [Action::Encoder {
                after: BLIT,
                before: DISPATCH,
                vis: VIS_DEVICE
            }]
        );
    }

    #[test]
    fn barrier_waits_for_a_command_it_can_affect() {
        let mut t = Tracker::covered_without_head();
        cmd(&mut t, BLIT);
        t.barrier(BLIT, DISPATCH, VIS_DEVICE);
        // Another copy is not a consumer of the barrier: nothing is encoded.
        assert_eq!(cmd(&mut t, BLIT), []);
        assert_eq!(t.pending(), 1);
        // The dispatch is; the barrier waits for both copies (it is encoded
        // later than declared, which is safe: more work is covered).
        assert_eq!(
            cmd(&mut t, DISPATCH),
            [Action::Encoder {
                after: BLIT,
                before: DISPATCH,
                vis: VIS_DEVICE
            }]
        );
    }

    #[test]
    fn barrier_across_an_encoder_split_is_a_consumer_barrier() {
        let mut t = Tracker::covered_without_head();
        cmd(&mut t, DISPATCH);
        t.barrier(DISPATCH, DISPATCH, VIS_DEVICE);
        assert_eq!(t.split(), [Action::Close]);
        assert_eq!(
            cmd(&mut t, DISPATCH),
            [
                Action::Open(EncoderKind::Compute),
                Action::Consumer {
                    after: DISPATCH,
                    before: DISPATCH,
                    vis: VIS_DEVICE
                }
            ]
        );
    }

    #[test]
    fn barrier_from_compute_to_render_is_a_consumer_barrier_in_the_render_encoder() {
        let mut t = Tracker::covered_without_head();
        cmd(&mut t, DISPATCH);
        t.barrier(DISPATCH, VERTEX | FRAGMENT, VIS_DEVICE);
        assert_eq!(
            cmd(&mut t, VERTEX),
            [
                Action::Close,
                Action::Open(EncoderKind::Render),
                Action::Consumer {
                    after: DISPATCH,
                    before: VERTEX | FRAGMENT,
                    vis: VIS_DEVICE
                }
            ]
        );
    }

    #[test]
    fn consumer_before_is_limited_to_the_consuming_encoder() {
        let mut t = Tracker::covered_without_head();
        cmd(&mut t, DISPATCH);
        t.barrier(DISPATCH, DISPATCH | VERTEX | MESH, VIS_DEVICE);
        let acts = cmd(&mut t, VERTEX);
        assert_eq!(
            acts.last(),
            Some(&Action::Consumer {
                after: DISPATCH,
                before: VERTEX | MESH,
                vis: VIS_DEVICE
            })
        );
    }

    #[test]
    fn pending_barrier_at_the_end_becomes_a_producer_barrier() {
        let mut t = Tracker::covered_without_head();
        cmd(&mut t, DISPATCH);
        t.barrier(DISPATCH, DISPATCH | BLIT, VIS_DEVICE);
        assert_eq!(
            t.finish(),
            [
                Action::Producer {
                    after: DISPATCH,
                    before: DISPATCH | BLIT,
                    vis: VIS_DEVICE
                },
                Action::Close
            ]
        );
    }

    #[test]
    fn producer_after_is_limited_to_work_the_list_did_when_prior_lists_are_covered() {
        let mut t = Tracker::new(true);
        cmd(&mut t, BLIT);
        t.barrier(ALL, ALL, VIS_RESOURCE_ALIAS);
        assert_eq!(
            t.finish(),
            [
                Action::Producer {
                    after: BLIT,
                    before: ALL,
                    vis: VIS_RESOURCE_ALIAS
                },
                Action::Close
            ]
        );
        // Without the head barrier nothing narrows it.
        let mut t = Tracker::new(false);
        cmd(&mut t, BLIT);
        t.barrier(ALL, ALL, VIS_DEVICE);
        assert_eq!(
            t.finish(),
            [
                Action::Producer {
                    after: ALL,
                    before: ALL,
                    vis: VIS_DEVICE
                },
                Action::Close
            ]
        );
    }

    #[test]
    fn barrier_about_earlier_lists_only() {
        // Covered by the head barrier of the next list: nothing at all, not
        // even an encoder just to hold it.
        let mut t = Tracker::new(true);
        t.barrier(DISPATCH, DISPATCH, VIS_DEVICE);
        assert_eq!(t.finish(), []);

        // Not covered: it is encoded, in an encoder made for it if need be.
        let mut t = Tracker::new(false);
        t.barrier(DISPATCH, DISPATCH, VIS_DEVICE);
        assert_eq!(
            t.finish(),
            [
                Action::Open(EncoderKind::Compute),
                Action::Producer {
                    after: DISPATCH,
                    before: DISPATCH,
                    vis: VIS_DEVICE
                },
                Action::Close
            ]
        );

        // A consumer inside the list: consumer barrier on the first encoder.
        let mut t = Tracker::new(false);
        t.barrier(DISPATCH, DISPATCH, VIS_DEVICE);
        assert_eq!(
            cmd(&mut t, DISPATCH),
            [
                Action::Open(EncoderKind::Compute),
                Action::Consumer {
                    after: DISPATCH,
                    before: DISPATCH,
                    vis: VIS_DEVICE
                }
            ]
        );
        // Covered: the head barrier alone.
        let mut t = Tracker::new(true);
        t.barrier(DISPATCH, DISPATCH, VIS_DEVICE);
        assert_eq!(
            cmd(&mut t, DISPATCH),
            [Action::Open(EncoderKind::Compute), HEAD]
        );
        // Aliasing visibility is not what the head barrier does: kept.
        let mut t = Tracker::new(true);
        t.barrier(DISPATCH, DISPATCH, VIS_RESOURCE_ALIAS);
        assert_eq!(
            cmd(&mut t, DISPATCH),
            [Action::Open(EncoderKind::Compute), HEAD]
        );
        let mut t = Tracker::new(true);
        t.barrier(DISPATCH, DISPATCH, VIS_RESOURCE_ALIAS);
        assert_eq!(
            t.finish(),
            [],
            "nothing in the list can be affected, the barrier is about earlier lists"
        );
    }

    /// The head barrier `Tracker::new(true)` puts at the start of every encoder.
    const HEAD: Action = Action::Consumer {
        after: ALL,
        before: COMPUTE_ENCODER_STAGES,
        vis: VIS_DEVICE,
    };

    #[test]
    fn every_encoder_starts_with_the_head_barrier_and_nothing_ends_with_one() {
        let mut t = Tracker::new(true);
        assert_eq!(
            cmd(&mut t, DISPATCH),
            [Action::Open(EncoderKind::Compute), HEAD]
        );
        assert_eq!(cmd(&mut t, BLIT), []);
        assert_eq!(t.finish(), [Action::Close], "no trailing barrier");
        // A second encoder of the same list gets its own: Metal 4 may overlap
        // the two, so the first one's barrier does not order the second.
        let mut t = Tracker::new(true);
        cmd(&mut t, DISPATCH);
        assert_eq!(t.split(), [Action::Close]);
        assert_eq!(
            cmd(&mut t, DISPATCH),
            [Action::Open(EncoderKind::Compute), HEAD]
        );
        // A render encoder's head barrier names its own stages.
        let mut t = Tracker::new(true);
        assert_eq!(
            cmd(&mut t, VERTEX),
            [
                Action::Open(EncoderKind::Render),
                Action::Consumer {
                    after: ALL,
                    before: RENDER_ENCODER_STAGES,
                    vis: VIS_DEVICE
                }
            ]
        );
    }

    #[test]
    fn an_empty_list_has_no_encoder_whatever_the_queue_option() {
        assert_eq!(Tracker::new(true).finish(), []);
        assert_eq!(Tracker::new(false).finish(), []);
    }

    #[test]
    fn device_barriers_at_the_end_of_a_list_are_covered_by_the_next_lists_head() {
        let mut t = Tracker::new(true);
        cmd(&mut t, DISPATCH);
        t.barrier(DISPATCH, DISPATCH | BLIT, VIS_DEVICE);
        assert_eq!(t.finish(), [Action::Close]);
        // Without the head barrier it must be encoded.
        let mut t = Tracker::new(false);
        cmd(&mut t, DISPATCH);
        t.barrier(DISPATCH, DISPATCH | BLIT, VIS_DEVICE);
        assert_eq!(
            t.finish(),
            [
                Action::Producer {
                    after: DISPATCH,
                    before: DISPATCH | BLIT,
                    vis: VIS_DEVICE
                },
                Action::Close
            ]
        );
    }

    #[test]
    fn consecutive_barriers_merge_and_visibilities_stay_apart() {
        let mut t = Tracker::covered_without_head();
        cmd(&mut t, DISPATCH);
        cmd(&mut t, BLIT);
        t.barrier(DISPATCH, DISPATCH, VIS_DEVICE);
        t.barrier(BLIT, DISPATCH, VIS_DEVICE);
        assert_eq!(t.pending(), 1);
        t.barrier(DISPATCH, DISPATCH, VIS_RESOURCE_ALIAS);
        assert_eq!(t.pending(), 2);
        let acts = cmd(&mut t, DISPATCH);
        assert_eq!(
            acts,
            [
                Action::Encoder {
                    after: DISPATCH | BLIT,
                    before: DISPATCH,
                    vis: VIS_DEVICE
                },
                Action::Encoder {
                    after: DISPATCH,
                    before: DISPATCH,
                    vis: VIS_RESOURCE_ALIAS
                }
            ]
        );
    }

    #[test]
    fn an_empty_barrier_is_dropped() {
        let mut t = Tracker::covered_without_head();
        cmd(&mut t, DISPATCH);
        t.barrier(0, DISPATCH, VIS_DEVICE);
        t.barrier(DISPATCH, 0, VIS_DEVICE);
        assert_eq!(t.pending(), 0);
    }

    #[test]
    fn a_render_pass_does_not_wait_on_its_own_fragment_work() {
        let mut t = Tracker::covered_without_head();
        cmd(&mut t, FRAGMENT);
        t.barrier(FRAGMENT, VERTEX, VIS_DEVICE);
        assert_eq!(
            cmd(&mut t, VERTEX),
            [
                Action::Close,
                Action::Open(EncoderKind::Render),
                Action::Consumer {
                    after: FRAGMENT,
                    before: VERTEX,
                    vis: VIS_DEVICE
                }
            ]
        );
        // Vertex-to-vertex inside one pass is allowed in the encoder.
        let mut t = Tracker::covered_without_head();
        cmd(&mut t, VERTEX);
        t.barrier(VERTEX, VERTEX, VIS_DEVICE);
        assert_eq!(
            cmd(&mut t, VERTEX),
            [Action::Encoder {
                after: VERTEX,
                before: VERTEX,
                vis: VIS_DEVICE
            }]
        );
    }

    #[test]
    fn unencodable_stages_are_refused() {
        let mut t = Tracker::covered_without_head();
        assert!(t.command(0).is_none());
        assert!(t.command(DISPATCH | BLIT).is_none());
        assert!(t.command(crate::stages::RESOURCE_STATE).is_none());
        assert!(t.command(0x4000).is_none());
    }

    #[test]
    fn earlier_encoder_work_is_found_through_closed_encoders() {
        // dispatch | barrier | split | blit | (new encoder) dispatch:
        // the barrier was declared in encoder 0; the consumer is in
        // encoder 1, so it is a consumer barrier for DISPATCH only.
        let mut t = Tracker::new(false);
        cmd(&mut t, DISPATCH);
        t.barrier(ALL, DISPATCH, VIS_DEVICE);
        t.split();
        let acts = cmd(&mut t, DISPATCH);
        assert_eq!(
            acts,
            [
                Action::Open(EncoderKind::Compute),
                Action::Consumer {
                    after: ALL,
                    before: DISPATCH,
                    vis: VIS_DEVICE
                }
            ]
        );
        // With the head barrier the device-visibility one is covered, and an
        // aliasing one is narrowed to the stages the list actually used.
        let mut t = Tracker::new(true);
        cmd(&mut t, DISPATCH);
        t.barrier(ALL, DISPATCH, VIS_RESOURCE_ALIAS);
        t.split();
        assert_eq!(
            cmd(&mut t, DISPATCH),
            [
                Action::Open(EncoderKind::Compute),
                HEAD,
                Action::Consumer {
                    after: DISPATCH,
                    before: DISPATCH,
                    vis: VIS_RESOURCE_ALIAS
                }
            ]
        );
    }
}
