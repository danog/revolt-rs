//! The timer queue: a 4-ary min-heap of (expiration, insertion sequence); equal expirations fire
//! in insertion order. Each record's position is kept in a dense array indexed by its slot (O(log
//! n) removal, as Revolt's TimerQueue, without touching the records while entries move).

use super::Cid;

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

const NONE: u32 = u32::MAX;
const ARITY: usize = 4;

#[derive(Default)]
pub struct Heap {
    v: Vec<Entry>,
    /// slot => position in `v` (NONE: not in the heap)
    pos: Vec<u32>,
    seq: u64,
}

impl Heap {
    #[inline(always)]
    fn place(&mut self, e: Entry, at: usize) {
        self.v[at] = e;
        self.pos[e.slot as usize] = at as u32;
    }

    pub fn len(&self) -> usize {
        self.v.len()
    }

    /// Whether the record in `slot` is in the heap.
    #[inline]
    pub fn contains(&self, slot: u32) -> bool {
        self.pos.get(slot as usize).is_some_and(|&p| p != NONE)
    }

    /// Inserts the callback `c` (not in the heap) at expiration `exp`.
    pub fn insert(&mut self, c: Cid, exp: f64) {
        let s = c.slot as usize;
        if self.pos.len() <= s {
            self.pos.resize(s + 1, NONE);
        }
        debug_assert!(self.pos[s] == NONE);
        self.seq += 1;
        let e = Entry { exp, seq: self.seq, id: c.id, slot: c.slot };
        self.v.push(e);
        self.up(e, self.v.len() - 1);
    }

    pub fn peek(&self) -> Option<f64> {
        self.v.first().map(|e| e.exp)
    }

    /// Removes and returns the first timer if it expired at `now`.
    pub fn pop_expired(&mut self, now: f64) -> Option<Cid> {
        let e = *self.v.first()?;
        if e.exp > now {
            return None;
        }
        self.remove_at(0);
        Some(Cid { id: e.id, slot: e.slot })
    }

    /// Removes the record in `slot` if it is in the heap.
    #[inline]
    pub fn remove(&mut self, slot: u32) {
        if let Some(&p) = self.pos.get(slot as usize) {
            if p != NONE {
                self.remove_at(p as usize);
            }
        }
    }

    fn remove_at(&mut self, at: usize) {
        let gone = self.v[at];
        self.pos[gone.slot as usize] = NONE;
        let last = self.v.pop().unwrap();
        if at < self.v.len() {
            if at > 0 && last.lt(&self.v[(at - 1) / ARITY]) {
                self.up(last, at);
            } else {
                self.down(last, at);
            }
        }
    }

    /// Puts `e` at `n` or above (a hole moves down instead of swapping at each level).
    fn up(&mut self, e: Entry, mut n: usize) {
        while n > 0 {
            let p = (n - 1) / ARITY;
            let pe = self.v[p];
            if !e.lt(&pe) {
                break;
            }
            self.place(pe, n);
            n = p;
        }
        self.place(e, n);
    }

    /// Puts `e` at `n` or below.
    fn down(&mut self, e: Entry, mut n: usize) {
        let len = self.v.len();
        loop {
            let first = ARITY * n + 1;
            if first >= len {
                break;
            }
            let mut c = first;
            for k in first + 1..(first + ARITY).min(len) {
                if self.v[k].lt(&self.v[c]) {
                    c = k;
                }
            }
            let ce = self.v[c];
            if !ce.lt(&e) {
                break;
            }
            self.place(ce, n);
            n = c;
        }
        self.place(e, n);
    }
}
