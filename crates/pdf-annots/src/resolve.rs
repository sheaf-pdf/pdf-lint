//! Internal object-following and typed dictionary-entry helpers.

use pdf_gfx::{Point, Quad, Rect};
use pdf_objects::fault::{Place, Sink};
use pdf_objects::pdf_tokenizer::Rules;
use pdf_objects::{Dict, Object, ObjectId, Resolve, Span, Str, Unreadable, Value, Why, text};
use pdf_semantics::date::Date;
use pdf_semantics::info::Dated;

use crate::error::Error;
use crate::fault::Fault;
use crate::limits::{Cap, Limits};

/// Raw 8-float quadrilateral `[x1, y1, x2, y2, x3, y3, x4, y4]` from a
/// `/QuadPoints` array (ISO 32000-2 §12.5.6.10).
///
/// Exhaustive on purpose: a quadrilateral in `/QuadPoints` always holds four
/// `(x, y)` pairs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct QuadPoints(pub [f32; 8]);

impl QuadPoints {
    /// Returns the four points `(p1, p2, p3, p4)` in the exact order written in
    /// `/QuadPoints`.
    #[must_use]
    pub const fn points(self) -> [Point; 4] {
        let [x1, y1, x2, y2, x3, y3, x4, y4] = self.0;
        [
            Point::new(x1, y1),
            Point::new(x2, y2),
            Point::new(x3, y3),
            Point::new(x4, y4),
        ]
    }

    /// Axis-aligned bounding [`Rect`] enclosing all four points regardless of
    /// whether the writer used Z-order or perimeter order.
    #[must_use]
    pub fn bounds(self) -> Rect {
        let pts = self.points();
        let x0 = pts[0].x.min(pts[1].x).min(pts[2].x).min(pts[3].x);
        let y0 = pts[0].y.min(pts[1].y).min(pts[2].y).min(pts[3].y);
        let x1 = pts[0].x.max(pts[1].x).max(pts[2].x).max(pts[3].x);
        let y1 = pts[0].y.max(pts[1].y).max(pts[2].y).max(pts[3].y);
        Rect::new(x0, y0, x1, y1)
    }

    /// Converts a perimeter-ordered [`Quad`] (`[bottom_left, bottom_right,
    /// top_right, top_left]`) into Z-order `/QuadPoints`
    /// (`[top_left, top_right, bottom_left, bottom_right]`), matching Acrobat
    /// and PDFium's `/QuadPoints` vertex convention and round-tripping through
    /// [`Self::to_quad`] for any non-degenerate convex quad.
    #[must_use]
    pub const fn from_quad(Quad([bl, br, tr, tl]): Quad) -> Self {
        Self([tl.x, tl.y, tr.x, tr.y, bl.x, bl.y, br.x, br.y])
    }

    /// Converts an axis-aligned [`Rect`] into Z-order `/QuadPoints`.
    #[must_use]
    pub const fn from_rect(r: Rect) -> Self {
        Self::from_quad(Quad::from_rect(r))
    }

    /// Converts these points into a perimeter-ordered [`Quad`]
    /// (`[bottom_left, bottom_right, top_right, top_left]`).
    ///
    /// Detects crossed Z-order (`(tl, tr, bl, br)` from ISO 32000-1's diagram,
    /// where segments `p1..p2` and `p3..p4` are parallel and `p2..p3` crosses
    /// the interior) and uncrosses the last two vertices so downstream geometry
    /// receives a non-self-intersecting quad.
    #[must_use]
    pub fn to_quad(self) -> Quad {
        let [p1, p2, p3, p4] = self.points();
        if segments_cross(p1, p4, p2, p3) {
            // Z-order `(tl, tr, bl, br)` -> perimeter `(bl, br, tr, tl)`.
            Quad([p3, p4, p2, p1])
        } else {
            Quad([p1, p2, p3, p4])
        }
    }
}

impl From<Quad> for QuadPoints {
    fn from(q: Quad) -> Self {
        Self::from_quad(q)
    }
}

impl From<Rect> for QuadPoints {
    fn from(r: Rect) -> Self {
        Self::from_rect(r)
    }
}

fn segments_cross(a: Point, b: Point, c: Point, d: Point) -> bool {
    let cross =
        |o: Point, u: Point, v: Point| (u.x - o.x) * (v.y - o.y) - (u.y - o.y) * (v.x - o.x);
    let d1 = cross(a, b, c);
    let d2 = cross(a, b, d);
    let d3 = cross(c, d, a);
    let d4 = cross(c, d, b);
    ((d1 > 0.0 && d2 < 0.0) || (d1 < 0.0 && d2 > 0.0))
        && ((d3 > 0.0 && d4 < 0.0) || (d3 < 0.0 && d4 > 0.0))
}

type ArrayRef<'o> = (&'o [Object], Option<ObjectId>);
type TextAt = (String, Option<ObjectId>, Option<Span>);

pub(crate) struct Reader<'s, R, S: ?Sized> {
    pub(crate) objects: R,
    pub(crate) rules: Rules,
    pub(crate) limits: &'s Limits,
    pub(crate) sink: &'s mut S,
}

impl<'o, R: Resolve<'o>, S: Sink<Fault> + ?Sized> Reader<'_, R, S> {
    #[inline]
    pub(crate) fn say(&mut self, within: Option<ObjectId>, span: Option<Span>, fault: Fault) {
        if self.sink.wants() {
            self.sink.fault(Place::new(within, span), fault);
        }
    }

    pub(crate) fn resolve_id(
        &mut self,
        id: ObjectId,
    ) -> Result<Option<(&'o Object, Option<ObjectId>)>, Error> {
        let mut cur_id = id;
        for _ in 0..self.limits.hops {
            match self.objects.try_resolve(cur_id) {
                Ok(Some(object)) => match object.value() {
                    Value::Ref(next) => cur_id = *next,
                    Value::Null => return Ok(None),
                    _ => return Ok(Some((object, Some(cur_id)))),
                },
                Ok(None) => return Ok(None),
                Err(unreadable) if unreadable.why == Why::Damaged => {
                    self.say(Some(cur_id), None, Fault::DamagedObject(cur_id));
                    return Ok(None);
                }
                Err(unreadable) => return Err(Error::Unreadable(unreadable)),
            }
        }
        self.say(Some(cur_id), None, Fault::ReferenceLoop);
        Ok(None)
    }

    pub(crate) fn reached(
        &mut self,
        value: &'o Object,
        within: Option<ObjectId>,
    ) -> Result<Option<(&'o Object, Option<ObjectId>)>, Error> {
        let (mut at, mut last) = (value, None);
        for _ in 0..self.limits.hops {
            match at.value() {
                Value::Ref(id) => {
                    last = Some(*id);
                    match self.objects.try_resolve(*id) {
                        Ok(Some(object)) => at = object,
                        Ok(None) => return Ok(None),
                        Err(unreadable) if unreadable.why == Why::Damaged => {
                            self.say(within, value.at(), Fault::DamagedObject(*id));
                            return Ok(None);
                        }
                        Err(unreadable) => return Err(Error::Unreadable(unreadable)),
                    }
                }
                Value::Null => return Ok(None),
                _ => return Ok(Some((at, last.or(within)))),
            }
        }
        self.say(within, value.at(), Fault::ReferenceLoop);
        Ok(None)
    }

    pub(crate) fn dict_entry(
        &mut self,
        dict: &'o Dict,
        within: Option<ObjectId>,
        key: &'static str,
    ) -> Result<Option<(&'o Dict, Option<ObjectId>)>, Error> {
        let Some(raw) = dict.get(key.as_bytes()) else {
            return Ok(None);
        };
        let Some((obj, id)) = self.reached(raw, within)? else {
            return Ok(None);
        };
        match obj.as_dict() {
            Some(d) => Ok(Some((d, id))),
            None => {
                self.say(id, obj.at(), Fault::WrongType { key });
                Ok(None)
            }
        }
    }

    pub(crate) fn array_entry(
        &mut self,
        dict: &'o Dict,
        within: Option<ObjectId>,
        key: &'static str,
    ) -> Result<Option<ArrayRef<'o>>, Error> {
        let Some(raw) = dict.get(key.as_bytes()) else {
            return Ok(None);
        };
        let Some((obj, id)) = self.reached(raw, within)? else {
            return Ok(None);
        };
        match obj.as_array() {
            Some(arr) => Ok(Some((arr, id))),
            None => {
                self.say(id, obj.at(), Fault::WrongType { key });
                Ok(None)
            }
        }
    }

    pub(crate) fn name_entry(
        &mut self,
        dict: &'o Dict,
        within: Option<ObjectId>,
        key: &'static str,
    ) -> Result<Option<&'o [u8]>, Error> {
        let Some(raw) = dict.get(key.as_bytes()) else {
            return Ok(None);
        };
        let Some((obj, id)) = self.reached(raw, within)? else {
            return Ok(None);
        };
        match obj.as_name() {
            Some(name) => Ok(Some(name)),
            None if self.rules == Rules::Pdfium => match obj.value() {
                Value::String(Str::Bytes(bytes), _) => {
                    self.say(id, obj.at(), Fault::WrongType { key });
                    Ok(Some(bytes.as_slice()))
                }
                Value::String(Str::Encrypted(_), _) => Err(locked_string(id, key)),
                _ => {
                    self.say(id, obj.at(), Fault::WrongType { key });
                    Ok(None)
                }
            },
            None => {
                self.say(id, obj.at(), Fault::WrongType { key });
                Ok(None)
            }
        }
    }

    pub(crate) fn text_entry(
        &mut self,
        dict: &'o Dict,
        within: Option<ObjectId>,
        key: &'static str,
    ) -> Result<Option<String>, Error> {
        let Some(raw) = dict.get(key.as_bytes()) else {
            return Ok(None);
        };
        self.text_value(raw, within, key)
            .map(|v| v.map(|(s, _, _)| s))
    }

    pub(crate) fn text_or_name_entry(
        &mut self,
        dict: &'o Dict,
        within: Option<ObjectId>,
        key: &'static str,
    ) -> Result<Option<String>, Error> {
        let Some(raw) = dict.get(key.as_bytes()) else {
            return Ok(None);
        };
        let Some((obj, id)) = self.reached(raw, within)? else {
            return Ok(None);
        };
        match obj.value() {
            Value::String(Str::Bytes(bytes), _) => Ok(Some(text::decode_str(bytes, self.rules))),
            Value::String(Str::Encrypted(_), _) => Err(locked_string(id, key)),
            Value::Name(bytes) if self.rules == Rules::Pdfium => {
                self.say(id, obj.at(), Fault::WrongType { key });
                Ok(Some(String::from_utf8_lossy(bytes).into_owned()))
            }
            _ => {
                self.say(id, obj.at(), Fault::WrongType { key });
                Ok(None)
            }
        }
    }

    pub(crate) fn byte_string_entry(
        &mut self,
        dict: &'o Dict,
        within: Option<ObjectId>,
        key: &'static str,
    ) -> Result<Option<&'o [u8]>, Error> {
        let Some(raw) = dict.get(key.as_bytes()) else {
            return Ok(None);
        };
        let Some((obj, id)) = self.reached(raw, within)? else {
            return Ok(None);
        };
        match obj.value() {
            Value::String(Str::Bytes(bytes), _) => Ok(Some(bytes.as_slice())),
            Value::String(Str::Encrypted(_), _) => Err(locked_string(id, key)),
            _ => {
                self.say(id, obj.at(), Fault::WrongType { key });
                Ok(None)
            }
        }
    }

    fn text_value(
        &mut self,
        raw: &'o Object,
        within: Option<ObjectId>,
        key: &'static str,
    ) -> Result<Option<TextAt>, Error> {
        let Some((obj, id)) = self.reached(raw, within)? else {
            return Ok(None);
        };
        match obj.value() {
            Value::String(Str::Bytes(bytes), _) => {
                Ok(Some((text::decode_str(bytes, self.rules), id, obj.at())))
            }
            Value::String(Str::Encrypted(_), _) => Err(locked_string(id, key)),
            _ => {
                self.say(id, obj.at(), Fault::WrongType { key });
                Ok(None)
            }
        }
    }

    pub(crate) fn dated_entry(
        &mut self,
        dict: &'o Dict,
        within: Option<ObjectId>,
        key: &'static str,
    ) -> Result<Option<Dated>, Error> {
        let Some(raw) = dict.get(key.as_bytes()) else {
            return Ok(None);
        };
        let Some((s, id, span)) = self.text_value(raw, within, key)? else {
            return Ok(None);
        };
        match Date::read(&s) {
            Ok(d) => Ok(Some(Dated::Date(d))),
            Err(_) => {
                self.say(id, span, Fault::MalformedDate { key });
                Ok(Some(Dated::Text(s)))
            }
        }
    }

    pub(crate) fn bool_entry(
        &mut self,
        dict: &'o Dict,
        within: Option<ObjectId>,
        key: &'static str,
    ) -> Result<Option<bool>, Error> {
        let Some(raw) = dict.get(key.as_bytes()) else {
            return Ok(None);
        };
        let Some((obj, id)) = self.reached(raw, within)? else {
            return Ok(None);
        };
        match obj.as_bool() {
            Some(b) => Ok(Some(b)),
            None => {
                self.say(id, obj.at(), Fault::WrongType { key });
                Ok(None)
            }
        }
    }

    pub(crate) fn int_entry(
        &mut self,
        dict: &'o Dict,
        within: Option<ObjectId>,
        key: &'static str,
    ) -> Result<Option<i64>, Error> {
        let Some(raw) = dict.get(key.as_bytes()) else {
            return Ok(None);
        };
        let Some((obj, id)) = self.reached(raw, within)? else {
            return Ok(None);
        };
        match obj.as_integer() {
            Some(n) => Ok(Some(n)),
            None if self.rules == Rules::Pdfium => match obj.as_number() {
                Some(v) if v.is_finite() => {
                    self.say(id, obj.at(), Fault::WrongType { key });
                    Ok(Some(v as i64))
                }
                _ => {
                    self.say(id, obj.at(), Fault::WrongType { key });
                    Ok(None)
                }
            },
            None => {
                self.say(id, obj.at(), Fault::WrongType { key });
                Ok(None)
            }
        }
    }

    pub(crate) fn float_entry(
        &mut self,
        dict: &'o Dict,
        within: Option<ObjectId>,
        key: &'static str,
    ) -> Result<Option<f32>, Error> {
        let Some(raw) = dict.get(key.as_bytes()) else {
            return Ok(None);
        };
        self.float_value(raw, within, Some(key))
    }

    pub(crate) fn float_value(
        &mut self,
        raw: &'o Object,
        within: Option<ObjectId>,
        key: Option<&'static str>,
    ) -> Result<Option<f32>, Error> {
        let Some((obj, id)) = self.reached(raw, within)? else {
            return Ok(None);
        };
        match obj.as_number() {
            Some(v) => {
                let f = v as f32;
                if f.is_finite() {
                    Ok(Some(f))
                } else {
                    if let Some(key) = key {
                        self.say(id, obj.at(), Fault::WrongType { key });
                    }
                    Ok(None)
                }
            }
            None => {
                if let Some(key) = key {
                    self.say(id, obj.at(), Fault::WrongType { key });
                }
                Ok(None)
            }
        }
    }

    /// Reads a 4-number `[x0, y0, x1, y1]` array, returning both raw `[f32; 4]`
    /// (matching `FPDFAnnot_GetRect`) and normalized [`Rect`].
    pub(crate) fn rect4(
        &mut self,
        dict: &'o Dict,
        within: Option<ObjectId>,
        key: &'static str,
    ) -> Result<Option<([f32; 4], Rect)>, Error> {
        let Some(raw) = dict.get(key.as_bytes()) else {
            return Ok(None);
        };
        let Some((obj, id)) = self.reached(raw, within)? else {
            return Ok(None);
        };
        let Some(arr) = obj.as_array() else {
            if key == "Rect" {
                self.say(id, obj.at(), Fault::InvalidRect);
            } else {
                self.say(id, obj.at(), Fault::WrongType { key });
            }
            return Ok(None);
        };
        if arr.len() != 4 && (self.rules == Rules::Iso || arr.len() < 4) {
            if key == "Rect" {
                self.say(id, obj.at(), Fault::InvalidRect);
            } else {
                self.say(id, obj.at(), Fault::WrongType { key });
            }
            return Ok(None);
        }
        if arr.len() > 4 {
            if key == "Rect" {
                self.say(id, obj.at(), Fault::InvalidRect);
            } else {
                self.say(id, obj.at(), Fault::WrongType { key });
            }
        }
        let mut nums = [0.0f32; 4];
        for (slot, item) in nums.iter_mut().zip(arr.iter().take(4)) {
            match self.float_value(item, id, None)? {
                Some(v) => *slot = v,
                None if self.rules == Rules::Pdfium => {
                    if key == "Rect" {
                        self.say(id, item.at(), Fault::InvalidRect);
                    } else {
                        self.say(id, item.at(), Fault::WrongType { key });
                    }
                    *slot = 0.0;
                }
                None => {
                    if key == "Rect" {
                        self.say(id, item.at(), Fault::InvalidRect);
                    } else {
                        self.say(id, item.at(), Fault::WrongType { key });
                    }
                    return Ok(None);
                }
            }
        }
        let [a, b, c, d] = nums;
        Ok(Some((nums, Rect::new(a, b, c, d))))
    }

    pub(crate) fn quad_points(
        &mut self,
        dict: &'o Dict,
        within: Option<ObjectId>,
    ) -> Result<Option<Vec<QuadPoints>>, Error> {
        let Some(raw) = dict.get(b"QuadPoints") else {
            return Ok(None);
        };
        let Some((obj, id)) = self.reached(raw, within)? else {
            return Ok(None);
        };
        let Some(arr) = obj.as_array() else {
            self.say(id, obj.at(), Fault::InvalidQuadPoints);
            return Ok(None);
        };
        let quad_count = arr.len() / 8;
        if quad_count > self.limits.quads {
            return Err(Error::Limit(Cap::Quads));
        }
        if arr.is_empty() || !arr.len().is_multiple_of(8) {
            self.say(id, obj.at(), Fault::InvalidQuadPoints);
            if self.rules == Rules::Iso || quad_count == 0 {
                return Ok(None);
            }
        }
        let mut out = Vec::with_capacity(quad_count);
        for chunk in arr.as_chunks::<8>().0 {
            let mut vals = [0.0f32; 8];
            for (slot, item) in vals.iter_mut().zip(chunk) {
                match self.float_value(item, id, None)? {
                    Some(v) => *slot = v,
                    None if self.rules == Rules::Pdfium => {
                        self.say(id, item.at(), Fault::InvalidQuadPoints);
                        *slot = 0.0;
                    }
                    None => {
                        self.say(id, item.at(), Fault::InvalidQuadPoints);
                        return Ok(None);
                    }
                }
            }
            out.push(QuadPoints(vals));
        }
        Ok(Some(out))
    }

    pub(crate) fn point_pairs(
        &mut self,
        arr: &'o [Object],
        within: Option<ObjectId>,
        span: Option<Span>,
        key: &'static str,
        budget: &mut usize,
    ) -> Result<Option<Vec<Point>>, Error> {
        let pair_count = arr.len() / 2;
        *budget = budget
            .checked_sub(pair_count)
            .ok_or(Error::Limit(Cap::Points))?;
        if !arr.len().is_multiple_of(2) {
            self.say(within, span, Fault::InvalidPoints { key });
            if self.rules == Rules::Iso {
                return Ok(None);
            }
        }
        let mut pts = Vec::with_capacity(pair_count);
        for chunk in arr.as_chunks::<2>().0 {
            let x = match self.float_value(&chunk[0], within, None)? {
                Some(v) => v,
                None if self.rules == Rules::Pdfium => {
                    self.say(within, chunk[0].at(), Fault::InvalidPoints { key });
                    0.0
                }
                None => {
                    self.say(within, chunk[0].at(), Fault::InvalidPoints { key });
                    return Ok(None);
                }
            };
            let y = match self.float_value(&chunk[1], within, None)? {
                Some(v) => v,
                None if self.rules == Rules::Pdfium => {
                    self.say(within, chunk[1].at(), Fault::InvalidPoints { key });
                    0.0
                }
                None => {
                    self.say(within, chunk[1].at(), Fault::InvalidPoints { key });
                    return Ok(None);
                }
            };
            pts.push(Point::new(x, y));
        }
        Ok(Some(pts))
    }

    pub(crate) fn dest(
        &mut self,
        raw: &'o Object,
        within: Option<ObjectId>,
        local: bool,
    ) -> Result<Option<pdf_semantics::dest::Dest>, Error> {
        let mut fwd = SemanticsSink(self.sink);
        let r: &dyn Resolve<'o> = &self.objects;
        Ok(pdf_semantics::dest::read(
            r, raw, within, local, self.rules, &mut fwd,
        )?)
    }

    pub(crate) fn action_chain(
        &mut self,
        raw: &'o Object,
        within: Option<ObjectId>,
    ) -> Result<Vec<pdf_semantics::action::Action>, Error> {
        let mut fwd = SemanticsSink(self.sink);
        let r: &dyn Resolve<'o> = &self.objects;
        Ok(pdf_semantics::action::read_chain(
            r,
            raw,
            within,
            self.rules,
            &self.limits.semantics,
            &mut fwd,
        )?)
    }

    pub(crate) fn additional_actions(
        &mut self,
        dict: &'o Dict,
        within: Option<ObjectId>,
    ) -> Result<Option<pdf_semantics::action::AdditionalActions>, Error> {
        let mut fwd = SemanticsSink(self.sink);
        let r: &dyn Resolve<'o> = &self.objects;
        Ok(pdf_semantics::action::read_additional(
            r,
            dict,
            within,
            self.rules,
            &self.limits.semantics,
            &mut fwd,
        )?)
    }

    pub(crate) fn file_spec(
        &mut self,
        raw: &'o Object,
        within: Option<ObjectId>,
        key: &'static str,
    ) -> Result<Option<String>, Error> {
        let mut fwd = SemanticsSink(self.sink);
        let r: &dyn Resolve<'o> = &self.objects;
        Ok(pdf_semantics::action::read_file_spec(
            r, raw, within, key, self.rules, &mut fwd,
        )?)
    }
}

struct SemanticsSink<'a, S: ?Sized>(&'a mut S);

impl<S: Sink<Fault> + ?Sized> Sink<pdf_semantics::Fault> for SemanticsSink<'_, S> {
    fn wants(&self) -> bool {
        self.0.wants()
    }

    fn fault(&mut self, place: Place, fault: pdf_semantics::Fault) {
        self.0.fault(place, Fault::Semantics(fault));
    }
}

pub(crate) fn locked_string(id: Option<ObjectId>, key: &str) -> Error {
    let id = id.unwrap_or(ObjectId::new(0, 0));
    Error::Unreadable(Unreadable::new(
        id,
        Why::Locked,
        format!("{id} R holds /{key} as ciphertext: it takes a password"),
    ))
}
