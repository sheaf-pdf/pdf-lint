//! Shared bounded indirect-reference chain walking and stream byte extraction for
//! `space` and `function`.

use std::borrow::Cow;

use pdf_objects::fault::{Place, Sink};
use pdf_objects::{Object, ObjectId, Resolve, Span, Stream, Streams, Unreadable, Why};

use crate::fault::Fault;

/// Outcome of following a chain of indirect object references.
pub(crate) enum Reached<'o> {
    /// Non-reference target object.
    At(&'o Object),
    /// Reference to a missing object (or chained reference rejected under PDFium function rules).
    Nothing,
    /// Reference loop, depth/visit cap, or damaged object (already reported to sink if wanted).
    Stopped,
}

/// Follows indirect references starting from `written`, pushing resolved `ObjectId`s onto `path`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn through_refs<'o, R, S>(
    objects: &R,
    written: &'o Object,
    stop_on_chain: bool,
    max_depth: usize,
    max_visits: usize,
    visits: &mut usize,
    visits_told: &mut bool,
    path: &mut Vec<ObjectId>,
    sink: &mut S,
) -> Result<Reached<'o>, Unreadable>
where
    R: Resolve<'o>,
    S: Sink<Fault> + ?Sized,
{
    let say = |sink: &mut S, path: &[ObjectId], at: Option<Span>, fault: Fault| {
        if sink.wants() {
            sink.fault(Place::new(path.last().copied(), at), fault);
        }
    };

    let mut object = written;
    let mut followed = 0_usize;
    while let Some(id) = object.as_reference() {
        followed += 1;
        if followed == 2 {
            say(sink, path, object.at(), Fault::Chained);
            if stop_on_chain {
                return Ok(Reached::Nothing);
            }
        }
        if path.contains(&id) {
            say(sink, path, object.at(), Fault::Loop);
            return Ok(Reached::Stopped);
        }
        if path.len() > max_depth {
            say(sink, path, object.at(), Fault::TooDeep);
            return Ok(Reached::Stopped);
        }
        if *visits >= max_visits {
            if !std::mem::replace(visits_told, true) {
                say(sink, path, object.at(), Fault::TooManyVisits);
            }
            return Ok(Reached::Stopped);
        }
        *visits += 1;
        match objects.try_resolve(id) {
            Ok(Some(next)) => {
                path.push(id);
                object = next;
            }
            Ok(None) => return Ok(Reached::Nothing),
            Err(e) if e.why == Why::Damaged => {
                say(sink, path, object.at(), Fault::Damaged { id });
                return Ok(Reached::Stopped);
            }
            Err(e) => return Err(e),
        }
    }
    Ok(Reached::At(object))
}

/// Failure category when decoding a numbered or direct in-memory [`Stream`].
pub(crate) enum StreamDecodeErr {
    /// Decoded data exceeded `limit` bytes.
    Limit,
    /// Stream was damaged, unfilterable without an `ObjectId`, or detached.
    Corrupt,
    /// Non-recoverable reader failure (`Locked`, `Capped`, `Source`, `Detached`).
    Unreadable(Unreadable),
}

/// Extracts up to `limit` decoded bytes from `stream` (using `streams.decoded` when `id` is
/// `Some`, or borrowing raw unfiltered bytes when `id` is `None`).
pub(crate) fn decode_stream_or_inline<'a, S: Streams + ?Sized>(
    streams: &'a S,
    stream: &'a Stream,
    id: Option<ObjectId>,
    limit: usize,
) -> Result<Cow<'a, [u8]>, StreamDecodeErr> {
    if let Some(id) = id {
        match streams.decoded(id, limit) {
            Ok(bytes) => Ok(bytes),
            Err(e) if e.why == Why::Limit => Err(StreamDecodeErr::Limit),
            Err(e) if matches!(e.why, Why::Damaged | Why::Filter) => Err(StreamDecodeErr::Corrupt),
            Err(e) => Err(StreamDecodeErr::Unreadable(e)),
        }
    } else {
        if stream.dict.has(b"Filter") {
            return Err(StreamDecodeErr::Corrupt);
        }
        let Some(bytes) = stream.data.bytes() else {
            return Err(StreamDecodeErr::Corrupt);
        };
        if bytes.len() > limit {
            return Err(StreamDecodeErr::Limit);
        }
        Ok(Cow::Borrowed(bytes))
    }
}
