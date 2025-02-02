use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet},
    ops::Range,
    sync::Arc,
};

use helix_core::{
    self as core, chars::char_is_word, completion::CompletionProvider, movement, Transaction,
};
use helix_event::TaskHandle;
use helix_stdx::rope::RopeSliceExt;
use helix_view::{
    document::SavePoint, handlers::completion::ResponseContext, Document, Editor, View,
};

use crate::handlers::completion::{CompletionItems, CompletionResponse};

use super::{item::CompletionItem, request::TriggerKind, Trigger};

const COMPLETION_KIND: &str = "word";

pub(super) fn retain_valid_completions(
    trigger: Trigger,
    doc: &Document,
    view: &View,
    items: &mut Vec<CompletionItem>,
) {
    if trigger.kind == TriggerKind::Manual {
        return;
    }

    let text = doc.text().slice(..);
    let cursor = doc.selection(view.id).primary().cursor(text);

    if text.char(cursor.saturating_sub(1)).is_whitespace() {
        items.retain(|item| {
            !matches!(
                item,
                CompletionItem::Other(core::CompletionItem {
                    kind: Cow::Borrowed(COMPLETION_KIND),
                    ..
                })
            )
        })
    }
}

pub(super) fn completion(
    editor: &Editor,
    trigger: Trigger,
    handle: TaskHandle,
    savepoint: Arc<SavePoint>,
) -> Option<impl FnOnce() -> CompletionResponse> {
    // The minimum number of grapheme clusters needed to suggest a word.
    let min_word_len = match trigger.kind {
        TriggerKind::Manual => 2,
        _ => 8,
    };

    let (view, doc) = current_ref!(editor);
    let rope = doc.text().clone();
    let text = doc.text().slice(..);
    let selection = doc.selection(view.id).clone();
    let pos = selection.primary().cursor(text);

    let cursor = movement::move_prev_word_start(text, core::Range::point(pos), 1);

    if cursor.head == pos {
        return None;
    }

    if trigger.kind != TriggerKind::Manual
        && text
            .slice(cursor.head..)
            .graphemes()
            .take(min_word_len)
            .take_while(|g| g.chars().all(char_is_word))
            .count()
            != min_word_len
    {
        return None;
    }

    let typed_word_range = cursor.head..pos;
    let prev_word = text.slice(typed_word_range.clone());
    let edit_diff = if prev_word
        .char(prev_word.len_chars().saturating_sub(1))
        .is_whitespace()
    {
        0
    } else {
        prev_word.chars().count()
    };

    let mut ranges = BTreeMap::new();
    for (view, _is_focused) in editor.tree.views() {
        let doc = doc!(editor, &view.doc);
        let text = doc.text().slice(..);
        let start = text.char_to_line(doc.view_offset(view.id).anchor);
        let end = view.estimate_last_doc_line(doc) + 1;

        ranges
            .entry(doc.id())
            .and_modify(|(_text, ranges): &mut (core::Rope, Vec<Range<usize>>)| {
                let range = start..end;
                // If this range overlaps with an existing one, merge the ranges.
                for r in ranges.iter_mut() {
                    if range_overlaps(&range, r) {
                        *r = range_union(&range, r);
                        return;
                    }
                }
                // If no range overlaps, add a new range for this doc.
                ranges.push(range);
            })
            .or_insert_with(|| {
                // This lint doesn't account for the Vec being mutable: it can store potentially
                // many ranges.
                #[allow(clippy::single_range_in_vec_init)]
                (doc.text().clone(), vec![start..end])
            });
    }

    if handle.is_canceled() {
        return None;
    }

    let future = move || {
        let mut words = BTreeSet::new();
        for (_doc_id, (text, ranges)) in ranges {
            let text = text.slice(..);
            for range in ranges {
                // TODO: the first word in a buffer can't be completed.
                let start = text.line_to_char(range.start);
                let end = text.line_to_char(range.end);
                let mut cursor = core::Range::point(start);
                if text.get_char(start).is_some_and(|c| !c.is_whitespace()) {
                    let cursor_word_end = movement::move_next_word_end(text, cursor, 1);
                    if cursor_word_end.anchor == start {
                        cursor = cursor_word_end;
                    }
                }
                while cursor.head < end {
                    if text
                        .slice(..cursor.head)
                        .graphemes_rev()
                        .take(min_word_len)
                        .take_while(|g| g.chars().all(char_is_word))
                        .count()
                        == min_word_len
                    {
                        cursor.anchor += text
                            .chars_at(cursor.anchor)
                            .take_while(|&c| !char_is_word(c))
                            .count();
                        let word_range = cursor.anchor..cursor.head;
                        // Don't insert the word which is currently being typed.
                        // We could consider subtracting the currently typed word from the
                        // set instead. I think the desired behavior though is to not include
                        // what is being typed rather than not including something like what
                        // is being typed.
                        if !range_overlaps(&typed_word_range, &word_range) {
                            words.insert(text.slice(word_range).to_string());
                        }
                    }
                    cursor = movement::move_next_word_end(text, cursor, 1);
                }
            }
        }

        let items: Vec<_> = words
            .into_iter()
            .map(|word| {
                let transaction = Transaction::change_by_selection(&rope, &selection, |range| {
                    let cursor = range.cursor(rope.slice(..));
                    (cursor - edit_diff, cursor, Some((&word).into()))
                });

                CompletionItem::Other(core::CompletionItem {
                    transaction,
                    label: word.into(),
                    kind: Cow::Borrowed(COMPLETION_KIND),
                    documentation: None,
                    provider: CompletionProvider::Word,
                })
            })
            .collect();

        // TODO: handle properly in the future
        const PRIORITY: i8 = 1;

        CompletionResponse {
            items: CompletionItems::Other(items),
            provider: CompletionProvider::Word,
            context: ResponseContext {
                is_incomplete: false,
                priority: PRIORITY,
                savepoint,
            },
        }
    };

    Some(future)
}

fn range_overlaps(a: &Range<usize>, b: &Range<usize>) -> bool {
    // See `Range::overlaps` in `helix_core`.
    a.start == b.start || (a.end > b.start && b.end > a.start)
}

fn range_union(a: &Range<usize>, b: &Range<usize>) -> Range<usize> {
    let start = a.start.min(b.start);
    let end = a.end.max(b.end);
    start..end
}
