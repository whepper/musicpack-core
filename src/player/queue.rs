//! Framework-free queue model.
//!
//! Port of `web/player-core/src/queue.ts` (BSD-3-Clause). Canonical items +
//! cursor, with repeat modes and shuffle layered over the canonical list
//! (never destroying it):
//!
//! - repeat `one` is resolved by the player at EOS (reload current), so
//!   `next` treats it like `off`;
//! - repeat `all` wraps `next` to index 0;
//! - shuffle produces a presentation order via [`super::order`] (seeded RNG);
//!   [`QueueModel::previous`] consults a history stack so Previous works
//!   under shuffle; toggling shuffle off restores canonical order.

use super::order::{Rng, next_index_under_repeat, shuffle_order};
use super::types::{PlaybackItem, RepeatMode};

/// Observable queue state.
#[derive(Debug, Clone, PartialEq)]
pub struct QueueState {
    /// Canonical item list.
    pub items: Vec<PlaybackItem>,
    /// Cursor index (`-1` when empty).
    pub index: i64,
}

/// Framework-free queue model.
pub struct QueueModel {
    items: Vec<PlaybackItem>,
    index: i64,
    order: Vec<usize>,
    history: Vec<i64>,
    rng: Box<Rng>,
    repeat: RepeatMode,
    shuffling: bool,
}

impl QueueModel {
    /// Creates an empty queue with the given shuffle RNG.
    pub fn new(rng: Box<Rng>) -> Self {
        Self {
            items: Vec::new(),
            index: -1,
            order: Vec::new(),
            history: Vec::new(),
            rng,
            repeat: RepeatMode::Off,
            shuffling: false,
        }
    }

    /// Snapshot of the observable state.
    pub fn state(&self) -> QueueState {
        QueueState {
            items: self.items.clone(),
            index: self.index,
        }
    }

    /// The canonical item list.
    pub fn items(&self) -> &[PlaybackItem] {
        &self.items
    }

    /// The cursor index (`-1` when empty).
    pub fn index(&self) -> i64 {
        self.index
    }

    /// The active repeat mode.
    pub fn repeat(&self) -> RepeatMode {
        self.repeat
    }

    /// Whether shuffle is on.
    pub fn shuffling(&self) -> bool {
        self.shuffling
    }

    /// Item at `i`, or `None`.
    pub fn at(&self, i: i64) -> Option<&PlaybackItem> {
        if i < 0 {
            return None;
        }
        self.items.get(i as usize)
    }

    /// The current item, or `None`.
    pub fn current(&self) -> Option<&PlaybackItem> {
        self.at(self.index)
    }

    /// Replaces the canonical list and cursor (session restore).
    pub fn set_all(&mut self, items: Vec<PlaybackItem>, index: i64) {
        self.items = items;
        self.index = index;
    }

    /// Sets the repeat mode.
    pub fn set_repeat(&mut self, mode: RepeatMode) {
        self.repeat = mode;
    }

    /// Toggles shuffle; rebuilds the current-first presentation order.
    pub fn set_shuffle(&mut self, on: bool) {
        if self.shuffling == on {
            return;
        }
        self.shuffling = on;
        self.history.clear();
        self.rebuild_order();
    }

    /// Test/inspection seam: the active presentation order (indices).
    pub fn presentation_order(&self) -> Vec<usize> {
        if self.shuffling {
            self.order.clone()
        } else {
            Vec::new()
        }
    }

    /// Test-only seam: install a presentation order.
    pub fn set_presentation_order_for_test(&mut self, next: Vec<usize>) {
        if self.shuffling {
            self.order = next;
        }
    }

    /// Plays a sequence starting at `start_index` (default 0).
    pub fn play_sequence(
        &mut self,
        items: Vec<PlaybackItem>,
        start_index: i64,
    ) -> Result<PlaybackItem, String> {
        if items.is_empty() {
            return Err("This release has no playable tracks.".to_string());
        }
        let index = start_index.clamp(0, items.len() as i64 - 1);
        self.items = items;
        self.index = index;
        self.history.clear();
        self.rebuild_order();
        self.items
            .get(index as usize)
            .cloned()
            .ok_or_else(|| "This release has no playable tracks.".to_string())
    }

    /// Replaces the queue with a single item.
    pub fn play_now(&mut self, item: PlaybackItem) -> PlaybackItem {
        self.items = vec![item.clone()];
        self.index = 0;
        self.history.clear();
        self.rebuild_order();
        item
    }

    /// Inserts right after the current item.
    pub fn play_next(&mut self, item: PlaybackItem) {
        if self.items.is_empty() {
            self.items = vec![item];
            self.index = 0;
        } else {
            let at = (self.index + 1).max(0) as usize;
            let at = at.min(self.items.len());
            self.items.insert(at, item);
        }
        self.rebuild_order();
    }

    /// Appends to the end of the queue.
    pub fn enqueue(&mut self, item: PlaybackItem) {
        self.items.push(item);
        self.rebuild_order();
    }

    /// Appends several items to the end of the queue.
    pub fn enqueue_many(&mut self, items: Vec<PlaybackItem>) {
        self.items.extend(items);
        self.rebuild_order();
    }

    /// Advances under the active policy. Records history.
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> Option<PlaybackItem> {
        let target = self.peek_next_index();
        let target = target?;
        self.index = target as i64;
        self.items.get(target).cloned()
    }

    /// History-aware back navigation.
    pub fn previous(&mut self) -> Option<PlaybackItem> {
        let target = self.peek_prev_index()?;
        self.index = target as i64;
        self.items.get(target).cloned()
    }

    /// Moves the cursor to an item (no queue rebuild).
    pub fn move_to(&mut self, i: i64) {
        if i < 0 || i >= self.items.len() as i64 {
            return;
        }
        self.index = i;
    }

    /// Reorders the canonical list. The cursor follows the current ITEM and
    /// history indices are remapped so Previous still retraces navigation.
    pub fn move_item(&mut self, from: i64, to: i64) {
        let n = self.items.len() as i64;
        if from < 0 || from >= n || to < 0 || to >= n || from == to {
            return;
        }
        let moved = self.items.remove(from as usize);
        self.items.insert(to as usize, moved);
        let remap = |i: i64| -> i64 {
            if i == from {
                return to;
            }
            if from < to && i > from && i <= to {
                return i - 1;
            }
            if from > to && i >= to && i < from {
                return i + 1;
            }
            i
        };
        self.history = self.history.iter().map(|&h| remap(h)).collect();
        self.index = remap(self.index);
        self.rebuild_order();
    }

    /// Removes the item at `i`, adjusting the cursor and history.
    pub fn remove_at(&mut self, i: i64) {
        if i < 0 || i >= self.items.len() as i64 {
            return;
        }
        let removed_cursor_shift = if i < self.index { 1 } else { 0 };
        let was_current = i == self.index;
        self.items.remove(i as usize);
        let mut index = self.index - removed_cursor_shift;
        if was_current {
            index = index.min(self.items.len() as i64 - 1);
        }
        self.history.retain(|&h| h != i);
        self.history = self
            .history
            .iter()
            .map(|&h| if h > i { h - 1 } else { h })
            .collect();
        self.index = index;
        self.rebuild_order();
    }

    /// Empties the queue.
    pub fn clear(&mut self) {
        self.items.clear();
        self.index = -1;
        self.history.clear();
        self.order.clear();
    }

    // -- internal helpers --------------------------------------------------

    fn rebuild_order(&mut self) {
        if !self.shuffling || self.items.is_empty() {
            self.order.clear();
            return;
        }
        let mut upcoming: Vec<usize> = Vec::new();
        for i in 0..self.items.len() {
            if i as i64 != self.index {
                upcoming.push(i);
            }
        }
        let rest = shuffle_order(&upcoming, &mut self.rng);
        self.order = if self.index >= 0 {
            let mut order = Vec::with_capacity(rest.len() + 1);
            order.push(self.index as usize);
            order.extend(rest);
            order
        } else {
            rest
        };
    }

    fn reshuffle(&mut self) {
        self.rebuild_order();
    }

    fn push_history(&mut self, from: i64) {
        if from >= 0 {
            self.history.push(from);
        }
        if self.history.len() > 500 {
            let excess = self.history.len() - 500;
            self.history.drain(0..excess);
        }
    }

    fn peek_next_index(&mut self) -> Option<usize> {
        if self.items.is_empty() {
            return None;
        }
        let cursor = self.index;
        self.push_history(cursor);
        if !self.shuffling {
            return next_index_under_repeat(cursor, self.items.len(), self.repeat);
        }
        let pos = self.order.iter().position(|&x| x as i64 == cursor);
        if let Some(pos) = pos {
            if pos + 1 < self.order.len() {
                return Some(self.order[pos + 1]);
            }
        }
        if self.repeat == RepeatMode::All {
            self.reshuffle();
            return self.order.first().copied();
        }
        None
    }

    fn peek_prev_index(&mut self) -> Option<usize> {
        if self.items.is_empty() {
            return None;
        }
        while let Some(prev) = self.history.pop() {
            if prev >= 0 && prev < self.items.len() as i64 && prev != self.index {
                return Some(prev as usize);
            }
        }
        if !self.shuffling {
            let pi = self.index - 1;
            return if pi >= 0 { Some(pi as usize) } else { None };
        }
        let pos = self.order.iter().position(|&x| x as i64 == self.index);
        pos.and_then(|p| if p > 0 { Some(self.order[p - 1]) } else { None })
    }
}
