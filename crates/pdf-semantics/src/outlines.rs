//! Document outline / bookmarks (ISO 32000-2, §12.3.3, Tables 150–152).
//!
//! [`read`] walks a catalog's `/Outlines` doubly-linked hierarchy under
//! [`Limits::outline_depth`] and [`Limits::outline_items`], guarding against
//! sibling and ancestor reference loops while checking signed `/Count` totals
//! against visible descendants. [`write()`] constructs a complete doubly-linked
//! `/Outlines` root and item dictionaries over caller-supplied [`ObjectId`]s.

use std::collections::HashSet;
use std::fmt;

use pdf_objects::fault::{Place, Sink};
use pdf_objects::pdf_tokenizer::Rules;
use pdf_objects::{Dict, Object, ObjectId, Resolve, Span, Str, Unreadable, Value, Why, dict, text};

use crate::action::{self, Action};
use crate::dest::{self, Dest};
use crate::info::Len;
use crate::phases::phase;
use crate::{ByRef, Fault, HOPS, Limits, too_long};

/// Visual style flags on an outline item (`/F`, ISO 32000-2 §12.3.3 Table 152).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct OutlineFlags(u32);

impl OutlineFlags {
    /// No style flags set (`0`).
    pub const NONE: Self = Self(0);
    /// Bit 1 (`1`): display item title in italic.
    pub const ITALIC: Self = Self(1 << 0);
    /// Bit 2 (`2`): display item title in bold.
    pub const BOLD: Self = Self(1 << 1);

    /// Wraps raw `/F` integer bits (preserving any higher bits as PDFium's
    /// `FPDFBookmark_GetStyle` does).
    #[must_use]
    pub const fn from_bits(bits: u32) -> Self {
        Self(bits)
    }

    /// Raw `/F` integer bits.
    #[must_use]
    pub const fn bits(self) -> u32 {
        self.0
    }

    /// Returns `true` if [`Self::ITALIC`] is set.
    #[must_use]
    pub const fn is_italic(self) -> bool {
        (self.0 & Self::ITALIC.0) != 0
    }

    /// Returns `true` if [`Self::BOLD`] is set.
    #[must_use]
    pub const fn is_bold(self) -> bool {
        (self.0 & Self::BOLD.0) != 0
    }

    /// Combines two flag masks.
    #[must_use]
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}

/// One decoded item in the document outline hierarchy (ISO 32000-2 §12.3.3
/// Table 151).
///
/// Its `Debug` implementation reports `title`'s byte length rather than the
/// decoded title text, which may come from an encrypted document.
#[derive(Clone, PartialEq)]
#[non_exhaustive]
pub struct OutlineItem {
    /// Indirect object ID of the outline item dictionary, if indirect.
    pub id: Option<ObjectId>,
    /// Decoded `/Title` text (`""` when `/Title` is absent or malformed).
    pub title: String,
    /// Whether child items are expanded by default (`true` unless `/Count < 0`).
    pub open: bool,
    /// Raw `/Count` integer written on the item dictionary, if present.
    pub raw_count: Option<i64>,
    /// Target destination (`/Dest`), if present.
    pub dest: Option<Dest>,
    /// Target action chain (`/A`), if present.
    pub actions: Vec<Action>,
    /// Optional RGB title color (`/C`, components in `0.0..=1.0`).
    pub color: Option<[f32; 3]>,
    /// Title style flags (`/F`).
    pub flags: OutlineFlags,
    /// Associated structure element (`/SE`), if an indirect reference.
    pub struct_elem: Option<ObjectId>,
    /// Direct child outline items in `/First` -> `/Next` order.
    pub children: Vec<OutlineItem>,
}

impl fmt::Debug for OutlineItem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OutlineItem")
            .field("id", &self.id)
            .field("title", &Len(self.title.len()))
            .field("open", &self.open)
            .field("raw_count", &self.raw_count)
            .field("has_dest", &self.dest.is_some())
            .field("actions", &self.actions.len())
            .field("color", &self.color)
            .field("flags", &self.flags)
            .field("struct_elem", &self.struct_elem)
            .field("children", &self.children.len())
            .finish()
    }
}

impl OutlineItem {
    /// Total number of descendants that are initially visible underneath this
    /// item (`0` if [`Self::open`] is `false` or [`Self::children`] is empty).
    #[must_use]
    pub fn visible_descendants(&self) -> usize {
        if !self.open || self.children.is_empty() {
            return 0;
        }
        self.children
            .iter()
            .map(|child| 1 + child.visible_descendants())
            .sum()
    }
}

/// Hard failure while reading `/Outlines`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// An indirect object or encrypted string could not be read.
    #[error("{0}")]
    Unreadable(#[from] Unreadable),
    /// An action chain on an outline item exceeded [`Limits::action_chain`].
    #[error("an outline item's /A chain exceeds the configured action_chain limit")]
    ActionTooLong,
}

impl From<action::Error> for Error {
    fn from(err: action::Error) -> Self {
        match err {
            action::Error::Unreadable(u) => Self::Unreadable(u),
            action::Error::TooLong => Self::ActionTooLong,
        }
    }
}

/// Reads the document outline hierarchy from `catalog`'s `/Outlines` entry
/// (ISO 32000-2 §12.3.3).
///
/// Returns `Ok(None)` when `/Outlines` is absent, `null`, a dangling reference,
/// or not a dictionary (emitting [`Fault::OutlineInvalid`] in the last case).
///
/// # Errors
///
/// Returns [`Error`] on reader caps, locked objects/strings, or action chains
/// exceeding [`Limits::action_chain`].
pub fn read<'o, R: Resolve<'o>, S: Sink<Fault> + ?Sized>(
    objects: R,
    catalog: &'o Dict,
    rules: Rules,
    limits: &Limits,
    sink: &mut S,
) -> Result<Option<Vec<OutlineItem>>, Error> {
    let Some(raw_root) = catalog.get(b"Outlines") else {
        return Ok(None);
    };
    let mut w = Walk {
        objects: &objects,
        rules,
        limits,
        sink,
        visited: HashSet::new(),
        items_read: 0,
        capped: false,
    };
    let Some((root_obj, root_id)) = w.reached(raw_root, None)? else {
        return Ok(None);
    };
    let Some(root_dict) = root_obj.as_dict() else {
        w.say(root_id, root_obj.at(), Fault::OutlineInvalid);
        return Ok(None);
    };
    if let Some(id) = root_id {
        w.visited.insert(id);
    }

    let raw_root_count = w.int_entry(root_dict, root_id, "Count")?;
    let (items, visible) = w.read_siblings(root_dict.get(b"First"), root_id, 0)?;

    if w.sink.wants() && !w.capped {
        let expected = i64::try_from(visible).unwrap_or(i64::MAX);
        if let Some(c) = raw_root_count {
            if c < 0 || c != expected {
                w.say(
                    root_id,
                    root_dict.get(b"Count").and_then(Object::at),
                    Fault::OutlineCount {
                        count: Some(c),
                        expected,
                    },
                );
            }
        } else if !items.is_empty() && dict_has_non_null(root_dict, b"Count") {
            w.say(
                root_id,
                root_dict.get(b"Count").and_then(Object::at),
                Fault::OutlineCount {
                    count: None,
                    expected,
                },
            );
        }
    }

    phase!(DEBUG, "outlines::read", items = w.items_read);
    Ok(Some(items))
}

fn dict_has_non_null(dict: &Dict, key: &[u8]) -> bool {
    dict.get(key).is_some_and(|o| !o.is_null())
}

struct Walk<'a, 's, R, S: ?Sized> {
    objects: &'a R,
    rules: Rules,
    limits: &'a Limits,
    sink: &'s mut S,
    visited: HashSet<ObjectId>,
    items_read: usize,
    capped: bool,
}

impl<'o, R: Resolve<'o>, S: Sink<Fault> + ?Sized> Walk<'_, '_, R, S> {
    fn say(&mut self, id: Option<ObjectId>, at: Option<Span>, fault: Fault) {
        if self.sink.wants() {
            self.sink.fault(Place::new(id, at), fault);
        }
    }

    fn reached(
        &mut self,
        value: &'o Object,
        within: Option<ObjectId>,
    ) -> Result<Option<(&'o Object, Option<ObjectId>)>, Unreadable> {
        let (mut at, mut last) = (value, None);
        for _ in 0..HOPS {
            match at.value() {
                Value::Ref(id) => {
                    last = Some(*id);
                    match self.objects.try_resolve(*id) {
                        Ok(Some(obj)) => at = obj,
                        Ok(None) => return Ok(None),
                        Err(u) if u.why == Why::Damaged => {
                            self.say(within, value.at(), Fault::Damaged { id: *id });
                            return Ok(None);
                        }
                        Err(u) => return Err(u),
                    }
                }
                Value::Null => return Ok(None),
                _ => return Ok(Some((at, last.or(within)))),
            }
        }
        Err(too_long(last.unwrap_or(ObjectId::new(0, 0))))
    }

    fn read_siblings(
        &mut self,
        first: Option<&'o Object>,
        parent_id: Option<ObjectId>,
        depth: usize,
    ) -> Result<(Vec<OutlineItem>, usize), Error> {
        let Some(mut cursor) = first else {
            return Ok((Vec::new(), 0));
        };
        if depth >= self.limits.outline_depth {
            self.capped = true;
            self.say(parent_id, cursor.at(), Fault::OutlineTooDeep);
            return Ok((Vec::new(), 0));
        }

        let mut items = Vec::new();
        let mut visible_total = 0usize;

        loop {
            if self.items_read >= self.limits.outline_items {
                self.capped = true;
                self.say(parent_id, cursor.at(), Fault::OutlineTooManyItems);
                break;
            }
            let Some((item_obj, item_id)) = self.reached(cursor, parent_id)? else {
                break;
            };
            if let Some(id) = item_id
                && !self.visited.insert(id)
            {
                self.say(parent_id, cursor.at(), Fault::OutlineCycle { id });
                break;
            }
            let Some(dict) = item_obj.as_dict() else {
                self.say(item_id, item_obj.at(), Fault::OutlineInvalid);
                break;
            };

            self.items_read += 1;
            let (item, item_visible_under) = self.read_item(dict, item_id, depth)?;
            visible_total += 1 + if item.open { item_visible_under } else { 0 };
            items.push(item);

            match dict.get(b"Next") {
                Some(next) => cursor = next,
                None => break,
            }
        }

        Ok((items, visible_total))
    }

    fn read_item(
        &mut self,
        dict: &'o Dict,
        id: Option<ObjectId>,
        depth: usize,
    ) -> Result<(OutlineItem, usize), Error> {
        let title = match dict.get(b"Title") {
            Some(raw) => match self.reached(raw, id)? {
                Some((obj, tid)) => match obj.value() {
                    Value::String(Str::Bytes(bytes), _) => text::decode_str(bytes, self.rules),
                    Value::String(Str::Encrypted(_), _) => {
                        let oid = tid.unwrap_or(ObjectId::new(0, 0));
                        return Err(Error::Unreadable(Unreadable::new(
                            oid,
                            Why::Locked,
                            format!("{oid} R holds /Title as ciphertext: it takes a password"),
                        )));
                    }
                    _ => {
                        self.say(tid, obj.at(), Fault::OutlineWrongType { key: "Title" });
                        String::new()
                    }
                },
                None => {
                    self.say(id, raw.at(), Fault::OutlineInvalid);
                    String::new()
                }
            },
            None => {
                self.say(id, None, Fault::OutlineInvalid);
                String::new()
            }
        };

        let raw_count = self.int_entry(dict, id, "Count")?;
        let has_dest = dict_has_non_null(dict, b"Dest");
        let has_a = dict_has_non_null(dict, b"A");
        if has_dest && has_a {
            self.say(id, None, Fault::OutlineBothDestAndAction);
        }
        let dest = match dict.get(b"Dest") {
            Some(raw) => dest::read(ByRef(self.objects), raw, id, true, self.rules, self.sink)?,
            None => None,
        };
        let actions = match dict.get(b"A") {
            Some(raw) => action::read_chain(
                ByRef(self.objects),
                raw,
                id,
                self.rules,
                self.limits,
                self.sink,
            )?,
            None => Vec::new(),
        };
        let color = self.color_entry(dict, id)?;
        let flags = self
            .int_entry(dict, id, "F")?
            .map_or(OutlineFlags::NONE, |v| OutlineFlags::from_bits(v as u32));
        let struct_elem = match dict.get(b"SE") {
            Some(raw) => match raw.as_reference() {
                Some(se) => Some(se),
                None if !raw.is_null() => {
                    self.say(id, raw.at(), Fault::OutlineWrongType { key: "SE" });
                    None
                }
                None => None,
            },
            None => None,
        };

        let (children, child_visible) = self.read_siblings(dict.get(b"First"), id, depth + 1)?;
        let open = !raw_count.is_some_and(|c| c < 0);

        if self.sink.wants() && !self.capped {
            if children.is_empty() {
                if let Some(c) = raw_count
                    && c != 0
                {
                    self.say(
                        id,
                        dict.get(b"Count").and_then(Object::at),
                        Fault::OutlineCount {
                            count: Some(c),
                            expected: 0,
                        },
                    );
                }
            } else {
                let vis_i64 = i64::try_from(child_visible).unwrap_or(i64::MAX);
                let expected = if open { vis_i64 } else { -vis_i64 };
                if raw_count != Some(expected) {
                    self.say(
                        id,
                        dict.get(b"Count").and_then(Object::at),
                        Fault::OutlineCount {
                            count: raw_count,
                            expected,
                        },
                    );
                }
            }
        }

        let visible_under = if open { child_visible } else { 0 };
        Ok((
            OutlineItem {
                id,
                title,
                open,
                raw_count,
                dest,
                actions,
                color,
                flags,
                struct_elem,
                children,
            },
            visible_under,
        ))
    }

    fn int_entry(
        &mut self,
        dict: &'o Dict,
        within: Option<ObjectId>,
        key: &'static str,
    ) -> Result<Option<i64>, Unreadable> {
        let Some(raw) = dict.get(key.as_bytes()) else {
            return Ok(None);
        };
        let Some((obj, id)) = self.reached(raw, within)? else {
            return Ok(None);
        };
        match obj.as_integer() {
            Some(v) => Ok(Some(v)),
            None => {
                self.say(id, obj.at(), Fault::OutlineWrongType { key });
                Ok(None)
            }
        }
    }

    fn color_entry(
        &mut self,
        dict: &'o Dict,
        within: Option<ObjectId>,
    ) -> Result<Option<[f32; 3]>, Unreadable> {
        let Some(raw) = dict.get(b"C") else {
            return Ok(None);
        };
        let Some((obj, id)) = self.reached(raw, within)? else {
            return Ok(None);
        };
        let Some(arr) = obj.as_array() else {
            self.say(id, obj.at(), Fault::OutlineWrongType { key: "C" });
            return Ok(None);
        };
        if arr.len() != 3 {
            self.say(id, obj.at(), Fault::OutlineWrongType { key: "C" });
            return Ok(None);
        }
        let mut rgb = [0.0f32; 3];
        for (slot, item) in rgb.iter_mut().zip(arr) {
            let Some((num_obj, _)) = self.reached(item, id)? else {
                self.say(id, item.at(), Fault::OutlineWrongType { key: "C" });
                return Ok(None);
            };
            let Some(v) = num_obj.as_number() else {
                self.say(id, num_obj.at(), Fault::OutlineWrongType { key: "C" });
                return Ok(None);
            };
            let vf = v as f32;
            if !vf.is_finite() || !(0.0..=1.0).contains(&vf) {
                self.say(id, num_obj.at(), Fault::OutlineWrongType { key: "C" });
                return Ok(None);
            }
            *slot = vf;
        }
        Ok(Some(rgb))
    }
}

/// Target navigation entry for an [`OutlineNode`] builder.
#[derive(Clone, PartialEq)]
pub enum OutlineTarget {
    /// `/Dest`: jump to an explicit or named destination.
    Dest(Dest),
    /// `/A`: execute an action dictionary (e.g. `/S /URI`).
    Action(Dict),
}

impl fmt::Debug for OutlineTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Dest(d) => f.debug_tuple("Dest").field(d).finish(),
            Self::Action(d) => f.debug_struct("Action").field("entries", &d.len()).finish(),
        }
    }
}

/// An outline item to serialize via [`write()`].
///
/// Its `Debug` implementation reports `title`'s byte length rather than raw text.
#[derive(Clone, PartialEq)]
pub struct OutlineNode {
    /// Bookmark `/Title`.
    pub title: String,
    /// Whether child items start expanded (`true` by default).
    pub open: bool,
    /// Optional `/Dest` or `/A` target.
    pub target: Option<OutlineTarget>,
    /// Optional `/C` RGB color (`0.0..=1.0`).
    pub color: Option<[f32; 3]>,
    /// `/F` style flags.
    pub flags: OutlineFlags,
    /// Optional `/SE` structure element reference.
    pub struct_elem: Option<ObjectId>,
    /// Child outline items.
    pub children: Vec<OutlineNode>,
}

impl fmt::Debug for OutlineNode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OutlineNode")
            .field("title", &Len(self.title.len()))
            .field("open", &self.open)
            .field("target", &self.target)
            .field("color", &self.color)
            .field("flags", &self.flags)
            .field("struct_elem", &self.struct_elem)
            .field("children", &self.children.len())
            .finish()
    }
}

impl OutlineNode {
    /// Creates an expanded outline node with `title` and no children.
    #[must_use]
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            open: true,
            target: None,
            color: None,
            flags: OutlineFlags::NONE,
            struct_elem: None,
            children: Vec::new(),
        }
    }

    /// Sets a `/Dest` destination target.
    #[must_use]
    pub fn with_dest(mut self, dest: Dest) -> Self {
        self.target = Some(OutlineTarget::Dest(dest));
        self
    }

    /// Sets a `/S /URI` action target.
    #[must_use]
    pub fn with_uri(mut self, uri: impl AsRef<[u8]>) -> Self {
        self.target = Some(OutlineTarget::Action(dict! {
            "Type" => Object::name("Action"),
            "S" => Object::name("URI"),
            "URI" => Object::bytes(uri.as_ref()),
        }));
        self
    }

    /// Marks this node's children collapsed (`open = false`).
    #[must_use]
    pub const fn closed(mut self) -> Self {
        self.open = false;
        self
    }

    /// Sets RGB title color (`0.0..=1.0`).
    #[must_use]
    pub const fn with_color(mut self, rgb: [f32; 3]) -> Self {
        self.color = Some(rgb);
        self
    }

    /// Sets `/F` style flags.
    #[must_use]
    pub const fn with_flags(mut self, flags: OutlineFlags) -> Self {
        self.flags = flags;
        self
    }

    /// Appends child outline nodes.
    #[must_use]
    pub fn with_children(mut self, children: impl IntoIterator<Item = OutlineNode>) -> Self {
        self.children.extend(children);
        self
    }
}

/// Output of [`write()`]: the `/Outlines` root ID and every dictionary to insert
/// into a document.
#[derive(Debug, Clone, PartialEq)]
pub struct WrittenOutlines {
    /// Indirect object ID of the `/Type /Outlines` root dictionary (wire to
    /// catalog `/Outlines`).
    pub root: ObjectId,
    /// `(id, dict)` pairs for the `/Outlines` root and all outline items.
    pub nodes: Vec<(ObjectId, Dict)>,
}

/// Reason [`write()`] refused an outline tree.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum WriteError {
    /// Tree depth exceeded [`Limits::outline_depth`].
    #[error("the outline tree exceeds Limits::outline_depth ({max})")]
    TooDeep {
        /// Configured depth limit.
        max: usize,
    },
    /// Total item count exceeded [`Limits::outline_items`].
    #[error("the outline tree exceeds Limits::outline_items ({max})")]
    TooManyItems {
        /// Configured item limit.
        max: usize,
    },
    /// An RGB component in `/C` is non-finite or outside `0.0..=1.0`.
    #[error("an outline item's /C RGB component is outside 0.0..=1.0")]
    InvalidColor,
    /// `next_id` returned a duplicate [`ObjectId`].
    #[error("the ObjectId allocator returned {id} R twice")]
    DuplicateId {
        /// Repeated ID.
        id: ObjectId,
    },
}

/// Builds a doubly-linked `/Outlines` dictionary hierarchy from `items`.
///
/// # Errors
///
/// Returns [`WriteError`] if `items` exceeds [`Limits`], contains an invalid
/// RGB color, or `next_id` yields a duplicate [`ObjectId`].
pub fn write(
    items: &[OutlineNode],
    limits: &Limits,
    mut next_id: impl FnMut() -> ObjectId,
) -> Result<WrittenOutlines, WriteError> {
    let mut seen = HashSet::new();
    let mut alloc = || {
        let id = next_id();
        if !seen.insert(id) {
            return Err(WriteError::DuplicateId { id });
        }
        Ok(id)
    };

    let root = alloc()?;
    let mut count_items = 0usize;
    let mut out = Vec::new();
    let root_slot = out.len();
    out.push((root, Dict::new()));

    let (first_last, visible) = write_siblings(
        items,
        root,
        0,
        limits,
        &mut count_items,
        &mut alloc,
        &mut out,
    )?;

    let mut root_dict = dict! {
        "Type" => Object::name("Outlines"),
    };
    if let Some((first, last)) = first_last {
        root_dict.set("First", first);
        root_dict.set("Last", last);
        root_dict.set("Count", i64::try_from(visible).unwrap_or(i64::MAX));
    }
    out[root_slot].1 = root_dict;

    Ok(WrittenOutlines { root, nodes: out })
}

fn write_siblings(
    items: &[OutlineNode],
    parent: ObjectId,
    depth: usize,
    limits: &Limits,
    count_items: &mut usize,
    alloc: &mut impl FnMut() -> Result<ObjectId, WriteError>,
    out: &mut Vec<(ObjectId, Dict)>,
) -> Result<(Option<(ObjectId, ObjectId)>, usize), WriteError> {
    if items.is_empty() {
        return Ok((None, 0));
    }
    if depth >= limits.outline_depth {
        return Err(WriteError::TooDeep {
            max: limits.outline_depth,
        });
    }
    if count_items.saturating_add(items.len()) > limits.outline_items {
        return Err(WriteError::TooManyItems {
            max: limits.outline_items,
        });
    }
    *count_items += items.len();

    let mut ids = Vec::with_capacity(items.len());
    let mut slots = Vec::with_capacity(items.len());
    for _ in items {
        let id = alloc()?;
        ids.push(id);
        slots.push(out.len());
        out.push((id, Dict::new()));
    }

    let mut total_visible = 0usize;
    for (idx, node) in items.iter().enumerate() {
        let id = ids[idx];
        let (child_fl, child_vis) = write_siblings(
            &node.children,
            id,
            depth + 1,
            limits,
            count_items,
            alloc,
            out,
        )?;

        let mut d = dict! {
            "Title" => Object::text(&node.title),
            "Parent" => parent,
        };
        if idx > 0 {
            d.set("Prev", ids[idx - 1]);
        }
        if idx + 1 < ids.len() {
            d.set("Next", ids[idx + 1]);
        }
        if let Some((c_first, c_last)) = child_fl {
            d.set("First", c_first);
            d.set("Last", c_last);
            let vis_i64 = i64::try_from(child_vis).unwrap_or(i64::MAX);
            d.set("Count", if node.open { vis_i64 } else { -vis_i64 });
        }
        match &node.target {
            Some(OutlineTarget::Dest(dest)) => {
                d.set("Dest", dest_to_object(dest));
            }
            Some(OutlineTarget::Action(act)) => {
                d.set("A", act.clone());
            }
            None => {}
        }
        if let Some([r, g, b]) = node.color {
            if ![r, g, b]
                .iter()
                .all(|c| c.is_finite() && (0.0..=1.0).contains(c))
            {
                return Err(WriteError::InvalidColor);
            }
            d.set(
                "C",
                vec![
                    Object::from(f64::from(r)),
                    Object::from(f64::from(g)),
                    Object::from(f64::from(b)),
                ],
            );
        }
        if node.flags.bits() != 0 {
            d.set("F", i64::from(node.flags.bits()));
        }
        if let Some(se) = node.struct_elem {
            d.set("SE", se);
        }

        out[slots[idx]].1 = d;
        total_visible += 1 + if node.open { child_vis } else { 0 };
    }

    Ok((ids.first().copied().zip(ids.last().copied()), total_visible))
}

pub(crate) fn dest_to_object(dest: &Dest) -> Object {
    use crate::dest::{DestFit, DestNameKind, PageTarget};

    match dest {
        Dest::Named { name, kind } => match kind {
            DestNameKind::ByteString => Object::bytes(name.as_slice()),
            DestNameKind::Name => Object::name(name.as_slice()),
        },
        Dest::Explicit(exp) => {
            let page_obj = match exp.page {
                PageTarget::PageRef(id) => Object::from(id),
                PageTarget::PageIndex(idx) => Object::from(i64::try_from(idx).unwrap_or(0)),
            };
            let opt = |v: Option<f32>| v.map_or_else(Object::null, |n| Object::from(f64::from(n)));
            let mut arr = vec![page_obj];
            match exp.fit {
                DestFit::XYZ { left, top, zoom } => {
                    arr.push(Object::name("XYZ"));
                    arr.push(opt(left));
                    arr.push(opt(top));
                    arr.push(opt(zoom));
                }
                DestFit::Fit | DestFit::Unknown => arr.push(Object::name("Fit")),
                DestFit::FitH { top } => {
                    arr.push(Object::name("FitH"));
                    arr.push(opt(top));
                }
                DestFit::FitV { left } => {
                    arr.push(Object::name("FitV"));
                    arr.push(opt(left));
                }
                DestFit::FitR {
                    left,
                    bottom,
                    right,
                    top,
                } => {
                    arr.push(Object::name("FitR"));
                    arr.push(Object::from(f64::from(left)));
                    arr.push(Object::from(f64::from(bottom)));
                    arr.push(Object::from(f64::from(right)));
                    arr.push(Object::from(f64::from(top)));
                }
                DestFit::FitB => arr.push(Object::name("FitB")),
                DestFit::FitBH { top } => {
                    arr.push(Object::name("FitBH"));
                    arr.push(opt(top));
                }
                DestFit::FitBV { left } => {
                    arr.push(Object::name("FitBV"));
                    arr.push(opt(left));
                }
            }
            Object::from(arr)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use pdf_objects::pdf_tokenizer::Rules;
    use pdf_objects::{Object, ObjectId, dict};

    use super::{OutlineFlags, OutlineItem, OutlineNode, read, write};
    use crate::dest::Dest;
    use crate::{Fault, Limits};

    #[test]
    fn round_trip_outlines_and_debug_hygiene() {
        let p1 = ObjectId::new(1, 0);
        let p2 = ObjectId::new(2, 0);
        let tree = vec![
            OutlineNode::new("Confidential Executive Summary")
                .with_dest(Dest::fit(p1))
                .with_color([0.25, 0.5, 1.0])
                .with_flags(OutlineFlags::BOLD.union(OutlineFlags::ITALIC))
                .with_children([
                    OutlineNode::new("Section 1.1").with_dest(Dest::xyz(
                        p1,
                        Some(0.0),
                        Some(720.0),
                        None,
                    )),
                    OutlineNode::new("Section 1.2")
                        .closed()
                        .with_children([OutlineNode::new("Hidden Leaf").with_dest(Dest::fit(p2))]),
                ]),
            OutlineNode::new("External Reference").with_uri("https://secret.example/report"),
        ];

        let mut next = 10u32;
        let written = write(&tree, &Limits::DEFAULT, || {
            let id = ObjectId::new(next, 0);
            next += 1;
            id
        })
        .unwrap();

        let mut objects: BTreeMap<ObjectId, Object> = written
            .nodes
            .into_iter()
            .map(|(id, d)| (id, Object::from(d)))
            .collect();
        let catalog = dict! { "Outlines" => written.root };
        objects.insert(p1, Object::from(dict! { "Type" => Object::name("Page") }));
        objects.insert(p2, Object::from(dict! { "Type" => Object::name("Page") }));

        let mut faults = Vec::new();
        let decoded = read(
            &objects,
            &catalog,
            Rules::Iso,
            &Limits::DEFAULT,
            &mut faults,
        )
        .unwrap()
        .unwrap();
        assert!(faults.is_empty(), "{faults:?}");
        assert_eq!(decoded.len(), 2);
        assert_eq!(decoded[0].title, "Confidential Executive Summary");
        assert!(decoded[0].open);
        assert_eq!(decoded[0].visible_descendants(), 2);
        assert_eq!(decoded[0].raw_count, Some(2));
        assert!(decoded[0].flags.is_bold() && decoded[0].flags.is_italic());
        assert_eq!(decoded[0].color, Some([0.25, 0.5, 1.0]));
        assert!(!decoded[0].children[1].open);
        assert_eq!(decoded[0].children[1].raw_count, Some(-1));

        let dbg = format!("{decoded:?} {tree:?}");
        assert!(!dbg.contains("Confidential"), "{dbg}");
        assert!(!dbg.contains("secret.example"), "{dbg}");
    }

    #[test]
    fn cyclic_siblings_and_bad_counts_terminate_and_report_faults() {
        let root_id = ObjectId::new(10, 0);
        let a_id = ObjectId::new(11, 0);
        let b_id = ObjectId::new(12, 0);
        let objects = BTreeMap::from([
            (
                root_id,
                Object::from(dict! {
                    "First" => a_id,
                    "Last" => b_id,
                    "Count" => 99,
                }),
            ),
            (
                a_id,
                Object::from(dict! {
                    "Title" => Object::text("A"),
                    "Next" => b_id,
                    "Count" => 3,
                }),
            ),
            (
                b_id,
                Object::from(dict! {
                    "Title" => Object::text("B"),
                    "Next" => a_id,
                }),
            ),
        ]);
        let catalog = dict! { "Outlines" => root_id };
        let mut faults = Vec::new();
        let items: Vec<OutlineItem> = read(
            &objects,
            &catalog,
            Rules::Pdfium,
            &Limits::DEFAULT,
            &mut faults,
        )
        .unwrap()
        .unwrap();
        assert_eq!(items.len(), 2);
        assert!(
            faults
                .iter()
                .any(|(_, f)| matches!(f, Fault::OutlineCycle { id } if *id == a_id))
        );
        assert!(
            faults
                .iter()
                .any(|(_, f)| matches!(f, Fault::OutlineCount { .. }))
        );
    }
}
