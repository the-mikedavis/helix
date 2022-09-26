use std::collections::VecDeque;
use std::mem::replace;

use crate::syntax::Highlight;

use super::HighlightEvent;
use HighlightEvent::*;

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
        // Sort by range: ascending by start and then descending by end for ties.
        if self.start == other.start {
            self.end.cmp(&other.end).reverse()
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

impl SpanIter {
    fn start_span(&mut self, span: Span) {
        debug_assert!(span.start <= span.end);
        self.event_queue
            .push_back(HighlightStart(Highlight(span.scope)));
        self.range_ends.push(span.end);
    }

    fn process_range_end(&mut self, end: usize) -> HighlightEvent {
        let start = replace(&mut self.cursor, end);
        if start != end {
            debug_assert!(start < end);
            self.event_queue.push_back(HighlightEnd);
            Source { start, end }
        } else {
            HighlightEnd
        }
    }

    // Any subslices that end before intersect span needs to be subsliced, consume the
    // left part of the subslice and leave the right.
    fn partition_spans_at(&mut self, intersect: usize) {
        let first_partitioned_span = self.spans[self.index];

        let mut i = self.index;
        while let Some(span) = self.spans.get_mut(i) {
            if span.start != self.cursor || span.end < intersect {
                break;
            }

            let mut partitioned_span = *span;
            partitioned_span.end = intersect;
            span.start = intersect;

            self.start_span(partitioned_span);
            i += 1;
        }

        let subslices = i - self.index;

        // When spans are subsliced, the span Vec may need to be re-sorted
        // because the `range.start` may now be greater than some `range.start`
        // later in the Vec. This is not a classic "sort": we take several
        // shortcuts to improve the runtime so that the sort may be done in
        // time linear to the cardinality of the span Vec. Practically speaking
        // the runtime is even better since we only scan from `self.index` to
        // the first element of the Vec with a `range.start` after this range.
        let mut after = None;
        let intersect_span = Span {
            start: intersect,
            ..first_partitioned_span
        };
        while let Some(span) = self.spans.get(i) {
            if span <= &intersect_span {
                after = Some(i);
                i += 1;
            } else {
                break;
            }
        }

        // Rotate the subsliced spans so that they come after the spans that
        // have smaller `range.start`s.
        if let Some(after) = after {
            self.spans[self.index..=after].rotate_left(subslices);
        }
    }
}

impl Iterator for SpanIter {
    type Item = HighlightEvent;

    fn next(&mut self) -> Option<Self::Item> {
        // Emit any queued highlight events
        if let Some(event) = self.event_queue.pop_front() {
            return Some(event);
        }

        if self.index == self.spans.len() {
            // There are no more spans. Emit Sources and HighlightEnds for
            // any ranges which have not been terminated yet.
            return self.range_ends.pop().map(|end| self.process_range_end(end));
        }

        let span = self.spans[self.index];

        // Finish processing in-progress ranges that end before the new span starts.
        // These can simply be popped off the end of `range_ends`
        // because it is sorted in descending order
        let subslice = if let Some(&end) = self.range_ends.last() {
            if span.start >= end {
                // The new range is past the end of this in-progress range.
                // Complete the in-progress range by emitting a Source,
                // if necessary, and a HighlightEnd and advance the cursor.
                self.range_ends.pop();
                return Some(self.process_range_end(end));
            } else {
                // If the new range is longer than some in-progress range,
                // we need to subslice this range and any ranges with the
                // same start. `subslice` is set to the smallest `end` for
                // which `range.start < end < range.end`.
                if span.end > end {
                    Some(end)
                } else {
                    None
                }
            }
        } else {
            None
        };

        // Emit a Source event between consecutive HighlightStart events
        let start = replace(&mut self.cursor, span.start);
        if span.start != start && !self.range_ends.is_empty() {
            debug_assert!(start < span.start);
            return Some(Source {
                start,
                end: span.start,
            });
        }

        if let Some(intersect) = subslice {
            self.partition_spans_at(intersect)
        }

        // start any new spans at the current position
        while let Some(&span) = self.spans.get(self.index) {
            if span.start != self.cursor {
                break;
            }
            self.start_span(span);
            self.index += 1;
        }

        // Ensure range ends are sorted in descending orders.
        // Ranges are sorted by their start instead of their end so the input sorting can't be reused.
        // The range ends must be sorted in descending order.
        // So that the ranges that end before the next range can be easily removed.
        self.range_ends
            .sort_unstable_by(|lhs, rhs| lhs.cmp(rhs).reverse());

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
