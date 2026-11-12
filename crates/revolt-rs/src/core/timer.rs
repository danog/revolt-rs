//! The timer queue: a binary min-heap of (expiration, insertion sequence) with each record's
//! position kept in the record (O(log n) removal, as Revolt's TimerQueue). Equal expirations
//! fire in insertion order. Entries carry their record's slot: moving one updates the record's
//! position without a lookup.

use super::{Cid, Records};

#[derive(Clone, Copy)]
struct Entry {
    exp: f64,
    seq: u64,
    id: u64,
    slot: u32,
}

impl Entry {
    #[inline(always)]
    fn lt(&self, o: &Entry) -> bool {
        self.exp < o.exp || (self.exp == o.exp && self.seq < o.seq)
    }
}

#[derive(Default)]
pub struct Heap {
    v: Vec<Entry>,
    seq: u64,
}

impl Heap {
    /// The record of `e` (if it is still there: one being removed may be gone already) is at
    /// `pos` now.
    #[inline(always)]
    fn set_pos(recs: &mut Records, e: &Entry, pos: usize) {
        if let Some(r) = recs.at_id(Cid { id: e.id, slot: e.slot }) {
            r.heap_pos = pos;
        }
    }

    pub fn len(&self) -> usize {
        self.v.len()
    }

    /// Inserts the record in `slot` at its expiration.
    pub fn insert(&mut self, recs: &mut Records, slot: u32) {
        let r = recs.at(slot);
        debug_assert!(r.heap_pos == usize::MAX);
        self.seq += 1;
        let n = self.v.len();
        r.heap_pos = n;
        self.v.push(Entry { exp: r.expiration, seq: self.seq, id: r.id, slot });
        self.up(recs, n);
    }

    pub fn peek(&self) -> Option<f64> {
        self.v.first().map(|e| e.exp)
    }

    /// Removes and returns the first timer if it expired at `now`.
    pub fn pop_expired(&mut self, recs: &mut Records, now: f64) -> Option<Cid> {
        let e = *self.v.first()?;
        if e.exp > now {
            return None;
        }
        self.remove(recs, 0);
        Some(Cid { id: e.id, slot: e.slot })
    }

    pub fn remove(&mut self, recs: &mut Records, pos: usize) {
        if pos >= self.v.len() {
            return;
        }
        let gone = self.v[pos];
        Self::set_pos(recs, &gone, usize::MAX);
        let last = self.v.pop().unwrap();
        if pos < self.v.len() {
            self.v[pos] = last;
            if pos > 0 && last.lt(&self.v[(pos - 1) / 2]) {
                self.up(recs, pos);
            } else {
                self.down(recs, pos);
            }
        }
    }

    /// Sifts the entry at `n` up (a hole moves down instead of swapping at each level).
    fn up(&mut self, recs: &mut Records, mut n: usize) {
        let e = self.v[n];
        while n > 0 {
            let p = (n - 1) / 2;
            let pe = self.v[p];
            if !e.lt(&pe) {
                break;
            }
            self.v[n] = pe;
            Self::set_pos(recs, &pe, n);
            n = p;
        }
        self.v[n] = e;
        Self::set_pos(recs, &e, n);
    }

    fn down(&mut self, recs: &mut Records, mut n: usize) {
        let len = self.v.len();
        let e = self.v[n];
        loop {
            let l = 2 * n + 1;
            if l >= len {
                break;
            }
            let r = l + 1;
            let c = if r < len && self.v[r].lt(&self.v[l]) { r } else { l };
            let ce = self.v[c];
            if !ce.lt(&e) {
                break;
            }
            self.v[n] = ce;
            Self::set_pos(recs, &ce, n);
            n = c;
        }
        self.v[n] = e;
        Self::set_pos(recs, &e, n);
    }
}
