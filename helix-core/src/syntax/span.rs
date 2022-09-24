use std::collections::VecDeque;

use crate::syntax::Highlight;

use super::HighlightEvent;

#[cfg(test)]
mod test;

/// A range highlighted with a given scope.
///
/// Spans are a simplifer data structure for describing a highlight range
/// than [super::HighlightEvent]s.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Span {
    pub scope: usize,
    pub start: usize,
    pub end: usize,
}

impl Ord for Span {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // Sort by range: ascending by start and then ascending by end for ties.
        if self.start == other.start {
            self.end.cmp(&other.end)
        } else {
            self.start.cmp(&other.start)
        }
    }
}

impl PartialOrd for Span {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

struct SpanIter {
    spans: Vec<Span>,
    index: usize,
    event_queue: VecDeque<HighlightEvent>,
    range_ends: Vec<usize>,
    cursor: usize,
}

/// Creates an iterator of [HighlightEvent]s from a [Vec] of [Span]s.
///
/// Spans may overlap. In the produced [HighlightEvent] iterator, all
/// `HighlightEvent::Source` events will be sorted by `start` and will not
/// overlap. The iterator produced by this function satisfies all invariants
/// and assumptions for [super::merge]
///
/// `spans` is assumed to be sorted by `range.start` ascending and then by
/// `range.end` descending for any ties.
///
/// # Panics
///
/// Panics on debug builds when the input spans overlap or are not sorted.
pub fn span_iter(spans: Vec<Span>) -> impl Iterator<Item = HighlightEvent> {
    // Assert that `spans` is sorted by `range.start` ascending and
    // `range.end` descending.
    debug_assert!(spans.windows(2).all(|window| window[0] <= window[1]));

    SpanIter {
        spans,
        index: 0,
        event_queue: VecDeque::new(),
        range_ends: Vec::new(),
        cursor: 0,
    }
}

impl Iterator for SpanIter {
    type Item = HighlightEvent;

    fn next(&mut self) -> Option<Self::Item> {
        use HighlightEvent::*;

        // Emit any queued highlight events
        if let Some(event) = self.event_queue.pop_front() {
            return Some(event);
        }

        if self.index == self.spans.len() {
            // There are no more spans. Emit Sources and HighlightEnds for
            // any ranges which have not been terminated yet.
            for end in self.range_ends.drain(..) {
                if self.cursor != end {
                    debug_assert!(self.cursor < end);
                    self.event_queue.push_back(Source {
                        start: self.cursor,
                        end,
                    });
                }
                self.event_queue.push_back(HighlightEnd);
                self.cursor = end;
            }
            return self.event_queue.pop_front();
        }

        let span = self.spans[self.index];
        let mut subslice = None;

        self.range_ends.retain(|end| {
            if span.start >= *end {
                // The new range is past the end of this in-progress range.
                // Complete the in-progress range by emitting a Source,
                // if necessary, and a HighlightEnd and advance the cursor.
                if self.cursor != *end {
                    debug_assert!(self.cursor < *end);
                    self.event_queue.push_back(Source {
                        start: self.cursor,
                        end: *end,
                    });
                }
                self.event_queue.push_back(HighlightEnd);
                self.cursor = *end;
                false
            } else if span.end > *end && subslice.is_none() {
                // If the new range is longer than some in-progress range,
                // we need to subslice this range and any ranges with the
                // same start. `subslice` is set to the smallest `end` for
                // which `range.start < end < range.end`.
                subslice = Some(*end);
                true
            } else {
                true
            }
        });

        // Emit a Source event between consecutive HighlightStart events
        if span.start != self.cursor && !self.range_ends.is_empty() {
            debug_assert!(self.cursor < span.start);
            self.event_queue.push_back(Source {
                start: self.cursor,
                end: span.start,
            });
        }

        self.cursor = span.start;

        // Handle all spans that share this starting point. Either subslice
        // or fully consume the span.
        let mut i = self.index;
        let mut subslices = 0;
        loop {
            match self.spans.get_mut(i) {
                Some(span) if span.start == self.cursor => {
                    self.event_queue
                        .push_back(HighlightStart(Highlight(span.scope)));
                    i += 1;

                    match subslice {
                        Some(intersect) => {
                            // If this span needs to be subsliced, consume the
                            // left part of the subslice and leave the right.
                            self.range_ends.push(intersect);
                            span.start = intersect;
                            subslices += 1;
                        }
                        None => {
                            // If there is no subslice, consume the span.
                            self.range_ends.push(span.end);
                            self.index = i;
                        }
                    }
                }
                _ => break,
            }
        }

        // Ensure range-ends are sorted ascending. Ranges which start at the
        // same point may be in descending order because of the assumed
        // sort-order of input ranges.
        self.range_ends.sort_unstable();

        // When spans are subsliced, the span Vec may need to be re-sorted
        // because the `range.start` may now be greater than some `range.start`
        // later in the Vec. This is not a classic "sort": we take several
        // shortcuts to improve the runtime so that the sort may be done in
        // time linear to the cardinality of the span Vec. Practically speaking
        // the runtime is even better since we only scan from `self.index` to
        // the first element of the Vec with a `range.start` after this range.
        if let Some(intersect) = subslice {
            let mut after = None;

            // Find the index of the largest span smaller than the intersect point.
            // `i` starts on the index after the last subsliced span.
            loop {
                match self.spans.get(i) {
                    Some(span) if span.start < intersect => {
                        after = Some(i);
                        i += 1;
                    }
                    _ => break,
                }
            }

            // Rotate the subsliced spans so that they come after the spans that
            // have smaller `range.start`s.
            if let Some(after) = after {
                self.spans[self.index..=after].rotate_left(subslices);
            }
        }

        self.event_queue.pop_front()
    }
}

struct FlatSpanIter<I> {
    iter: I,
}

/// Converts a Vec of spans into an [Iterator] over [HighlightEvent]s
///
/// This implementation does not resolve overlapping spans. Zero-width spans are
/// eliminated but otherwise the ranges are trusted to not overlap.
///
/// This iterator has much less overhead than [span_iter] and is appropriate for
/// cases where the input spans are known to satisfy all of [super::merge]'s
/// assumptions and invariants, such as with selection highlights.
///
/// # Panics
///
/// Panics on debug builds when the input spans overlap or are not sorted.
pub fn flat_span_iter(spans: Vec<Span>) -> impl Iterator<Item = HighlightEvent> {
    use HighlightEvent::*;

    // Consecutive items are sorted and non-overlapping
    debug_assert!(spans
        .windows(2)
        .all(|window| window[1].start >= window[0].end));

    FlatSpanIter {
        iter: spans
            .into_iter()
            .filter(|span| span.start != span.end)
            .flat_map(|span| {
                [
                    HighlightStart(Highlight(span.scope)),
                    Source {
                        start: span.start,
                        end: span.end,
                    },
                    HighlightEnd,
                ]
            }),
    }
}

impl<I: Iterator<Item = HighlightEvent>> Iterator for FlatSpanIter<I> {
    type Item = HighlightEvent;
    fn next(&mut self) -> Option<Self::Item> {
        self.iter.next()
    }
}
