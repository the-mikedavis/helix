pub mod config;

use std::{
    borrow::Cow,
    fmt, fs,
    ops::{self, RangeBounds},
    path::Path,
    sync::Arc,
    time::Duration,
};

use anyhow::{anyhow, bail, Context, Result};
use arc_swap::{ArcSwap, Guard};
use config::{Configuration, FileType, LanguageConfiguration, LanguageServerConfiguration};
use foldhash::{HashMap, HashSet};
use helix_loader::grammar;
use helix_stdx::rope::RopeSliceExt as _;
use once_cell::sync::OnceCell;
use ropey::RopeSlice;
use tree_house::{
    highlighter,
    query_iter::QueryIter,
    tree_sitter::{query::UserPredicate, Capture, Grammar, InputEdit, Node, Pattern, Query, Tree},
    Error, InjectionLanguageMarker, LanguageConfig as SyntaxConfig, LanguageLoader, Layer,
};

use crate::{indent::IndentQuery, textobject::TextObjectQuery, tree_sitter, ChangeSet, Language};

pub use tree_house::{
    highlighter::{Highlight, HighlightEvent},
    query_iter::QueryIterEvent,
    Error as HighlighterError, TreeCursor, TREE_SITTER_MATCH_LIMIT,
};

#[derive(Debug)]
pub struct LanguageData {
    config: Arc<LanguageConfiguration>,
    syntax: OnceCell<Option<SyntaxConfig>>,
    indent_query: OnceCell<Option<IndentQuery>>,
    textobject_query: OnceCell<Option<TextObjectQuery>>,
    rainbow_query: OnceCell<Option<RainbowQuery>>,
}

impl LanguageData {
    fn new(config: LanguageConfiguration) -> Self {
        Self {
            config: Arc::new(config),
            syntax: OnceCell::new(),
            indent_query: OnceCell::new(),
            textobject_query: OnceCell::new(),
            rainbow_query: OnceCell::new(),
        }
    }

    pub fn config(&self) -> &Arc<LanguageConfiguration> {
        &self.config
    }

    fn syntax_config(&self, loader: &Loader) -> Option<&SyntaxConfig> {
        fn compile_syntax_config(
            config: &LanguageConfiguration,
            loader: &Loader,
        ) -> Result<SyntaxConfig> {
            let grammars = &loader.grammar_loader;
            let name = &config.language_id;
            let parser_name = config.grammar.as_deref().unwrap_or(name);
            let Some((grammar, library_path)) =
                grammars.get_compiled_parser_path(config.grammar.as_deref().unwrap_or(name))
            else {
                bail!("No parser file found for '{parser_name}' in any language support repo");
            };
            let grammar = unsafe { Grammar::new(&grammar, &library_path) }?;

            let grammar_dir = grammars.grammar_dir(name).ok_or_else(|| {
                anyhow!("Could not find a language support directory for '{name}'")
            })?;

            let highlight_query_path = grammar_dir.join("highlights.scm");
            let highlight_query_text = read_query(grammars, name, "highlights.scm");

            let injection_query_path = grammar_dir.join("injections.scm");
            let injection_query_text = read_query(grammars, name, "injections.scm");

            let local_query_path = grammar_dir.join("locals.scm");
            let local_query_text = read_query(grammars, name, "locals.scm");

            let config = SyntaxConfig::new(
                grammar,
                &highlight_query_text,
                highlight_query_path,
                &injection_query_text,
                injection_query_path,
                &local_query_text,
                local_query_path,
            )
            .with_context(|| format!("Failed to compile highlights for '{name}'"))?;
            reconfigure_highlights(&config, &loader.scopes());

            Ok(config)
        }

        self.syntax
            .get_or_init(|| {
                compile_syntax_config(&self.config, loader)
                    .map_err(|err| {
                        log::error!("{err:#}");
                    })
                    .ok()
            })
            .as_ref()
    }

    fn indent_query(&self, loader: &Loader) -> Option<&IndentQuery> {
        self.indent_query
            .get_or_init(|| {
                let name = &self.config.language_id;
                let grammar = self.syntax_config(loader)?.grammar;
                let mut path = loader
                    .grammar_loader
                    .grammar_dir(name)
                    .expect("must have a language support dir to have a syntax config");
                path.push("indents.scm");
                let text = read_query(&loader.grammar_loader, name, "indents.scm");
                if text.is_empty() {
                    return None;
                }
                IndentQuery::new(grammar, &text, path)
                    .map_err(|err| {
                        log::error!("Failed to compile indents.scm queries for '{name}': {err:#}");
                    })
                    .ok()
            })
            .as_ref()
    }

    fn textobject_query(&self, loader: &Loader) -> Option<&TextObjectQuery> {
        self.textobject_query
            .get_or_init(|| {
                let name = &self.config.language_id;
                let grammar = self.syntax_config(loader)?.grammar;
                let mut path = loader
                    .grammar_loader
                    .grammar_dir(name)
                    .expect("must have a language support dir to have a syntax config");
                path.push("textobjects.scm");
                let text = read_query(&loader.grammar_loader, name, "textobjects.scm");
                if text.is_empty() {
                    return None;
                }
                match Query::new(grammar, &text, path, |_, _| Ok(())) {
                    Ok(query) => Some(TextObjectQuery::new(query)),
                    Err(err) => {
                        log::error!(
                            "Failed to compile textobjects.scm queries for '{name}': {err:#}"
                        );
                        None
                    }
                }
            })
            .as_ref()
    }

    fn rainbow_query(&self, loader: &Loader) -> Option<&RainbowQuery> {
        self.rainbow_query
            .get_or_init(|| {
                let name = &self.config.language_id;
                let grammar = self.syntax_config(loader)?.grammar;
                let mut path = loader
                    .grammar_loader
                    .grammar_dir(name)
                    .expect("must have a language support dir to have a syntax config");
                path.push("rainbows.scm");
                let text = read_query(&loader.grammar_loader, name, "rainbows.scm");
                if text.is_empty() {
                    return None;
                }
                RainbowQuery::new(grammar, &text, path)
                    .map_err(|err| {
                        log::error!("Failed to compile rainbows.scm queries for '{name}': {err:#}");
                    })
                    .ok()
            })
            .as_ref()
    }

    fn reconfigure(&self, scopes: &[String]) {
        if let Some(Some(config)) = self.syntax.get() {
            reconfigure_highlights(config, scopes);
        }
    }
}

fn reconfigure_highlights(config: &SyntaxConfig, recognized_names: &[String]) {
    config.configure(move |capture_name| {
        let capture_parts: Vec<_> = capture_name.split('.').collect();

        let mut best_index = None;
        let mut best_match_len = 0;
        for (i, recognized_name) in recognized_names.iter().enumerate() {
            let mut len = 0;
            let mut matches = true;
            for (i, part) in recognized_name.split('.').enumerate() {
                match capture_parts.get(i) {
                    Some(capture_part) if *capture_part == part => len += 1,
                    _ => {
                        matches = false;
                        break;
                    }
                }
            }
            if matches && len > best_match_len {
                best_index = Some(i);
                best_match_len = len;
            }
        }
        best_index.map(|idx| Highlight::new(idx as u32))
    });
}

fn read_query(grammars: &grammar::Loader, lang: &str, query_filename: &str) -> String {
    tree_house::read_query(lang, |language| {
        let Some(mut path) = grammars.grammar_dir(language) else {
            return Default::default();
        };
        path.push(query_filename);
        fs::read_to_string(path).unwrap_or_default()
    })
}

#[derive(Debug, Default)]
pub struct Loader {
    languages: Vec<LanguageData>,
    languages_by_extension: HashMap<String, Language>,
    languages_by_shebang: HashMap<String, Language>,
    languages_glob_matcher: FileTypeGlobMatcher,
    language_server_configs: HashMap<String, LanguageServerConfiguration>,
    grammar_loader: grammar::Loader,
    scopes: ArcSwap<Vec<String>>,
}

pub type LoaderError = globset::Error;

impl Loader {
    pub fn new(config: Configuration) -> Result<Self, LoaderError> {
        let mut languages = Vec::with_capacity(config.language.len());
        let mut languages_by_extension = HashMap::default();
        let mut languages_by_shebang = HashMap::default();
        let mut file_type_globs = Vec::new();

        for mut config in config.language {
            let language = Language(languages.len() as u32);
            config.language = Some(language);

            for file_type in &config.file_types {
                match file_type {
                    FileType::Extension(extension) => {
                        languages_by_extension.insert(extension.clone(), language);
                    }
                    FileType::Glob(glob) => {
                        file_type_globs.push(FileTypeGlob::new(glob.to_owned(), language));
                    }
                };
            }
            for shebang in &config.shebangs {
                languages_by_shebang.insert(shebang.clone(), language);
            }

            languages.push(LanguageData::new(config));
        }

        let grammar_loader = grammar::Loader::new(&config.language_support_repo);

        Ok(Self {
            languages,
            languages_by_extension,
            languages_by_shebang,
            languages_glob_matcher: FileTypeGlobMatcher::new(file_type_globs)?,
            language_server_configs: config.language_server,
            grammar_loader,
            scopes: ArcSwap::from_pointee(Vec::new()),
        })
    }

    pub fn language_configs(&self) -> impl Iterator<Item = &LanguageConfiguration> {
        self.languages.iter().map(|language| &*language.config)
    }

    pub fn language(&self, lang: Language) -> &LanguageData {
        &self.languages[lang.idx()]
    }

    pub fn language_for_name(&self, name: impl PartialEq<String>) -> Option<Language> {
        self.languages.iter().enumerate().find_map(|(idx, config)| {
            (name == config.config.language_id).then_some(Language(idx as u32))
        })
    }

    pub fn language_for_match(&self, text: RopeSlice) -> Option<Language> {
        // PERF: If the name matches up with the id, then this saves the need to do expensive regex.
        let shortcircuit = self.language_for_name(text);
        if shortcircuit.is_some() {
            return shortcircuit;
        }

        // If the name did not match up with a known id, then match on injection regex.

        let mut best_match_length = 0;
        let mut best_match_position = None;
        for (idx, data) in self.languages.iter().enumerate() {
            if let Some(injection_regex) = &data.config.injection_regex {
                if let Some(mat) = injection_regex.find(text.regex_input()) {
                    let length = mat.end() - mat.start();
                    if length > best_match_length {
                        best_match_position = Some(idx);
                        best_match_length = length;
                    }
                }
            }
        }

        best_match_position.map(|i| Language(i as u32))
    }

    pub fn language_for_filename(&self, path: &Path) -> Option<Language> {
        // Find all the language configurations that match this file name
        // or a suffix of the file name.

        // TODO: content_regex handling conflict resolution
        self.languages_glob_matcher
            .language_for_path(path)
            .or_else(|| {
                path.extension()
                    .and_then(|extension| extension.to_str())
                    .and_then(|extension| self.languages_by_extension.get(extension).copied())
            })
    }

    pub fn language_for_shebang(&self, text: RopeSlice) -> Option<Language> {
        let shebang: Cow<str> = text.into();
        self.languages_by_shebang.get(shebang.as_ref()).copied()
    }

    pub fn indent_query(&self, lang: Language) -> Option<&IndentQuery> {
        self.language(lang).indent_query(self)
    }

    pub fn textobject_query(&self, lang: Language) -> Option<&TextObjectQuery> {
        self.language(lang).textobject_query(self)
    }

    pub fn rainbow_query(&self, lang: Language) -> Option<&RainbowQuery> {
        self.language(lang).rainbow_query(self)
    }

    pub fn language_server_configs(&self) -> &HashMap<String, LanguageServerConfiguration> {
        &self.language_server_configs
    }

    pub fn scopes(&self) -> Guard<Arc<Vec<String>>> {
        self.scopes.load()
    }

    pub fn set_scopes(&self, scopes: Vec<String>) {
        self.scopes.store(Arc::new(scopes));

        // Reconfigure existing grammars
        for data in &self.languages {
            data.reconfigure(&self.scopes());
        }
    }

    pub fn grammar_loader(&self) -> &grammar::Loader {
        &self.grammar_loader
    }
}

impl LanguageLoader for Loader {
    fn language_for_marker(&self, marker: InjectionLanguageMarker) -> Option<Language> {
        match marker {
            InjectionLanguageMarker::Name(name) => self.language_for_name(name),
            InjectionLanguageMarker::Match(text) => self.language_for_match(text),
            InjectionLanguageMarker::Filename(text) => {
                let path: Cow<str> = text.into();
                self.language_for_filename(Path::new(path.as_ref()))
            }
            InjectionLanguageMarker::Shebang(text) => self.language_for_shebang(text),
        }
    }

    fn get_config(&self, lang: Language) -> Option<&SyntaxConfig> {
        self.languages[lang.idx()].syntax_config(self)
    }
}

#[derive(Debug)]
struct FileTypeGlob {
    glob: globset::Glob,
    language: Language,
}

impl FileTypeGlob {
    pub fn new(glob: globset::Glob, language: Language) -> Self {
        Self { glob, language }
    }
}

#[derive(Debug)]
struct FileTypeGlobMatcher {
    matcher: globset::GlobSet,
    file_types: Vec<FileTypeGlob>,
}

impl Default for FileTypeGlobMatcher {
    fn default() -> Self {
        Self {
            matcher: globset::GlobSet::empty(),
            file_types: Default::default(),
        }
    }
}

impl FileTypeGlobMatcher {
    fn new(file_types: Vec<FileTypeGlob>) -> Result<Self, globset::Error> {
        let mut builder = globset::GlobSetBuilder::new();
        for file_type in &file_types {
            builder.add(file_type.glob.clone());
        }

        Ok(Self {
            matcher: builder.build()?,
            file_types,
        })
    }

    fn language_for_path(&self, path: &Path) -> Option<Language> {
        self.matcher
            .matches(path)
            .iter()
            .filter_map(|idx| self.file_types.get(*idx))
            .max_by_key(|file_type| file_type.glob.glob().len())
            .map(|file_type| file_type.language)
    }
}

#[derive(Debug)]
pub struct Syntax {
    inner: tree_house::Syntax,
}

const PARSE_TIMEOUT: Duration = Duration::from_millis(500); // half a second is pretty generous

impl Syntax {
    pub fn new(source: RopeSlice, language: Language, loader: &Loader) -> Result<Self, Error> {
        let inner = tree_house::Syntax::new(source, language, PARSE_TIMEOUT, loader)?;
        Ok(Self { inner })
    }

    pub fn update(
        &mut self,
        old_source: RopeSlice,
        source: RopeSlice,
        changeset: &ChangeSet,
        loader: &Loader,
    ) -> Result<(), Error> {
        let edits = generate_edits(old_source, changeset);
        if edits.is_empty() {
            Ok(())
        } else {
            self.inner.update(source, PARSE_TIMEOUT, &edits, loader)
        }
    }

    pub fn layer(&self, layer: Layer) -> &tree_house::LayerData {
        self.inner.layer(layer)
    }

    pub fn root_layer(&self) -> Layer {
        self.inner.root()
    }

    pub fn layer_for_byte_range(&self, start: u32, end: u32) -> Layer {
        self.inner.layer_for_byte_range(start, end)
    }

    pub fn root_language(&self) -> Language {
        self.layer(self.root_layer()).language
    }

    pub fn tree(&self) -> &Tree {
        self.inner.tree()
    }

    pub fn tree_for_byte_range(&self, start: u32, end: u32) -> &Tree {
        self.inner.tree_for_byte_range(start, end)
    }

    pub fn named_descendant_for_byte_range(&self, start: u32, end: u32) -> Option<Node> {
        self.inner.named_descendant_for_byte_range(start, end)
    }

    pub fn descendant_for_byte_range(&self, start: u32, end: u32) -> Option<Node> {
        self.inner.descendant_for_byte_range(start, end)
    }

    pub fn walk(&self) -> TreeCursor {
        self.inner.walk()
    }

    pub fn highlighter<'a>(
        &'a self,
        source: RopeSlice<'a>,
        loader: &'a Loader,
        range: impl RangeBounds<u32>,
    ) -> Highlighter<'a> {
        Highlighter::new(&self.inner, source, loader, range)
    }

    pub fn query_iter<'a, QueryLoader, LayerState, Range>(
        &'a self,
        source: RopeSlice<'a>,
        loader: QueryLoader,
        range: Range,
    ) -> QueryIter<'a, 'a, QueryLoader, LayerState>
    where
        QueryLoader: FnMut(Language) -> Option<&'a Query> + 'a,
        LayerState: Default,
        Range: RangeBounds<u32>,
    {
        QueryIter::new(&self.inner, source, loader, range)
    }
}

pub type Highlighter<'a> = highlighter::Highlighter<'a, 'a, Loader>;

fn generate_edits(old_text: RopeSlice, changeset: &ChangeSet) -> Vec<InputEdit> {
    use crate::Operation::*;

    let mut old_pos = 0;

    let mut edits = Vec::new();

    if changeset.changes.is_empty() {
        return edits;
    }

    let mut iter = changeset.changes.iter().peekable();

    // The tree and subtree code does not consider point information. Set it to zero for
    // simplicity.
    let zero_point = tree_sitter::Point { row: 0, col: 0 };

    // TODO; this is a lot easier with Change instead of Operation.
    while let Some(change) = iter.next() {
        let len = match change {
            Delete(i) | Retain(i) => *i,
            Insert(_) => 0,
        };
        let mut old_end = old_pos + len;

        match change {
            Retain(_) => {}
            Delete(_) => {
                let start_byte = old_text.char_to_byte(old_pos) as u32;
                let old_end_byte = old_text.char_to_byte(old_end) as u32;

                // deletion
                edits.push(InputEdit {
                    start_byte,               // old_pos to byte
                    old_end_byte,             // old_end to byte
                    new_end_byte: start_byte, // old_pos to byte
                    start_point: zero_point,
                    old_end_point: zero_point,
                    new_end_point: zero_point,
                });
            }
            Insert(s) => {
                let start_byte = old_text.char_to_byte(old_pos) as u32;

                // a subsequent delete means a replace, consume it
                if let Some(Delete(len)) = iter.peek() {
                    old_end = old_pos + len;
                    let old_end_byte = old_text.char_to_byte(old_end) as u32;

                    iter.next();

                    // replacement
                    edits.push(InputEdit {
                        start_byte,                                // old_pos to byte
                        old_end_byte,                              // old_end to byte
                        new_end_byte: start_byte + s.len() as u32, // old_pos to byte + s.len()
                        start_point: zero_point,
                        old_end_point: zero_point,
                        new_end_point: zero_point,
                    });
                } else {
                    // insert
                    edits.push(InputEdit {
                        start_byte,                                // old_pos to byte
                        old_end_byte: start_byte,                  // same
                        new_end_byte: start_byte + s.len() as u32, // old_pos + s.len()
                        start_point: zero_point,
                        old_end_point: zero_point,
                        new_end_point: zero_point,
                    });
                }
            }
        }
        old_pos = old_end;
    }
    edits
}

/// A set of "overlay" highlights and ranges they apply to.
///
/// As overlays, the styles for the given `Highlight`s are merged on top of the syntax highlights.
#[derive(Debug)]
pub enum OverlayHighlights {
    /// All highlights use a single `Highlight`.
    ///
    /// Note that, currently, all ranges are assumed to be non-overlapping. This could change in
    /// the future though.
    Homogeneous {
        highlight: Highlight,
        ranges: Vec<ops::Range<usize>>,
    },
    /// A collection of different highlights for given ranges.
    ///
    /// Note that the ranges **must be non-overlapping**.
    Heterogenous {
        highlights: Vec<(Highlight, ops::Range<usize>)>,
    },
}

impl OverlayHighlights {
    pub fn single(highlight: Highlight, range: ops::Range<usize>) -> Self {
        Self::Homogeneous {
            highlight,
            ranges: vec![range],
        }
    }

    fn is_empty(&self) -> bool {
        match self {
            Self::Homogeneous { ranges, .. } => ranges.is_empty(),
            Self::Heterogenous { highlights } => highlights.is_empty(),
        }
    }
}

#[derive(Debug)]
struct Overlay {
    highlights: OverlayHighlights,
    /// The position of the highlighter into the Vec of ranges of the overlays.
    ///
    /// Used by the `OverlayHighlighter`.
    idx: usize,
    /// The currently active highlight (and the ending character index) for this overlay.
    ///
    /// Used by the `OverlayHighlighter`.
    active_highlight: Option<(Highlight, usize)>,
}

impl Overlay {
    fn new(highlights: OverlayHighlights) -> Option<Self> {
        (!highlights.is_empty()).then_some(Self {
            highlights,
            idx: 0,
            active_highlight: None,
        })
    }

    fn current(&self) -> Option<(Highlight, ops::Range<usize>)> {
        match &self.highlights {
            OverlayHighlights::Homogeneous { highlight, ranges } => ranges
                .get(self.idx)
                .map(|range| (*highlight, range.clone())),
            OverlayHighlights::Heterogenous { highlights } => highlights.get(self.idx).cloned(),
        }
    }

    fn start(&self) -> Option<usize> {
        match &self.highlights {
            OverlayHighlights::Homogeneous { ranges, .. } => {
                ranges.get(self.idx).map(|range| range.start)
            }
            OverlayHighlights::Heterogenous { highlights } => highlights
                .get(self.idx)
                .map(|(_highlight, range)| range.start),
        }
    }
}

/// A collection of highlights to apply when rendering which merge on top of syntax highlights.
#[derive(Debug)]
pub struct OverlayHighlighter {
    overlays: Vec<Overlay>,
    next_highlight_start: usize,
    next_highlight_end: usize,
}

impl OverlayHighlighter {
    pub fn new(overlays: impl IntoIterator<Item = OverlayHighlights>) -> Self {
        let overlays: Vec<_> = overlays.into_iter().filter_map(Overlay::new).collect();
        let next_highlight_start = overlays
            .iter()
            .filter_map(|overlay| overlay.start())
            .min()
            .unwrap_or(usize::MAX);

        Self {
            overlays,
            next_highlight_start,
            next_highlight_end: usize::MAX,
        }
    }

    /// The current position in the overlay highlights.
    ///
    /// This method is meant to be used when treating this type as a cursor over the overlay
    /// highlights.
    ///
    /// `usize::MAX` is returned when there are no more overlay highlights.
    pub fn next_event_offset(&self) -> usize {
        self.next_highlight_start.min(self.next_highlight_end)
    }

    pub fn advance(&mut self) -> (HighlightEvent, impl Iterator<Item = Highlight> + '_) {
        let mut refresh = false;
        let prev_stack_size = self
            .overlays
            .iter()
            .filter(|overlay| overlay.active_highlight.is_some())
            .count();
        let pos = self.next_event_offset();

        if self.next_highlight_end == pos {
            for overlay in self.overlays.iter_mut() {
                if overlay
                    .active_highlight
                    .is_some_and(|(_highlight, end)| end == pos)
                {
                    overlay.active_highlight.take();
                }
            }

            refresh = true;
        }

        while self.next_highlight_start == pos {
            let mut activated_idx = usize::MAX;
            for (idx, overlay) in self.overlays.iter_mut().enumerate() {
                let Some((highlight, range)) = overlay.current() else {
                    continue;
                };
                if range.start != self.next_highlight_start {
                    continue;
                }

                // If this overlay has a highlight at this start index, set its active highlight
                // and increment the cursor position within the overlay.
                overlay.active_highlight = Some((highlight, range.end));
                overlay.idx += 1;

                activated_idx = activated_idx.min(idx);
            }

            // If `self.next_highlight_start == pos` that means that some overlay was ready to
            // emit a highlight, so `activated_idx` must have been set to an existing index.
            assert!(
                (0..self.overlays.len()).contains(&activated_idx),
                "expected an overlay to highlight (at pos {pos}, there are {} overlays)",
                self.overlays.len()
            );

            // If any overlays are active after the (lowest) one which was just activated, the
            // highlights need to be refreshed.
            refresh |= self.overlays[activated_idx..]
                .iter()
                .any(|overlay| overlay.active_highlight.is_some());

            self.next_highlight_start = self
                .overlays
                .iter()
                .filter_map(|overlay| overlay.start())
                .min()
                .unwrap_or(usize::MAX);
        }

        self.next_highlight_end = self
            .overlays
            .iter()
            .filter_map(|overlay| Some(overlay.active_highlight?.1))
            .min()
            .unwrap_or(usize::MAX);

        let (event, start) = if refresh {
            (HighlightEvent::Refresh, 0)
        } else {
            (HighlightEvent::Push, prev_stack_size)
        };

        (
            event,
            self.overlays
                .iter()
                .flat_map(|overlay| overlay.active_highlight)
                .map(|(highlight, _end)| highlight)
                .skip(start),
        )
    }
}

pub fn pretty_print_tree<W: fmt::Write>(fmt: &mut W, node: Node) -> fmt::Result {
    if node.child_count() == 0 {
        if node_is_visible(&node) {
            write!(fmt, "({})", node.kind())
        } else {
            write!(fmt, "\"{}\"", format_anonymous_node_kind(node.kind()))
        }
    } else {
        pretty_print_tree_impl(fmt, &mut node.walk(), 0)
    }
}

fn node_is_visible(node: &Node) -> bool {
    node.is_missing() || (node.is_named() && node.grammar().node_kind_is_visible(node.kind_id()))
}

fn format_anonymous_node_kind(kind: &str) -> Cow<str> {
    if kind.contains('"') {
        Cow::Owned(kind.replace('"', "\\\""))
    } else {
        Cow::Borrowed(kind)
    }
}

fn pretty_print_tree_impl<W: fmt::Write>(
    fmt: &mut W,
    cursor: &mut tree_sitter::TreeCursor,
    depth: usize,
) -> fmt::Result {
    let node = cursor.node();
    let visible = node_is_visible(&node);

    if visible {
        let indentation_columns = depth * 2;
        write!(fmt, "{:indentation_columns$}", "")?;

        if let Some(field_name) = cursor.field_name() {
            write!(fmt, "{}: ", field_name)?;
        }

        write!(fmt, "({}", node.kind())?;
    } else {
        write!(fmt, " \"{}\"", format_anonymous_node_kind(node.kind()))?;
    }

    // Handle children.
    if cursor.goto_first_child() {
        loop {
            if node_is_visible(&cursor.node()) {
                fmt.write_char('\n')?;
            }

            pretty_print_tree_impl(fmt, cursor, depth + 1)?;

            if !cursor.goto_next_sibling() {
                break;
            }
        }

        let moved = cursor.goto_parent();
        // The parent of the first child must exist, and must be `node`.
        debug_assert!(moved);
        debug_assert!(cursor.node() == node);
    }

    if visible {
        fmt.write_char(')')?;
    }

    Ok(())
}

/// Finds the child of `node` which contains the given byte range.
pub fn child_for_byte_range<'a>(node: &Node<'a>, range: ops::Range<u32>) -> Option<Node<'a>> {
    for child in node.children() {
        let child_range = child.byte_range();
        if range.start >= child_range.start && range.end <= child_range.end {
            return Some(child);
        }
    }

    None
}

#[derive(Debug)]
pub struct RainbowQuery {
    pub query: Query,
    /// The patterns in the query which are marked with `(#set! rainbow.include-children)`.
    pub include_children_patterns: HashSet<Pattern>,
    pub scope_capture: Option<Capture>,
    pub bracket_capture: Option<Capture>,
}

impl RainbowQuery {
    fn new(
        grammar: Grammar,
        source: &str,
        path: impl AsRef<Path>,
    ) -> Result<Self, tree_sitter::query::ParseError> {
        let mut include_children_patterns = HashSet::default();
        let query = Query::new(
            grammar,
            source,
            path,
            |pattern, predicate| match predicate {
                UserPredicate::SetProperty {
                    key: "rainbow.include-children",
                    val: None,
                } => {
                    include_children_patterns.insert(pattern);
                    Ok(())
                }
                _ => Err(format!("unsupported predicate {predicate}").into()),
            },
        )?;

        Ok(Self {
            include_children_patterns,
            scope_capture: query.get_capture("rainbow.scope"),
            bracket_capture: query.get_capture("rainbow.bracket"),
            query,
        })
    }
}
