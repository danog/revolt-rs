//! The timer queue: a binary min-heap of (expiration, insertion sequence) with each record's
//! position kept in the record (O(log n) removal, as Revolt's TimerQueue). Equal expirations
//! fire in insertion order.

use super::{IdMap, Record};

#[derive(Clone, Copy)]
struct Entry {
    exp: f64,
    seq: u64,
    id: u64,
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

type Recs = IdMap<Box<Record>>;

impl Heap {
    #[inline]
    fn set_pos(recs: &mut Recs, id: u64, pos: usize) {
        if let Some(r) = recs.get_mut(&id) {
            r.heap_pos = pos;
        }
    }

    pub fn len(&self) -> usize {
        self.v.len()
    }

    pub fn insert(&mut self, recs: &mut Recs, id: u64) {
        let Some(r) = recs.get_mut(&id) else { return };
        debug_assert!(r.heap_pos == usize::MAX);
        self.seq += 1;
        let e = Entry { exp: r.expiration, seq: self.seq, id };
        self.v.push(e);
        let n = self.v.len() - 1;
        r.heap_pos = n;
        self.up(recs, n);
    }

    pub fn peek(&self) -> Option<f64> {
        self.v.first().map(|e| e.exp)
    }

    /// Removes and returns the first timer if it expired at `now`.
    pub fn pop_expired(&mut self, recs: &mut Recs, now: f64) -> Option<u64> {
        let e = *self.v.first()?;
        if e.exp > now {
            return None;
        }
        self.remove(recs, 0);
        Some(e.id)
    }

    pub fn remove(&mut self, recs: &mut Recs, pos: usize) {
        if pos >= self.v.len() {
            return;
        }
        let id = self.v[pos].id;
        Self::set_pos(recs, id, usize::MAX);
        let last = self.v.pop().unwrap();
        if pos < self.v.len() {
            self.v[pos] = last;
            Self::set_pos(recs, last.id, pos);
            if pos > 0 && self.v[pos].lt(&self.v[(pos - 1) / 2]) {
                self.up(recs, pos);
            } else {
                self.down(recs, pos);
            }
        }
    }

    fn up(&mut self, recs: &mut Recs, mut n: usize) {
        while n > 0 {
            let p = (n - 1) / 2;
            if !self.v[n].lt(&self.v[p]) {
                break;
            }
            self.v.swap(n, p);
            Self::set_pos(recs, self.v[n].id, n);
            Self::set_pos(recs, self.v[p].id, p);
            n = p;
        }
    }

    fn down(&mut self, recs: &mut Recs, mut n: usize) {
        let len = self.v.len();
        loop {
            let l = 2 * n + 1;
            if l >= len {
                break;
            }
            let r = l + 1;
            let c = if r < len && self.v[r].lt(&self.v[l]) { r } else { l };
            if !self.v[c].lt(&self.v[n]) {
                break;
            }
            self.v.swap(n, c);
            Self::set_pos(recs, self.v[n].id, n);
            Self::set_pos(recs, self.v[c].id, c);
            n = c;
        }
    }
}
