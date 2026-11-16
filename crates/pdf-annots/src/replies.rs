//! Reply chains (`/IRT`, `/RT`) and review/marked state aggregation across a
//! page's annotations (ISO 32000-2 §12.5.6.2 & §12.5.6.4 Table 174).

use std::collections::{HashMap, HashSet};

use pdf_objects::ObjectId;
use pdf_objects::fault::{Place, Sink};

use crate::annot::Annot;
use crate::fault::Fault;
use crate::limits::Limits;
use crate::subtypes::TypedAnnot;

/// Relationship of an `/IRT` annotation to its target (`/RT`, ISO 32000-2
/// §12.5.6.2 Table 172).
///
/// Exhaustive on purpose: `/RT` specifies either `/R` (reply, default) or
/// `/Group`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum ReplyKind {
    /// `/R`: distinct reply comment attached to the target annotation.
    #[default]
    Reply,
    /// `/Group`: grouped together with the target annotation so viewers treat
    /// them as a single logical unit.
    Group,
}

impl ReplyKind {
    #[must_use]
    pub(crate) fn from_name(name: &[u8]) -> Self {
        match name {
            b"Group" => Self::Group,
            _ => Self::Reply,
        }
    }
}

/// Effective `/StateModel (Marked)` state (ISO 32000-2 §12.5.6.4 Table 174).
///
/// Exhaustive on purpose.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum MarkedState {
    /// `/Unmarked` (default initial state).
    #[default]
    Unmarked,
    /// `/Marked`.
    Marked,
}

/// Effective `/StateModel (Review)` state (ISO 32000-2 §12.5.6.4 Table 174).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ReviewState {
    /// `/None` (default initial state).
    #[default]
    None,
    /// `/Accepted`.
    Accepted,
    /// `/Rejected`.
    Rejected,
    /// `/Cancelled`.
    Cancelled,
    /// `/Completed`.
    Completed,
}

impl ReviewState {
    /// Parses a `/State` value under `/StateModel (Review)`.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "None" => Self::None,
            "Accepted" => Self::Accepted,
            "Rejected" => Self::Rejected,
            "Cancelled" => Self::Cancelled,
            "Completed" => Self::Completed,
            _ => return None,
        })
    }
}

/// Resolved reply-tree and effective state summary for one root annotation on a
/// page.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct AnnotThread {
    /// Index of the root annotation in the slice passed to [`resolve_threads`].
    pub root_index: usize,
    /// Indirect `ObjectId` of the root annotation, if numbered.
    pub root_id: Option<ObjectId>,
    /// Indices of direct and transitive `/RT /R` reply annotations (excluding
    /// state-only `/Text` updates), in `/Annots` order.
    pub replies: Vec<usize>,
    /// Indices of `/RT /Group` member annotations, in `/Annots` order.
    pub group_members: Vec<usize>,
    /// Effective `Marked` state after applying all state updates in `/Annots`
    /// order.
    pub marked: MarkedState,
    /// Effective `Review` state after applying all state updates in `/Annots`
    /// order.
    pub review: ReviewState,
}

/// Groups a page's parsed [`Annot`]s into reply threads by `/IRT` and computes
/// each root annotation's effective [`MarkedState`] and [`ReviewState`].
///
/// Cycles or dangling `/IRT` targets report [`Fault::InvalidReplyChain`] to
/// `sink` and treat the affected annotation as a standalone root.
pub fn resolve_threads<S: Sink<Fault> + ?Sized>(
    annots: &[Annot<'_>],
    limits: &Limits,
    sink: &mut S,
) -> Vec<AnnotThread> {
    let mut by_id: HashMap<ObjectId, usize> = HashMap::with_capacity(annots.len());
    for (idx, a) in annots.iter().enumerate() {
        if let Some(id) = a.id() {
            by_id.entry(id).or_insert(idx);
        }
    }

    // Map each annotation index to `(root_index, ReplyKind)`.
    let mut root_of: Vec<usize> = (0..annots.len()).collect();
    let mut visited = HashSet::new();
    for idx in 0..annots.len() {
        let Some(first_irt) = annots[idx].markup().and_then(|m| m.in_reply_to) else {
            continue;
        };
        visited.clear();
        if let Some(self_id) = annots[idx].id() {
            visited.insert(self_id);
        }
        let mut cur_target = first_irt;
        let mut hops = 0usize;
        let mut resolved;
        loop {
            if hops >= limits.reply_depth || !visited.insert(cur_target) {
                if sink.wants() {
                    sink.fault(Place::new(annots[idx].id(), None), Fault::InvalidReplyChain);
                }
                resolved = idx;
                break;
            }
            let Some(&parent_idx) = by_id.get(&cur_target) else {
                if sink.wants() {
                    sink.fault(Place::new(annots[idx].id(), None), Fault::InvalidReplyChain);
                }
                resolved = idx;
                break;
            };
            resolved = parent_idx;
            match annots[parent_idx].markup().and_then(|m| m.in_reply_to) {
                Some(next_irt) => {
                    cur_target = next_irt;
                    hops += 1;
                }
                None => break,
            }
        }
        root_of[idx] = resolved;
    }

    // Popups are attached UI windows, not independent comment threads.
    let mut threads: Vec<AnnotThread> = Vec::new();
    let mut thread_pos: HashMap<usize, usize> = HashMap::new();

    for (idx, annot) in annots.iter().enumerate() {
        if matches!(annot.typed(), TypedAnnot::Popup(_)) {
            continue;
        }
        if root_of[idx] == idx {
            let pos = threads.len();
            thread_pos.insert(idx, pos);
            let mut marked = MarkedState::Unmarked;
            let mut review = ReviewState::None;
            if let TypedAnnot::Text(t) = annot.typed() {
                apply_state(
                    t.state_model.as_deref(),
                    t.state.as_deref(),
                    &mut marked,
                    &mut review,
                );
            }
            threads.push(AnnotThread {
                root_index: idx,
                root_id: annot.id(),
                replies: Vec::new(),
                group_members: Vec::new(),
                marked,
                review,
            });
        }
    }

    for (idx, annot) in annots.iter().enumerate() {
        let root_idx = root_of[idx];
        if root_idx == idx || matches!(annot.typed(), TypedAnnot::Popup(_)) {
            continue;
        }
        let Some(&pos) = thread_pos.get(&root_idx) else {
            continue;
        };
        let thread = &mut threads[pos];
        if let TypedAnnot::Text(t) = annot.typed()
            && (t.state_model.is_some() || t.state.is_some())
        {
            apply_state(
                t.state_model.as_deref(),
                t.state.as_deref(),
                &mut thread.marked,
                &mut thread.review,
            );
            continue;
        }
        let kind = annot.markup().map_or(ReplyKind::Reply, |m| m.reply_kind);
        match kind {
            ReplyKind::Group => thread.group_members.push(idx),
            ReplyKind::Reply => thread.replies.push(idx),
        }
    }

    threads
}

fn apply_state(
    model: Option<&str>,
    state: Option<&str>,
    marked: &mut MarkedState,
    review: &mut ReviewState,
) {
    let Some(state) = state else {
        return;
    };
    match model {
        Some("Marked") => match state {
            "Marked" => *marked = MarkedState::Marked,
            "Unmarked" => *marked = MarkedState::Unmarked,
            _ => {}
        },
        Some("Review") => {
            if let Some(r) = ReviewState::parse(state) {
                *review = r;
            }
        }
        None => {
            if state == "Marked" {
                *marked = MarkedState::Marked;
            } else if state == "Unmarked" {
                *marked = MarkedState::Unmarked;
            } else if let Some(r) = ReviewState::parse(state) {
                *review = r;
            }
        }
        Some(_) => {}
    }
}
