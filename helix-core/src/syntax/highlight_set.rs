use std::fmt::{self, Debug};
use std::iter::once;
use std::ops::Range;

use crate::syntax::span::Span;
use crate::syntax::{Highlight, HighlightEvent};

/// A datastructure that records all `Highlight`s at every position as a bitset.
/// This allows collection highlights from various sources (`HighlightEvent`s and `Spans`)
/// and comparing them to ensure they match up.
///
/// The bitset has a fixed size of 128 (u128) and therefore any `Highlight` `x`
///  inserted into this must fullfill `0 < x < 128`;
#[derive(Default, PartialEq, Eq, Clone)]
pub struct HighlightSet(Vec<u128>);

impl HighlightSet {
    fn insert_highlights(
        &mut self,
        positions: Range<usize>,
        highlights: impl IntoIterator<Item = Highlight>,
    ) {
        if self.0.len() < positions.end {
            self.0.resize(positions.end, 0u128);
        }

        let highlights = highlights.into_iter().fold(0, |highlight_set, highlight| {
            // we can only represent 128 bits in a u128
            debug_assert!(highlight.0 < 128);
            let highlight_bit = 1u128 << highlight.0 as u8;
            highlight_set | highlight_bit
        });

        for dst in &mut self.0[positions] {
            *dst |= highlights
        }
    }

    fn highlights_in_set(set: u128) -> impl Iterator<Item = Highlight> {
        (0..128).filter_map(move |i| {
            if (set & 1u128 << i) == 0 {
                None
            } else {
                Some(Highlight(i))
            }
        })
    }

    fn trim(&mut self) {
        while self.0.last().map_or(false, |&last_set| last_set == 0) {
            self.0.pop();
        }
    }
}

impl Debug for HighlightSet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        struct SetPrinter(u128);
        impl Debug for SetPrinter {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                let entries = HighlightSet::highlights_in_set(self.0).map(|highlight| highlight.0);
                f.debug_set().entries(entries).finish()
            }
        }
        let sets = self.0.iter().map(|&set| SetPrinter(set)).enumerate();
        f.debug_map().entries(sets).finish()
    }
}

impl FromIterator<HighlightEvent> for HighlightSet {
    fn from_iter<T: IntoIterator<Item = HighlightEvent>>(events: T) -> Self {
        let mut res = HighlightSet::default();
        res.extend(events);
        res
    }
}

impl FromIterator<Span> for HighlightSet {
    fn from_iter<T: IntoIterator<Item = Span>>(spans: T) -> Self {
        let mut res = HighlightSet::default();
        res.extend(spans);
        res
    }
}

impl Extend<Span> for HighlightSet {
    fn extend<T: IntoIterator<Item = Span>>(&mut self, spans: T) {
        for span in spans {
            self.insert_highlights(span.start..span.end, once(Highlight(span.scope)))
        }
        self.trim()
    }
}

impl Extend<HighlightEvent> for HighlightSet {
    fn extend<T: IntoIterator<Item = HighlightEvent>>(&mut self, events: T) {
        let mut state = Vec::new();
        for event in events {
            match event {
                HighlightEvent::HighlightStart(highlight) => state.push(highlight),
                HighlightEvent::HighlightEnd => {
                    state.pop();
                }
                HighlightEvent::Source { start, end } => {
                    self.insert_highlights(start..end, state.iter().copied())
                }
            }
        }
        self.trim()
    }
}
