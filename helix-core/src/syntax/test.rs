use super::*;
use crate::syntax::highlight_set::HighlightSet;
use crate::syntax::span::{span_iter, Span};
use crate::{Rope, Transaction};
use proptest::strategy::{Just, Strategy};
use proptest::test_runner::TestCaseResult;
use proptest::{prop_assert, prop_assert_eq, prop_assert_ne, proptest};

#[test]
fn test_textobject_queries() {
    let query_str = r#"
        (line_comment)+ @quantified_nodes
        ((line_comment)+) @quantified_nodes_grouped
        ((line_comment) (line_comment)) @multiple_nodes_grouped
        "#;
    let source = Rope::from_str(
        r#"
/// a comment on
/// multiple lines
        "#,
    );

    let loader = Loader::new(Configuration { language: vec![] });
    let language = get_language("Rust").unwrap();

    let query = Query::new(language, query_str).unwrap();
    let textobject = TextObjectQuery { query };
    let mut cursor = QueryCursor::new();

    let config = HighlightConfiguration::new(language, "", "", "").unwrap();
    let syntax = Syntax::new(&source, Arc::new(config), Arc::new(loader));

    let root = syntax.tree().root_node();
    let mut test = |capture, range| {
        let matches: Vec<_> = textobject
            .capture_nodes(capture, root, source.slice(..), &mut cursor)
            .unwrap()
            .collect();

        assert_eq!(
            matches[0].byte_range(),
            range,
            "@{} expected {:?}",
            capture,
            range
        )
    };

    test("quantified_nodes", 1..36);
    // NOTE: Enable after implementing proper node group capturing
    // test("quantified_nodes_grouped", 1..36);
    // test("multiple_nodes_grouped", 1..36);
}

#[test]
fn test_parser() {
    let highlight_names: Vec<String> = [
        "attribute",
        "constant",
        "function.builtin",
        "function",
        "keyword",
        "operator",
        "property",
        "punctuation",
        "punctuation.bracket",
        "punctuation.delimiter",
        "string",
        "string.special",
        "tag",
        "type",
        "type.builtin",
        "variable",
        "variable.builtin",
        "variable.parameter",
    ]
    .iter()
    .cloned()
    .map(String::from)
    .collect();

    let loader = Loader::new(Configuration { language: vec![] });

    let language = get_language("Rust").unwrap();
    let config = HighlightConfiguration::new(
        language,
        &std::fs::read_to_string("../runtime/grammars/sources/rust/queries/highlights.scm")
            .unwrap(),
        &std::fs::read_to_string("../runtime/grammars/sources/rust/queries/injections.scm")
            .unwrap(),
        "", // locals.scm
    )
    .unwrap();
    config.configure(&highlight_names);

    let source = Rope::from_str(
        "
            struct Stuff {}
            fn main() {}
        ",
    );
    let syntax = Syntax::new(&source, Arc::new(config), Arc::new(loader));
    let tree = syntax.tree();
    let root = tree.root_node();
    assert_eq!(root.kind(), "source_file");

    assert_eq!(
        root.to_sexp(),
        concat!(
            "(source_file ",
            "(struct_item name: (type_identifier) body: (field_declaration_list)) ",
            "(function_item name: (identifier) parameters: (parameters) body: (block)))"
        )
    );

    let struct_node = root.child(0).unwrap();
    assert_eq!(struct_node.kind(), "struct_item");
}

#[test]
fn test_input_edits() {
    use tree_sitter::InputEdit;

    let doc = Rope::from("hello world!\ntest 123");
    let transaction = Transaction::change(
        &doc,
        vec![(6, 11, Some("test".into())), (12, 17, None)].into_iter(),
    );
    let edits = generate_edits(&doc, transaction.changes());
    // transaction.apply(&mut state);

    assert_eq!(
        edits,
        &[
            InputEdit {
                start_byte: 6,
                old_end_byte: 11,
                new_end_byte: 10,
                start_position: Point { row: 0, column: 6 },
                old_end_position: Point { row: 0, column: 11 },
                new_end_position: Point { row: 0, column: 10 }
            },
            InputEdit {
                start_byte: 12,
                old_end_byte: 17,
                new_end_byte: 12,
                start_position: Point { row: 0, column: 12 },
                old_end_position: Point { row: 1, column: 4 },
                new_end_position: Point { row: 0, column: 12 }
            }
        ]
    );

    // Testing with the official example from tree-sitter
    let mut doc = Rope::from("fn test() {}");
    let transaction = Transaction::change(&doc, vec![(8, 8, Some("a: u32".into()))].into_iter());
    let edits = generate_edits(&doc, transaction.changes());
    transaction.apply(&mut doc);

    assert_eq!(doc, "fn test(a: u32) {}");
    assert_eq!(
        edits,
        &[InputEdit {
            start_byte: 8,
            old_end_byte: 8,
            new_end_byte: 14,
            start_position: Point { row: 0, column: 8 },
            old_end_position: Point { row: 0, column: 8 },
            new_end_position: Point { row: 0, column: 14 }
        }]
    );
}

#[test]
fn test_load_runtime_file() {
    // Test to make sure we can load some data from the runtime directory.
    let contents = load_runtime_file("rust", "indents.scm").unwrap();
    assert!(!contents.is_empty());

    let results = load_runtime_file("rust", "does-not-exist");
    assert!(results.is_err());
}

#[test]
fn test_sample_highlight_event_stream_merge() {
    use HighlightEvent::*;

    /*
    Left:
                          2          3
                   |-----------|-----------|

                                1
        |-----------------------------------------------|

        |---|---|---|---|---|---|---|---|---|---|---|---|
        0   1   2   3   4   5   6   7   8   9  10  11  12
    */
    let left = vec![
        HighlightStart(Highlight(1)),
        Source { start: 0, end: 3 },
        HighlightStart(Highlight(2)),
        Source { start: 3, end: 6 },
        HighlightEnd, // ends 2
        HighlightStart(Highlight(3)),
        Source { start: 6, end: 9 },
        HighlightEnd, // ends 3
        Source { start: 9, end: 12 },
        HighlightEnd, // ends 1
    ]
    .into_iter();

    /*
    Right:
                   100            200
                |-------|   |---------------|

        |---|---|---|---|---|---|---|---|---|---|---|---|
        0   1   2   3   4   5   6   7   8   9  10  11  12
    */
    let right = Box::new(
        vec![
            HighlightStart(Highlight(100)),
            Source { start: 2, end: 4 },
            HighlightEnd, // ends 100
            HighlightStart(Highlight(200)),
            Source { start: 5, end: 9 },
            HighlightEnd, // ends 200
        ]
        .into_iter(),
    );

    /*
    Output:
                     100          200
                    |---|   |---------------|

                 100      2          3
                |---|-----------|-----------|

                                1
        |-----------------------------------------------|

        |---|---|---|---|---|---|---|---|---|---|---|---|
        0   1   2   3   4   5   6   7   8   9  10  11  12
    */
    let output: Vec<_> = merge(left, right).collect();

    assert_eq!(
        output,
        &[
            HighlightStart(Highlight(1)),
            Source { start: 0, end: 2 },
            HighlightStart(Highlight(100)),
            Source { start: 2, end: 3 },
            HighlightEnd, // ends 100
            HighlightStart(Highlight(2)),
            HighlightStart(Highlight(100)),
            Source { start: 3, end: 4 },
            HighlightEnd, // ends 100
            Source { start: 4, end: 5 },
            HighlightStart(Highlight(200)),
            Source { start: 5, end: 6 },
            HighlightEnd, // ends 200
            HighlightEnd, // ends 2
            HighlightStart(Highlight(3)),
            HighlightStart(Highlight(200)),
            Source { start: 6, end: 9 },
            HighlightEnd, // ends 200
            HighlightEnd, // ends 3
            Source { start: 9, end: 12 },
            HighlightEnd // ends 1
        ],
    );
}

#[test]
fn test_highlight_event_stream_merge_overlapping() {
    use HighlightEvent::*;

    /*
    Left:
               1, 2, 3
        |-------------------|

        |---|---|---|---|---|
        0   1   2   3   4   5
    */
    let left = vec![
        HighlightStart(Highlight(1)),
        HighlightStart(Highlight(2)),
        HighlightStart(Highlight(3)),
        Source { start: 0, end: 5 },
        HighlightEnd, // ends 3
        HighlightEnd, // ends 2
        HighlightEnd, // ends 1
    ]
    .into_iter();

    /*
    Right:
                  4
        |-------------------|

        |---|---|---|---|---|
        0   1   2   3   4   5
    */
    let right = Box::new(
        vec![
            HighlightStart(Highlight(4)),
            Source { start: 0, end: 5 },
            HighlightEnd, // ends 4
        ]
        .into_iter(),
    );

    /*
    Output:
              1, 2, 3, 4
        |-------------------|

        |---|---|---|---|---|
        0   1   2   3   4   5
    */
    let output: Vec<_> = merge(left, right).collect();

    assert_eq!(
        output,
        &[
            HighlightStart(Highlight(1)),
            HighlightStart(Highlight(2)),
            HighlightStart(Highlight(3)),
            HighlightStart(Highlight(4)),
            Source { start: 0, end: 5 },
            HighlightEnd, // ends 4
            HighlightEnd, // ends 3
            HighlightEnd, // ends 2
            HighlightEnd, // ends 1
        ],
    );
}

#[test]
fn test_highlight_event_stream_merge_right_is_truncated() {
    use HighlightEvent::*;
    // This can happen when there are selections outside of the
    // viewport. `left` is the syntax highlight event stream and
    // `right` is the `span_iter` of selections/cursors.

    /*
    Left:
                          1
                |-------------------|

        |---|---|---|---|---|---|---|---|---|---|
        0   1   2   3   4   5   6   7   8   9   10
    */
    let left = vec![
        HighlightStart(Highlight(1)),
        Source { start: 2, end: 7 },
        HighlightEnd, // ends 1
    ]
    .into_iter();

    /*
    Right:
          2                 3                 4
        |---|-------------------------------|---|

        |---|---|---|---|---|---|---|---|---|---|
        0   1   2   3   4   5   6   7   8   9   10
    */
    let right = Box::new(
        vec![
            HighlightStart(Highlight(2)),
            Source { start: 0, end: 1 },
            HighlightEnd, // ends 2
            HighlightStart(Highlight(3)),
            Source { start: 1, end: 9 },
            HighlightEnd, // ends 3
            HighlightStart(Highlight(4)),
            Source { start: 9, end: 10 },
            HighlightEnd, // ends 4
        ]
        .into_iter(),
    );

    // 2 and 4 are out of range and are discarded. 3 is truncated at the
    // beginning but allowed to finish after the `left`. This is a special
    // case for the trailing space from selection highlights.
    /*
    Output:
                         1, 3           3
                |-------------------|-------|

        |---|---|---|---|---|---|---|---|---|---|
        0   1   2   3   4   5   6   7   8   9   10
    */
    let output: Vec<_> = merge(left, right).collect();

    assert_eq!(
        output,
        &[
            HighlightStart(Highlight(1)),
            HighlightStart(Highlight(3)),
            Source { start: 2, end: 7 },
            HighlightEnd, // ends 3
            HighlightEnd, // ends 1
            HighlightStart(Highlight(3)),
            Source { start: 7, end: 9 },
            HighlightEnd, // ends 3
        ],
    );
}

#[test]
fn test_highlight_event_stream_right_ends_before_left_starts() {
    use HighlightEvent::*;

    /*
    Left:
                                      1
                            |-------------------|

        |---|---|---|---|---|---|---|---|---|---|
        0   1   2   3   4   5   6   7   8   9   10
    */
    let left = vec![
        HighlightStart(Highlight(1)),
        Source { start: 5, end: 10 },
        HighlightEnd, // ends 1
    ];

    /*
    Right:
                2
        |---------------|

        |---|---|---|---|---|---|---|---|---|---|
        0   1   2   3   4   5   6   7   8   9   10
    */
    let right = Box::new(
        vec![
            HighlightStart(Highlight(2)),
            Source { start: 0, end: 4 },
            HighlightEnd, // ends 2
        ]
        .into_iter(),
    );

    // Left starts after right ends. Right is discarded.
    let output: Vec<_> = merge(left.clone().into_iter(), right).collect();
    assert_eq!(output, left);
}

#[test]
fn test_highlight_event_stream_right_ends_as_left_starts() {
    use HighlightEvent::*;

    /*
    Left:
                                      1
                            |-------------------|

        |---|---|---|---|---|---|---|---|---|---|
        0   1   2   3   4   5   6   7   8   9   10
    */
    let left = vec![
        HighlightStart(Highlight(1)),
        Source { start: 5, end: 10 },
        HighlightEnd, // ends 1
    ];

    /*
    Right:
                  2
        |-------------------|

        |---|---|---|---|---|---|---|---|---|---|
        0   1   2   3   4   5   6   7   8   9   10
    */
    let right = Box::new(
        vec![
            HighlightStart(Highlight(2)),
            Source { start: 0, end: 5 },
            HighlightEnd, // ends 2
        ]
        .into_iter(),
    );

    // Right is discarded if the range ends in the same place as left starts.
    let output: Vec<_> = merge(left.clone().into_iter(), right).collect();
    assert_eq!(output, left);
}

#[test]
fn test_highlight_event_stream_merge_layered_overlapping() {
    use HighlightEvent::*;

    /*
    Left:
                          1
                    |-----------|

        |---|---|---|---|---|---|---|---|---|---|
        0   1   2   3   4   5   6   7   8   9   10
    */
    let left = vec![
        HighlightStart(Highlight(1)),
        Source { start: 3, end: 6 },
        HighlightEnd, // ends 1
    ]
    .into_iter();

    /*
    Right:
                          3     4 (0-width)
                    |-----------|

                            2
            |-------------------------------|

        |---|---|---|---|---|---|---|---|---|---|
        0   1   2   3   4   5   6   7   8   9   10
    */
    let right = Box::new(
        vec![
            HighlightStart(Highlight(2)),
            Source { start: 1, end: 3 },
            HighlightStart(Highlight(3)),
            Source { start: 3, end: 6 },
            HighlightEnd, // ends 3
            HighlightStart(Highlight(4)),
            // Trimmed zero-width Source.
            HighlightEnd, // ends 4
            Source { start: 6, end: 9 },
            HighlightEnd, // ends 2
        ]
        .into_iter(),
    );

    /*
    Output:
                        1,2,3
                    |-----------|

        |---|---|---|---|---|---|---|---|---|---|
        0   1   2   3   4   5   6   7   8   9   10
    */
    let output: Vec<_> = merge(left, right).collect();

    assert_eq!(
        output,
        &[
            HighlightStart(Highlight(1)),
            HighlightStart(Highlight(2)),
            HighlightStart(Highlight(3)),
            Source { start: 3, end: 6 },
            HighlightEnd, // ends 3
            HighlightEnd, // ends 2
            HighlightEnd, // ends 1
        ],
    );
}

#[test]
fn test_highlight_event_stream_merge_double_zero_width_span() {
    use HighlightEvent::*;
    // This is possible when merging two syntax highlight iterators.
    // Syntax highlight iterators may produce a HighlightStart
    // immediately followed by a HighlightEnd.

    /*
    Left:
                          1     2 (0-width)
                    |-----------|

        |---|---|---|---|---|---|---|---|---|---|
        0   1   2   3   4   5   6   7   8   9   10
    */
    let left = vec![
        HighlightStart(Highlight(1)),
        Source { start: 3, end: 6 },
        HighlightEnd, // ends 1
        HighlightStart(Highlight(2)),
        HighlightEnd, // ends 2
    ]
    .into_iter();

    /*
    Right:
                          3     4 (0-width)
                    |-----------|

        |---|---|---|---|---|---|---|---|---|---|
        0   1   2   3   4   5   6   7   8   9   10
    */
    let right = Box::new(
        vec![
            HighlightStart(Highlight(3)),
            Source { start: 3, end: 6 },
            HighlightEnd, // ends 3
            HighlightStart(Highlight(4)),
            HighlightEnd, // ends 4
        ]
        .into_iter(),
    );

    /*
    Output:
                         1,3
                    |-----------|

        |---|---|---|---|---|---|---|---|---|---|
        0   1   2   3   4   5   6   7   8   9   10
    */
    let output: Vec<_> = merge(left, right).collect();

    assert_eq!(
        output,
        &[
            HighlightStart(Highlight(1)),
            HighlightStart(Highlight(3)),
            Source { start: 3, end: 6 },
            HighlightEnd, // ends 3
            HighlightEnd, // ends 1
            HighlightStart(Highlight(4)),
            // Zero-width Source span.
            HighlightEnd, // ends 4
        ],
    );
}

fn span(file_size: usize, allow_empty: bool, scope: usize) -> impl Strategy<Value = Span> + Clone {
    let start = 0..file_size;
    start
        .prop_flat_map(move |start| (Just(start), start..file_size))
        .prop_map(move |(start, end)| Span {
            scope,
            start,
            end: if allow_empty { end } else { end + 1 },
        })
}

/// The maximum number of created spans.
/// Must not surpass 128 because `HighlightSet` can not represent more elements
/// When trying to reduce a regression it is often useful to reduce this significantly
const MAX_SPAN_LIST_SIZE: usize = 128;
const MAX_FILE_SIZE: usize = 200;

fn span_list() -> impl Strategy<Value = Vec<Span>> + Clone {
    let file_size = 1..MAX_FILE_SIZE;
    let span_list_size = 0..MAX_SPAN_LIST_SIZE;
    (file_size, span_list_size)
        .prop_flat_map(|(file_size, span_list_size)| {
            let ranges: Vec<_> = (0..span_list_size)
                .map(|i| span(file_size, false, i))
                .collect();
            ranges
        })
        .prop_map(|mut ranges| {
            ranges.sort_unstable();
            ranges
        })
}

fn check_highlight_event_invariants(
    events: impl Iterator<Item = HighlightEvent>,
) -> TestCaseResult {
    let mut missing_end_events = 0;
    let mut last_source_range = None;
    let mut prev_event_was_source = false;
    for event in events {
        match event {
            HighlightEvent::Source { start, end } => {
                prop_assert_ne!(start, end, "empty source events are invalid");
                prop_assert!(
                    !prev_event_was_source,
                    "consectuive source events are not allowed {:?} {start}..{end}",
                    last_source_range.unwrap()
                );
                match last_source_range {
                    None => last_source_range = Some(start..end),
                    Some(old_range) => {
                        prop_assert!(
                            old_range.start < start && old_range.end <= start,
                            "source ranges must be monotonically increasing but range {start}..{end} starts in previous range {old_range:?}"
                        );
                        last_source_range = Some(start..end);
                    }
                }

                prev_event_was_source = true;
            }
            HighlightEvent::HighlightStart(_) => {
                prev_event_was_source = false;
                missing_end_events += 1;
            }
            HighlightEvent::HighlightEnd => {
                prev_event_was_source = false;
                missing_end_events -= 1;
            }
        }
    }

    prop_assert_eq!(
        missing_end_events,
        0,
        "number of end events must match number of start events"
    );

    Ok(())
}

/// prop_assert_ne but uses similar_assert for a more readable diff
macro_rules! prop_assert_eq {
    ($lhs: expr, $rhs: expr, $message: expr ) => {
        let lhs = $lhs;
        let rhs = $rhs;

        let lhs_label = stringify!($lhs);
        let rhs_label = stringify!($rhs);
        ::proptest::prop_assert!(
            lhs == rhs,
            "assertion failed: `({lhs_label} == {rhs_label})`{}\
                           \n\n{}\n",
            $message,
            similar_asserts::SimpleDiff::from_str(
                &format!("{lhs:#?}"),
                &&format!("{rhs:#?}"),
                lhs_label,
                rhs_label,
            )
        );
    };
}

proptest! {
    #[test]
    fn test_span_iter_invariants(spans in span_list()) {
        let events = span_iter(spans);
        check_highlight_event_invariants(events)?;
    }


    #[test]
    fn test_span_iter_highlights(spans in span_list()) {
        let reference_highlights: HighlightSet = spans.iter().copied().collect();
        let events: Vec<_> = span_iter(spans).collect();
        let computed_highlights: HighlightSet = events.iter().copied().collect();
        prop_assert_eq!(reference_highlights, computed_highlights, format_args!("\n{events:#?}\n"));
    }
}
