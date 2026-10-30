//! A page's `q`/`Q` balance (ISO 32000-2, 8.4.2): how many graphics states
//! its content leaves saved, and how far it restores states it didn't
//! save, counted from its operations, with no "unknown".

use crate::error::Error;
use crate::inline::ResCx;
use crate::limits::Limits;
use crate::operations::Operation;
use crate::page::Concat;

/// How content's `q`s and `Q`s balance: the net count, and the lowest the
/// running count goes from 0 (never above 0). `q … cm` is `{ net: 1,
/// min_prefix: 0 }`, `Q q` is `{ net: 0, min_prefix: -1 }`. ISO 32000-2
/// asks that they balance, a page's streams read as one (8.4.2): `{ net:
/// 0, min_prefix: 0 }`.
///
/// It's counted an operation at a time ([`take`](Balance::take)), so a
/// caller stepping through content for more than its balance counts it in
/// the same pass; [`balance`] counts a page's.
///
/// ```
/// use pdf_content_streams::{Balance, Concat, Limits, ResCx, balance};
///
/// let page = Concat::new(&[b"Q q q 1 0 0 1 9 9 cm", b"Q"]);
/// let res = ResCx::none();
/// let counted = balance(&page, &res, &Limits::DEFAULT).unwrap();
/// assert_eq!(counted, Balance { net: 0, min_prefix: -1 });
///
/// let mut stepped = Balance::default();
/// for op in page.operations(&res, &Limits::DEFAULT) {
///     stepped.take(&op.unwrap());
/// }
/// assert_eq!(stepped, counted);
/// ```
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Balance {
    /// The `q`s less the `Q`s.
    pub net: i32,
    /// The lowest the running count goes, from 0: below it where a `Q`
    /// restores a state the content didn't save.
    pub min_prefix: i32,
}

impl Balance {
    /// The count after `op`, content's next operation.
    pub fn take(&mut self, op: &Operation) {
        match op.name() {
            b"q" => self.net = self.net.saturating_add(1),
            b"Q" => {
                self.net = self.net.saturating_sub(1);
                self.min_prefix = self.min_prefix.min(self.net);
            }
            _ => {}
        }
    }
}

/// How `content`'s `q`s and `Q`s balance, its operations read with `res`
/// under `limits`; the error that stops them, if one does.
pub fn balance(content: &Concat, res: &ResCx, limits: &Limits) -> Result<Balance, Error> {
    let mut balance = Balance::default();
    for op in content.operations(res, limits) {
        balance.take(&op?);
    }
    Ok(balance)
}
