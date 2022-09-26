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
        // Sort by span: ascending by start and then descending by end for ties.
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
    span_ends: Vec<usize>,
    cursor: usize,
}

/// Creates an iterator of [HighlightEvent]s from a [Vec] of [Span]s.
///
/// Spans may overlap. In the produced [HighlightEvent] iterator, all
/// `HighlightEvent::Source` events will be sorted by `start` and will not
/// overlap. The iterator produced by this function satisfies all invariants
/// and assumptions for [super::merge]
///
/// `spans` is assumed to be sorted by `span.start` ascending and then by
/// `span.end` descending for any ties.
///
/// # Panics
///
/// Panics on debug builds when the input spans overlap or are not sorted.
pub fn span_iter(spans: Vec<Span>) -> impl Iterator<Item = HighlightEvent> {
    // Assert that `spans` is sorted by `span.start` ascending and
    // `span.end` descending.
    debug_assert!(spans.windows(2).all(|window| window[0] <= window[1]));

    SpanIter {
        spans,
        index: 0,
        event_queue: VecDeque::new(),
        span_ends: Vec::new(),
        cursor: 0,
    }
}

impl SpanIter {
    fn start_span(&mut self, span: Span) {
        debug_assert!(span.start <= span.end);
        self.event_queue
            .push_back(HighlightStart(Highlight(span.scope)));
        self.span_ends.push(span.end);
    }

    fn emit_span_end(&mut self, end: usize) -> HighlightEvent {
        let start = replace(&mut self.cursor, end);
        if start != end {
            debug_assert!(start < end);
            self.event_queue.push_back(HighlightEnd);
            Source { start, end }
        } else {
            HighlightEnd
        }
    }

    /// This function is called if any in-progress span (henceforth called span A)
    /// ends before the end of the span we are looking at (henceforth called span B).
    /// It partitions span B at the end of span `A` so that `A` can be removed from the highlight stack
    /// before the remainder of `B`  is added to the stack.
    ///
    /// There might be multiple spans starting at the same point as `B`.
    /// All spans that also end past `A` are processed the same in this function.
    /// Spans that end before `A` are not handled here and should be processed as usual.
    ///
    /// This function calls `start_span` for the subs pans that end at the same point as span `A`.
    /// The remaining subspaces are stored in `self.spans` at the correct position.
    ///
    /// # Arguments
    ///
    /// * intersect: the end of span `A`   
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

        let num_partitioned_spans = i - self.index;

        // When spans are partioned, the span Vec may need to be re-sorted
        // because the `span.start` may now be greater than some `span.start`
        // later in the Vec. This is not a classic "sort": we take several
        // shortcuts to improve the runtime so that the sort may be done in
        // time linear to the cardinality of the span Vec. Practically speaking
        // the runtime is even better since we only scan from `self.index` to
        // the first element of the Vec with a `span.start` after this span.
        let intersect_span = Span {
            start: intersect,
            ..first_partitioned_span
        };

        let num_spans_to_resort = self.spans[i..]
            .iter()
            .take_while(|&&span| span <= intersect_span)
            .count();

        // Rotate the subsliced spans so that they come after the spans that
        // have smaller `span.start`s.
        if num_spans_to_resort != 0 {
            let first_sorted_span = i + num_spans_to_resort;
            self.spans[self.index..first_sorted_span].rotate_left(num_partitioned_spans);
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
            // any spans which have not been terminated yet.
            return self.span_ends.pop().map(|end| self.emit_span_end(end));
        }

        let span = self.spans[self.index];

        // Finish processing in-progress spans that end before the new span starts.
        // These can simply be popped off the end of `span_ends`
        // because it is sorted in descending order
        let subslice = if let Some(&end) = self.span_ends.last() {
            if span.start >= end {
                // The new span is past the end of this in-progress span.
                // Complete the in-progress span by emitting a Source,
                // if necessary, and a HighlightEnd and advance the cursor.
                self.span_ends.pop();
                return Some(self.emit_span_end(end));
            } else {
                // If the new span is longer than some in-progress span,
                // we need to subslice this span and any spans with the
                // same start. `subslice` is set to the smallest `end` for
                // which `span.start < end < span.end`.
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
        if span.start != start && !self.span_ends.is_empty() {
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

        // Ensure span ends are sorted in descending orders.
        // Spans are sorted by their start instead of their end so the input sorting can't be reused.
        // The span ends must be sorted in descending order.
        // So that the spans that end before the next span can be easily removed.
        self.span_ends
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
/// eliminated but otherwise the spans are trusted to not overlap.
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
