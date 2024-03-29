use crate::{syntax::TreeCursor, Range, RopeSlice, Selection, Syntax};

pub fn expand_selection(syntax: &Syntax, text: RopeSlice, selection: Selection) -> Selection {
    select_node_impl(syntax, text, selection, |cursor, byte_range| {
        while cursor.node().byte_range() == byte_range {
            if !cursor.goto_parent() {
                break;
            }
        }
    })
}

pub fn shrink_selection(syntax: &Syntax, text: RopeSlice, selection: Selection) -> Selection {
    select_node_impl(syntax, text, selection, |cursor, byte_range| {
        cursor.goto_first_child();
        while cursor.node().start_byte() < byte_range.start
            || cursor.node().end_byte() > byte_range.end
        {
            if !cursor.goto_next_sibling() {
                // If a child within the range couldn't be found, default to the first child.
                cursor.goto_parent();
                cursor.goto_first_child();
                break;
            }
        }
    })
}

pub fn select_next_sibling(syntax: &Syntax, text: RopeSlice, selection: Selection) -> Selection {
    select_node_impl(syntax, text, selection, |cursor, _byte_range| {
        while !cursor.goto_next_sibling() {
            if !cursor.goto_parent() {
                break;
            }
        }
    })
}

pub fn select_prev_sibling(syntax: &Syntax, text: RopeSlice, selection: Selection) -> Selection {
    select_node_impl(syntax, text, selection, |cursor, _byte_range| {
        while !cursor.goto_prev_sibling() {
            if !cursor.goto_parent() {
                break;
            }
        }
    })
}

fn select_node_impl<F>(
    syntax: &Syntax,
    text: RopeSlice,
    selection: Selection,
    motion: F,
) -> Selection
where
    // Fn(tree cursor, original selection's byte range)
    F: Fn(&mut TreeCursor, std::ops::Range<usize>),
{
    let cursor = &mut syntax.walk();

    selection.transform(|range| {
        let from = text.char_to_byte(range.from());
        let to = text.char_to_byte(range.to());

        cursor.reset_to_byte_range(from, to);

        motion(cursor, from..to);

        let node = cursor.node();
        let from = text.byte_to_char(node.start_byte());
        let to = text.byte_to_char(node.end_byte());

        Range::new(from, to).with_direction(range.direction())
    })
}
