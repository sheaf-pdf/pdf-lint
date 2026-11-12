//! Which objects share an object stream: grouped by the pages that use
//! them, in the parts Annex F lays a linearized file out in. A pure
//! function of each object's number and its use, with no object model's
//! types in it.

/// The most objects a container holds: qpdf's default.
pub const MEMBERS: usize = 100;

/// Which pages use an object. Exhaustive on purpose: a grouping matches
/// each.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Use {
    /// Only page `i`, from 0.
    Page(u32),
    /// More than one page.
    Shared,
    /// No page: the catalog, the page tree, the form, outlines, names, and
    /// anything no page reaches.
    None,
}

/// The containers `objects` go in, each listed once by number: their
/// members by number, in order. Objects are ordered in parts, as Annex F
/// lays out a linearized file: those only page 1 uses; those a single
/// later page uses, by page; those pages share; then the rest. Numbers
/// ascend within each part, and within each page's. Containers fill in
/// that order, up to [`MEMBERS`] each, and the first part's last container
/// ends with it, so page 1's objects never share one with a later page's.
pub fn group(objects: impl IntoIterator<Item = (u32, Use)>) -> Vec<Vec<u32>> {
    let mut ordered: Vec<((u8, u32), u32)> = (objects.into_iter())
        .map(|(n, used)| (part(used), n))
        .collect();
    ordered.sort_unstable();
    let first = ordered.partition_point(|&((part, _), _)| part == 0);
    let (first, rest) = ordered.split_at(first);
    (first.chunks(MEMBERS).chain(rest.chunks(MEMBERS)))
        .map(|members| members.iter().map(|&(_, n)| n).collect())
        .collect()
}

/// Where an object `used` so goes: its part, and its page in the second.
fn part(used: Use) -> (u8, u32) {
    match used {
        Use::Page(0) => (0, 0),
        Use::Page(i) => (1, i),
        Use::Shared => (2, 0),
        Use::None => (3, 0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Objects go into containers in Annex F's parts: page 1's, apart, then
    /// a single later page's by page, the shared, and the rest, numbers
    /// ascending within each, at most 100 to a container.
    #[test]
    fn the_grouping_keeps_page_one_apart_and_caps_containers() {
        let mut objects: Vec<(u32, Use)> = Vec::new();
        objects.extend((1..=150).map(|n| (n, Use::Page(0))));
        objects.extend((300..310).map(|n| (n, Use::Page(2))));
        objects.extend((400..420).map(|n| (n, Use::Page(1))));
        objects.extend((500..600).map(|n| (n, Use::Shared)));
        objects.extend((600..651).map(|n| (n, Use::None)));
        objects.reverse();
        let got = group(objects);
        let range = |r: std::ops::Range<u32>| r.collect::<Vec<_>>();
        let rest: Vec<u32> = [
            range(400..420),
            range(300..310),
            range(500..600),
            range(600..651),
        ]
        .concat();
        let want = vec![
            range(1..101),
            range(101..151),
            rest[..100].to_vec(),
            rest[100..].to_vec(),
        ];
        assert_eq!(got, want);
        assert!(got.iter().all(|c| c.len() <= MEMBERS));

        // Page 1's part filling its container exactly still ends it, and a
        // file with nothing only page 1 uses starts with the next part.
        let exactly: Vec<(u32, Use)> = (1..=100)
            .map(|n| (n, Use::Page(0)))
            .chain([(101, Use::Shared)])
            .collect();
        assert_eq!(group(exactly), [range(1..101), vec![101]]);
        assert_eq!(group([(7, Use::None), (5, Use::Page(3))]), [vec![5, 7]]);
        assert_eq!(group([]), Vec::<Vec<u32>>::new());
    }
}
