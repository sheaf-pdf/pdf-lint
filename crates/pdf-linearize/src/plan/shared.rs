//! Each later page's list of shared objects, and each group's where its table
//! lists them, an embedded file's among them (F.4.2, F.4.6, F.4.7).
//!
//! The walk that gave each object its [`Use`](super::Use) goes into an object
//! twice at most, so a third page stops where the second did and learns nothing
//! of what's under what they share. These are walks of their own, under the
//! same stops: one for each list, marking what it's seen with a stamp so that
//! nothing is cleared between walks. So they cost the sum over the lists of
//! what each reaches, and stop at a cap.

use std::ops::Range;

use super::place::{Flat, NONE};
use super::{Error, Group};
use crate::Limits;

/// What the lists are made from.
pub(super) struct Walks<'a> {
    pub flat: &'a Flat<'a>,
    pub stop: &'a [bool],
    /// Each object's entry in the shared object table, or none.
    pub entry: &'a [u32],
    pub pages: &'a [u32],
    pub first: usize,
    /// Each group's roots, where its table lists shared objects.
    pub groups: &'a [Vec<u32>; Group::ALL.len()],
    /// Each embedded file's root.
    pub files: &'a [u32],
    pub entries: usize,
    pub limits: &'a Limits,
}

/// The lists, one after another, and each page's and group's range of them.
pub(super) struct Lists {
    pub listed: Vec<u32>,
    pub pages: Vec<Range<usize>>,
    pub groups: [Range<usize>; Group::ALL.len()],
    pub files: Vec<Range<usize>>,
    pub visits: u64,
}

/// A walk's state, kept between them.
struct Walker<'a> {
    walks: &'a Walks<'a>,
    /// The walk each object and each entry was last met on.
    object_met: Vec<u32>,
    entry_met: Vec<u32>,
    walk: u32,
    stack: Vec<u32>,
    listed: Vec<u32>,
    visits: u64,
}

/// Every list `walks` asks for, or the cap that stopped them.
pub(super) fn lists(walks: Walks<'_>) -> Result<Lists, Error> {
    let mut walker = Walker {
        walks: &walks,
        object_met: vec![0; walks.flat.ids.len()],
        entry_met: vec![0; walks.entries],
        walk: 0,
        stack: Vec::new(),
        listed: Vec::new(),
        visits: 0,
    };
    let mut pages = Vec::with_capacity(walks.pages.len());
    for (i, &page) in walks.pages.iter().enumerate() {
        pages.push(match i == walks.first {
            true => 0..0,
            false => walker.walk(&[page], Some(page))?,
        });
    }
    let mut groups = [const { 0..0 }; Group::ALL.len()];
    for (list, roots) in groups.iter_mut().zip(walks.groups) {
        if !roots.is_empty() {
            *list = walker.walk(roots, None)?;
        }
    }
    let mut files = Vec::with_capacity(walks.files.len());
    for &root in walks.files {
        files.push(walker.walk(&[root], None)?);
    }
    Ok(Lists {
        listed: walker.listed,
        pages,
        groups,
        files,
        visits: walker.visits,
    })
}

impl Walker<'_> {
    /// The entries of what `roots` reach, listed in order, `page` being the
    /// page walked from, if one is, which a walk may enter though it's a stop.
    fn walk(&mut self, roots: &[u32], page: Option<u32>) -> Result<Range<usize>, Error> {
        let walks = self.walks;
        let start = self.listed.len();
        self.walk += 1;
        for &root in roots {
            self.meet(root, page);
        }
        while let Some(at) = self.stack.pop() {
            let refs = walks.flat.refs(at);
            self.visits = self.visits.saturating_add(1 + refs.len() as u64);
            if self.visits > walks.limits.shared_visits {
                return Err(Error::TooManySharedVisits {
                    visits: self.visits,
                    limit: walks.limits.shared_visits,
                });
            }
            let entry = walks.entry[at as usize];
            if entry != NONE {
                let met = &mut self.entry_met[entry as usize];
                if std::mem::replace(met, self.walk) != self.walk {
                    self.listed.push(entry);
                    if self.listed.len() > walks.limits.shared_entries {
                        return Err(Error::TooManySharedEntries {
                            entries: self.listed.len(),
                            limit: walks.limits.shared_entries,
                        });
                    }
                }
            }
            for &r in refs {
                self.meet(r, page);
            }
        }
        self.listed[start..].sort_unstable();
        Ok(start..self.listed.len())
    }

    /// Object `at`, put on the stack if this walk may go into it and hasn't.
    fn meet(&mut self, at: u32, page: Option<u32>) {
        if Some(at) == page || !self.walks.stop[at as usize] {
            let met = &mut self.object_met[at as usize];
            if std::mem::replace(met, self.walk) != self.walk {
                self.stack.push(at);
            }
        }
    }
}
