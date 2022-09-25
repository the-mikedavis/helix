use super::*;
use similar_asserts::assert_eq;

macro_rules! span {
    ($scope:literal, $range:expr) => {
        Span {
            scope: $scope,
            start: $range.start,
            end: $range.end,
        }
    };
}

#[test]
fn test_non_overlapping_span_iter_events() {
    use HighlightEvent::*;
    let input = vec![span!(1, 0..5), span!(2, 6..10)];
    let output: Vec<_> = span_iter(input).collect();
    assert_eq!(
        output,
        &[
            HighlightStart(Highlight(1)),
            Source { start: 0, end: 5 },
            HighlightEnd, // ends 1
            HighlightStart(Highlight(2)),
            Source { start: 6, end: 10 },
            HighlightEnd, // ends 2
        ],
    );
}

#[test]
fn test_simple_overlapping_span_iter_events() {
    use HighlightEvent::*;

    let input = vec![span!(1, 0..10), span!(2, 3..6)];
    let output: Vec<_> = span_iter(input).collect();
    assert_eq!(
        output,
        &[
            HighlightStart(Highlight(1)),
            Source { start: 0, end: 3 },
            HighlightStart(Highlight(2)),
            Source { start: 3, end: 6 },
            HighlightEnd, // ends 2
            Source { start: 6, end: 10 },
            HighlightEnd, // ends 1
        ],
    );
}

#[test]
fn test_many_overlapping_span_iter_events() {
    use HighlightEvent::*;

    /*
    Input:

                                                                5
                                                            |-------|
                                                               4
                                                         |----------|
                                              3
                                |---------------------------|
                    2
            |---------------|
                            1
        |---------------------------------------|

        |---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
        0   1   2   3   4   5   6   7   8   9  10  11  12  13  14  15
    */
    let input = vec![
        span!(1, 0..10),
        span!(2, 1..5),
        span!(3, 6..13),
        span!(4, 12..15),
        span!(5, 13..15),
    ];

    /*
    Output:

                    2                  3                  4     5
            |---------------|   |---------------|       |---|-------|

                            1                         3         4
        |---------------------------------------|-----------|-------|

        |---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
        0   1   2   3   4   5   6   7   8   9  10  11  12  13  14  15
    */
    let output: Vec<_> = span_iter(input).collect();

    assert_eq!(
        output,
        &[
            HighlightStart(Highlight(1)),
            Source { start: 0, end: 1 },
            HighlightStart(Highlight(2)),
            Source { start: 1, end: 5 },
            HighlightEnd, // ends 2
            Source { start: 5, end: 6 },
            HighlightStart(Highlight(3)),
            Source { start: 6, end: 10 },
            HighlightEnd, // ends 3
            HighlightEnd, // ends 1
            HighlightStart(Highlight(3)),
            Source { start: 10, end: 12 },
            HighlightStart(Highlight(4)),
            Source { start: 12, end: 13 },
            HighlightEnd, // ends 4
            HighlightEnd, // ends 3
            HighlightStart(Highlight(5)),
            HighlightStart(Highlight(4)),
            Source { start: 13, end: 15 },
            HighlightEnd, // ends 5
            HighlightEnd, // ends 4
        ],
    );
}

#[test]
fn test_multiple_duplicate_overlapping_span_iter_events() {
    use HighlightEvent::*;
    // This is based an a realistic case from rust-analyzer
    // diagnostics. Spans may both overlap and duplicate one
    // another at varying diagnostic levels.

    /*
    Input:

                                  4,5
                        |-----------------------|
                                3
                        |---------------|
                    1,2
        |-----------------------|

        |---|---|---|---|---|---|---|---|---|---|
        0   1   2   3   4   5   6   7   8   9  10
    */

    let input = vec![
        span!(1, 0..6),
        span!(2, 0..6),
        span!(3, 4..10),
        span!(4, 4..10),
        span!(5, 4..8),
    ];

    /*
    Output:

               1,2         1..5    3..5    4,5
        |---------------|-------|-------|-------|

        |---|---|---|---|---|---|---|---|---|---|
        0   1   2   3   4   5   6   7   8   9  10
    */
    let output: Vec<_> = span_iter(input).collect();
    assert_eq!(
        output,
        &[
            HighlightStart(Highlight(1)),
            HighlightStart(Highlight(2)),
            Source { start: 0, end: 4 },
            HighlightStart(Highlight(3)),
            HighlightStart(Highlight(4)),
            HighlightStart(Highlight(5)),
            Source { start: 4, end: 6 },
            HighlightEnd, // ends 5
            HighlightEnd, // ends 4
            HighlightEnd, // ends 3
            HighlightEnd, // ends 2
            HighlightEnd, // ends 1
            HighlightStart(Highlight(3)),
            HighlightStart(Highlight(4)),
            HighlightStart(Highlight(5)),
            Source { start: 6, end: 8 },
            HighlightEnd, // ends 5
            Source { start: 8, end: 10 },
            HighlightEnd, // ends 4
            HighlightEnd, // ends 3
        ],
    );
}

#[test]
fn test_span_iter_events_where_ranges_must_be_sorted() {
    use HighlightEvent::*;
    // This case needs the span Vec to be re-sorted because
    // span 3 is subsliced to 9..10, putting it after span 4 and 5
    // in the ordering.

    /*
    Input:

                                      4   5
                                    |---|---|
                    2                   3
            |---------------|   |---------------|
                          1
        |-----------------------------------|

        |---|---|---|---|---|---|---|---|---|---|
        0   1   2   3   4   5   6   7   8   9  10
    */
    let input = vec![
        span!(1, 0..9),
        span!(2, 1..5),
        span!(3, 6..10),
        span!(4, 7..8),
        span!(5, 8..9),
    ];

    /*
    Output:

                                      4   5
                                    |---|---|
                    2                   3
            |---------------|   |-----------|
                          1                   3
        |-----------------------------------|---|

        |---|---|---|---|---|---|---|---|---|---|
        0   1   2   3   4   5   6   7   8   9  10
    */
    let output: Vec<_> = span_iter(input).collect();
    assert_eq!(
        output,
        &[
            HighlightStart(Highlight(1)),
            Source { start: 0, end: 1 },
            HighlightStart(Highlight(2)),
            Source { start: 1, end: 5 },
            HighlightEnd, // ends 2
            Source { start: 5, end: 6 },
            HighlightStart(Highlight(3)),
            Source { start: 6, end: 7 },
            HighlightStart(Highlight(4)),
            Source { start: 7, end: 8 },
            HighlightEnd, // ends 4
            HighlightStart(Highlight(5)),
            Source { start: 8, end: 9 },
            HighlightEnd, // ends 5
            HighlightEnd, // ends 3
            HighlightEnd, // ends 1
            HighlightStart(Highlight(3)),
            Source { start: 9, end: 10 },
            HighlightEnd, // ends 3
        ],
    );
}

#[test]
fn empty_span_at_sublice_start() {
    use HighlightEvent::*;
    /*
    Input:
                5

                |
                   3
            |-----------|
            2      4
        |-------|----|
              1
        |-----------|

        |---|---|---|---|
        0   1   2   3   4         */
    let input = vec![
        span!(1, 0..3),
        span!(2, 0..2),
        span!(3, 1..4),
        span!(4, 2..3),
        // This last empty span is what causes the edgecase, do not remove or this test is useless
        span!(5, 2..2),
    ];

    /*
    Output:
              3    4
            |---|---|
            2     3
        |-------|---|
              1       3
        |-----------|---|

        |---|---|---|---|
        0   1   2   3   4         */
    let output: Vec<_> = span_iter(input).collect();
    assert_eq!(
        output,
        &[
            HighlightStart(Highlight(1)),
            HighlightStart(Highlight(2)),
            Source { start: 0, end: 1 },
            HighlightStart(Highlight(3)),
            Source { start: 1, end: 2 },
            HighlightEnd, // ends 3
            HighlightEnd, // ends 2
            HighlightStart(Highlight(3)),
            HighlightStart(Highlight(4)),
            HighlightStart(Highlight(5)),
            HighlightEnd, // ends 5
            Source { start: 2, end: 3 },
            HighlightEnd, // ends 4
            HighlightEnd, // ends 3
            HighlightEnd, // ends 1
            HighlightStart(Highlight(3)),
            HighlightStart(Highlight(4)),
            HighlightEnd, // ends 4
            Source { start: 3, end: 4 },
            HighlightEnd, // ends 4
        ],
    );
}
