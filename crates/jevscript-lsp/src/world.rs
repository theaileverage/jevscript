//! The server's view of the workspace: open buffers, files on disk, and the
//! modules that connect them.
//!
//! An open buffer is authoritative for its own file: navigation reads it in
//! preference to the disk copy, and compilation links against it
//! ([`World::linking_resolver`]), so go-to-definition and diagnostics both see
//! a library as it is being edited. Everything else is read from disk on
//! demand and cached against its modification time.
//!
//! Module resolution is the compiler's: [`Resolver::resolve`] with the
//! importing file's path, the `JEVSCRIPT_PATH` roots and any roots the client
//! configured (spec section 3.9).

use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::str::FromStr;
use std::time::SystemTime;

use lsp_types::{Location, TextDocumentContentChangeEvent, Uri};

use jevscript_compiler::Resolver;
use jevscript_compiler::link::{PRELUDE_ALIAS, PRELUDE_SOURCE};

use crate::index::{Index, SymbolId, SymbolKind, Target};
use crate::line_index::{Encoding, LineIndex};
use crate::recover::{self, Recovered};

/// The URI the prelude is known by. It is not a file; hover shows its
/// source instead of navigating to it.
pub const PRELUDE_URI: &str = "jevscript-prelude:///std.jev";

/// One analyzed source text.
#[derive(Debug)]
pub struct SourceFile {
    /// Where it is.
    pub uri: Uri,
    /// Its path, when it is a file.
    pub path: Option<PathBuf>,
    /// The text.
    pub text: String,
    /// Line starts, for positions.
    pub lines: LineIndex,
    /// The recovering parse (see [`crate::recover`]).
    pub recovered: Recovered,
    /// The name index of whatever parsed.
    pub index: Option<Index>,
}

impl SourceFile {
    /// Parse and index `text`.
    pub fn new(uri: Uri, path: Option<PathBuf>, text: String, encoding: Encoding) -> Self {
        let lines = LineIndex::new(&text, encoding);
        let recovered = recover::parse(&text);
        let index = recovered
            .program
            .as_ref()
            .map(|program| Index::build(program, &text));
        Self {
            uri,
            path,
            text,
            lines,
            recovered,
            index,
        }
    }

    /// Whether this is the prelude.
    pub fn is_prelude(&self) -> bool {
        self.uri.as_str() == PRELUDE_URI
    }

    /// The editor location of a symbol of this file.
    pub fn location(&self, symbol: SymbolId) -> Option<Location> {
        let index = self.index.as_ref()?;
        let symbol = index.symbols.get(symbol)?;
        Some(Location::new(
            self.uri.clone(),
            self.lines.range(&self.text, symbol.selection),
        ))
    }

    /// A key that is equal for the same file however it was reached.
    pub fn key(&self) -> String {
        match &self.path {
            Some(path) => canonical(path).display().to_string(),
            None => self.uri.as_str().to_string(),
        }
    }
}

/// An open buffer.
#[derive(Debug)]
struct Document {
    uri: Uri,
    path: Option<PathBuf>,
    text: String,
    version: i32,
    analyzed: Option<Rc<SourceFile>>,
}

/// A file read from disk, with the modification time it was read at.
type DiskEntry = (Option<SystemTime>, Rc<SourceFile>);

/// A symbol somewhere in the workspace.
#[derive(Debug, Clone)]
pub struct Resolved {
    /// The file that declares it.
    pub file: Rc<SourceFile>,
    /// The declaration in that file's index.
    pub symbol: SymbolId,
}

impl Resolved {
    /// The declaration.
    pub fn symbol(&self) -> &crate::index::Symbol {
        &self
            .file
            .index
            .as_ref()
            .expect("resolved in an indexed file")
            .symbols[self.symbol]
    }

    /// Whether two resolutions name the same declaration.
    pub fn same(&self, other: &Resolved) -> bool {
        self.symbol == other.symbol && self.file.key() == other.file.key()
    }
}

/// Everything the server knows.
pub struct World {
    encoding: Encoding,
    documents: HashMap<String, Document>,
    disk: RefCell<HashMap<PathBuf, DiskEntry>>,
    prelude: Rc<SourceFile>,
    /// The workspace folders, searched for references.
    pub roots: Vec<PathBuf>,
    /// Where non-relative `use` paths resolve, with every open buffer in its
    /// overlay (kept in step by `open`, `change` and `close`).
    resolver: Resolver,
}

impl World {
    /// An empty world counting columns in `encoding`.
    pub fn new(encoding: Encoding, roots: Vec<PathBuf>, resolver: Resolver) -> Self {
        let prelude = Rc::new(SourceFile::new(
            Uri::from_str(PRELUDE_URI).expect("the prelude URI parses"),
            None,
            PRELUDE_SOURCE.to_string(),
            encoding,
        ));
        Self {
            encoding,
            documents: HashMap::new(),
            disk: RefCell::new(HashMap::new()),
            prelude,
            roots,
            resolver,
        }
    }

    /// The negotiated column encoding.
    pub fn encoding(&self) -> Encoding {
        self.encoding
    }

    /// The prelude as a file (spec section 8.1).
    pub fn prelude(&self) -> Rc<SourceFile> {
        self.prelude.clone()
    }

    /// `textDocument/didOpen`.
    pub fn open(&mut self, uri: Uri, text: String, version: i32) {
        let path = path_from_uri(&uri);
        if let Some(path) = &path {
            self.resolver.overlay.insert(canonical(path), text.clone());
        }
        self.documents.insert(
            uri.as_str().to_string(),
            Document {
                uri,
                path,
                text,
                version,
                analyzed: None,
            },
        );
    }

    /// `textDocument/didChange`, applying each change in order.
    pub fn change(
        &mut self,
        uri: &Uri,
        changes: Vec<TextDocumentContentChangeEvent>,
        version: i32,
    ) {
        let encoding = self.encoding;
        let Some(document) = self.documents.get_mut(uri.as_str()) else {
            return;
        };
        for change in changes {
            match change.range {
                None => document.text = change.text,
                Some(range) => {
                    let lines = LineIndex::new(&document.text, encoding);
                    let start = lines.offset(&document.text, range.start);
                    let end = lines.offset(&document.text, range.end).max(start);
                    document.text.replace_range(start..end, &change.text);
                }
            }
        }
        document.version = version;
        document.analyzed = None;
        if let Some(path) = &document.path {
            self.resolver
                .overlay
                .insert(canonical(path), document.text.clone());
        }
    }

    /// `textDocument/didClose`.
    pub fn close(&mut self, uri: &Uri) {
        if let Some(document) = self.documents.remove(uri.as_str())
            && let Some(path) = &document.path
        {
            self.resolver.overlay.remove(&canonical(path));
        }
    }

    /// Forget every file read from disk, after a save or a watched change.
    pub fn invalidate_disk(&mut self) {
        self.disk.borrow_mut().clear();
    }

    /// The URIs of every open buffer.
    pub fn open_uris(&self) -> Vec<Uri> {
        self.documents.values().map(|d| d.uri.clone()).collect()
    }

    /// The text of an open buffer.
    pub fn text(&self, uri: &Uri) -> Option<&str> {
        self.documents.get(uri.as_str()).map(|d| d.text.as_str())
    }

    /// The version of an open buffer.
    pub fn version(&self, uri: &Uri) -> Option<i32> {
        self.documents.get(uri.as_str()).map(|d| d.version)
    }

    /// The analysis of an open buffer, computed on first use after a change.
    pub fn file(&mut self, uri: &Uri) -> Option<Rc<SourceFile>> {
        if uri.as_str() == PRELUDE_URI {
            return Some(self.prelude());
        }
        let encoding = self.encoding;
        let document = self.documents.get_mut(uri.as_str())?;
        if document.analyzed.is_none() {
            document.analyzed = Some(Rc::new(SourceFile::new(
                document.uri.clone(),
                document.path.clone(),
                document.text.clone(),
                encoding,
            )));
        }
        document.analyzed.clone()
    }

    /// The analysis of the file at `path`: its open buffer if it has one,
    /// otherwise the disk copy.
    pub fn file_at(&mut self, path: &Path) -> Option<Rc<SourceFile>> {
        let wanted = canonical(path);
        let open = self
            .documents
            .values()
            .find(|d| d.path.as_deref().map(canonical).as_ref() == Some(&wanted))
            .map(|d| d.uri.clone());
        if let Some(uri) = open {
            return self.file(&uri);
        }
        let modified = std::fs::metadata(&wanted).and_then(|m| m.modified()).ok();
        if let Some((stamp, file)) = self.disk.borrow().get(&wanted)
            && *stamp == modified
        {
            return Some(file.clone());
        }
        let text = std::fs::read_to_string(&wanted).ok()?;
        let file = Rc::new(SourceFile::new(
            uri_from_path(&wanted),
            Some(wanted.clone()),
            text,
            self.encoding,
        ));
        self.disk
            .borrow_mut()
            .insert(wanted, (modified, file.clone()));
        Some(file)
    }

    /// The file a `use` path of `from` names.
    pub fn use_target(&mut self, from: &SourceFile, written: &str) -> Option<Rc<SourceFile>> {
        let base = from
            .path
            .clone()
            .unwrap_or_else(|| PathBuf::from("./<source>"));
        let path = self.resolver.resolve(&base, written).ok()?;
        self.file_at(&path)
    }

    /// The module an alias path names from `from`: `["harness"]`, or
    /// `["a", "b"]` through a library's own `use`. `std` is the prelude.
    pub fn module(&mut self, from: &Rc<SourceFile>, aliases: &[String]) -> Option<Rc<SourceFile>> {
        let Some((first, rest)) = aliases.split_first() else {
            return Some(from.clone());
        };
        if first == PRELUDE_ALIAS
            && !from
                .index
                .as_ref()
                .is_some_and(|i| i.uses.iter().any(|u| u.alias == *first))
        {
            return rest.is_empty().then(|| self.prelude());
        }
        let written = from
            .index
            .as_ref()?
            .uses
            .iter()
            .find(|u| u.alias == *first)?
            .path
            .clone()?;
        let next = self.use_target(from, &written)?;
        self.module(&next, rest)
    }

    /// A unit named by the dotted path written in `from`.
    pub fn unit(&mut self, from: &Rc<SourceFile>, path: &[String]) -> Option<Resolved> {
        let (name, aliases) = path.split_last()?;
        let module = if aliases.is_empty() {
            let own = from.index.as_ref().and_then(|i| i.unit(name)).is_some();
            if own { from.clone() } else { self.prelude() }
        } else {
            self.module(from, aliases)?
        };
        let (symbol, _) = module.index.as_ref()?.unit(name)?;
        Some(Resolved {
            file: module,
            symbol,
        })
    }

    /// What an occurrence's target refers to, wherever it is declared.
    pub fn resolve(&mut self, from: &Rc<SourceFile>, target: &Target) -> Option<Resolved> {
        let here = |symbol| {
            Some(Resolved {
                file: from.clone(),
                symbol,
            })
        };
        match target {
            Target::Symbol(id) => here(*id),
            Target::Unit(path) => self.unit(from, path),
            Target::Module(path) => {
                let (alias, parents) = path.split_last()?;
                let module = self.module(from, parents)?;
                let symbol = module
                    .index
                    .as_ref()?
                    .uses
                    .iter()
                    .find(|u| u.alias == *alias)?
                    .symbol;
                Some(Resolved {
                    file: module,
                    symbol,
                })
            }
            Target::Member { unit, member } => {
                let unit = self.unit(from, unit)?;
                let symbol = unit.file.index.as_ref()?.child(unit.symbol, member)?;
                Some(Resolved {
                    file: unit.file,
                    symbol,
                })
            }
            Target::MemberLabel {
                unit,
                member,
                label,
            } => {
                let unit = self.unit(from, unit)?;
                let index = unit.file.index.as_ref()?;
                let member = index.child(unit.symbol, member)?;
                let symbol = index
                    .labels_of(member)
                    .into_iter()
                    .find(|id| index.symbols[*id].name == *label)?;
                Some(Resolved {
                    file: unit.file,
                    symbol,
                })
            }
            Target::UnitParam { unit, param } => {
                let unit = self.unit(from, unit)?;
                let index = unit.file.index.as_ref()?;
                let symbol = index.child(unit.symbol, param)?;
                (index.symbols[symbol].kind == SymbolKind::Parameter).then_some(Resolved {
                    file: unit.file,
                    symbol,
                })
            }
            Target::LibraryCapability { alias, name } => {
                let module = self.module(from, std::slice::from_ref(alias))?;
                let symbol = *module.index.as_ref()?.capabilities.get(name)?;
                Some(Resolved {
                    file: module,
                    symbol,
                })
            }
            Target::UsePath(written) => {
                let file = self.use_target(from, written)?;
                file.index.as_ref()?;
                Some(Resolved { file, symbol: 0 })
            }
            Target::Label(name) => {
                let index = from.index.as_ref()?;
                let symbol = index
                    .all_labels()
                    .into_iter()
                    .find(|id| index.symbols[*id].name == *name)?;
                here(symbol)
            }
            Target::Verb { .. } | Target::HandleVerb(_) | Target::Builtin(_) | Target::None => None,
        }
    }

    /// The resolver to compile with: the configured one, with every open
    /// buffer overlaid on its file, so an importer is linked against a
    /// library as it is being edited, saved or not, and even before it exists
    /// on disk. [`World::use_target`] resolves through the same overlay, so
    /// navigation and dependency tracking see the same modules the compiler
    /// links.
    pub fn linking_resolver(&self) -> &Resolver {
        &self.resolver
    }

    /// The keys of every module `file` imports, directly or through another
    /// module, as [`SourceFile::key`] spells them.
    pub fn imports(&mut self, file: &Rc<SourceFile>) -> BTreeSet<String> {
        let mut seen = BTreeSet::new();
        let mut stack = vec![file.clone()];
        while let Some(next) = stack.pop() {
            let written: Vec<String> = next
                .index
                .as_ref()
                .map(|i| i.uses.iter().filter_map(|u| u.path.clone()).collect())
                .unwrap_or_default();
            for path in written {
                if let Some(module) = self.use_target(&next, &path)
                    && seen.insert(module.key())
                {
                    stack.push(module);
                }
            }
        }
        seen
    }

    /// The open buffers other than `uri` that import `uri`'s file, directly
    /// or not: the ones whose diagnostics change when it does.
    pub fn dependents(&mut self, uri: &Uri) -> Vec<Uri> {
        let Some(key) = self.file(uri).map(|f| f.key()) else {
            return Vec::new();
        };
        let mut found = Vec::new();
        for other in self.open_uris() {
            if other.as_str() == uri.as_str() {
                continue;
            }
            if let Some(file) = self.file(&other)
                && self.imports(&file).contains(&key)
            {
                found.push(other);
            }
        }
        found
    }

    /// Every `.jev` file a references search should read: the open buffers,
    /// then the workspace folders' files in path order, at most 2,000 of them
    /// so a huge folder cannot stall the server. A reference in a file past
    /// the bound is not found unless that file is open.
    pub fn search_set(&mut self) -> Vec<Rc<SourceFile>> {
        const LIMIT: usize = 2000;
        let mut seen = BTreeSet::new();
        let mut files = Vec::new();
        for uri in self.open_uris() {
            if let Some(file) = self.file(&uri) {
                seen.insert(file.key());
                files.push(file);
            }
        }
        let mut paths = Vec::new();
        for root in self.roots.clone() {
            collect_jev(&root, &mut paths, LIMIT);
        }
        for path in paths {
            if seen.contains(&canonical(&path).display().to_string()) {
                continue;
            }
            if let Some(file) = self.file_at(&path) {
                seen.insert(file.key());
                files.push(file);
            }
        }
        files
    }
}

fn collect_jev(dir: &Path, out: &mut Vec<PathBuf>, limit: usize) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        if out.len() >= limit {
            return;
        }
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            if !name.starts_with('.') && name != "target" && name != "node_modules" {
                collect_jev(&path, out, limit);
            }
        } else if path.extension().is_some_and(|e| e == "jev") {
            out.push(path);
        }
    }
}

/// The compiler's spelling of a path, so the server's keys and the overlay
/// the linker reads agree (see [`jevscript_compiler::link::canonical`]).
pub fn canonical(path: &Path) -> PathBuf {
    jevscript_compiler::link::canonical(path)
}

/// The path a `file:` URI names.
pub fn path_from_uri(uri: &Uri) -> Option<PathBuf> {
    let rest = uri.as_str().strip_prefix("file://")?;
    // Skip an authority: `file://host/path` is not a local path.
    let path = if rest.starts_with('/') {
        rest
    } else {
        let slash = rest.find('/')?;
        if &rest[..slash] != "localhost" {
            return None;
        }
        &rest[slash..]
    };
    let decoded = percent_decode(path.split(['?', '#']).next().unwrap_or(path))?;
    #[cfg(windows)]
    {
        // `/C:/x` is `C:/x`.
        let bytes = decoded.as_bytes();
        if bytes.len() >= 3 && bytes[0] == b'/' && bytes[2] == b':' {
            return Some(PathBuf::from(&decoded[1..]));
        }
    }
    Some(PathBuf::from(decoded))
}

/// The `file:` URI of an absolute path.
pub fn uri_from_path(path: &Path) -> Uri {
    let text = path.display().to_string().replace('\\', "/");
    let mut encoded = String::from("file://");
    if !text.starts_with('/') {
        encoded.push('/');
    }
    for byte in text.bytes() {
        let keep = byte.is_ascii_alphanumeric() || b"/-._~:".contains(&byte);
        if keep {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    Uri::from_str(&encoded).unwrap_or_else(|_| Uri::from_str("file:///").expect("a valid URI"))
}

fn percent_decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = text.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn examples() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples")
    }

    #[test]
    fn uris_and_paths_round_trip() {
        let path = PathBuf::from("/tmp/a dir/é.jev");
        let uri = uri_from_path(&path);
        assert_eq!(uri.as_str(), "file:///tmp/a%20dir/%C3%A9.jev");
        assert_eq!(path_from_uri(&uri), Some(path));
        let host = Uri::from_str("file://server/share/x.jev").expect("parses");
        assert_eq!(path_from_uri(&host), None);
        let untitled = Uri::from_str("untitled:Untitled-1").expect("parses");
        assert_eq!(path_from_uri(&untitled), None);
    }

    #[test]
    fn a_qualified_unit_resolves_into_the_library() {
        // Spec 3.9: `harness.watch` is the library's `watch`.
        let mut world = World::new(Encoding::Utf16, vec![], Resolver::default());
        let root = world
            .file_at(&examples().join("fix_issue.jev"))
            .expect("the example reads");
        let resolved = world
            .unit(&root, &["harness".to_string(), "watch".to_string()])
            .expect("resolves");
        assert!(resolved.file.key().ends_with("agent_loop.jev"));
        assert_eq!(resolved.symbol().name, "watch");
    }

    #[test]
    fn an_open_buffer_wins_over_the_disk() {
        let mut world = World::new(Encoding::Utf16, vec![], Resolver::default());
        let path = examples().join("lib/agent_loop.jev");
        let uri = uri_from_path(&canonical(&path));
        world.open(
            uri.clone(),
            "program agent_loop\n\ndef edited(x):\n  return x\n".to_string(),
            1,
        );
        let file = world.file_at(&path).expect("reads");
        assert!(
            file.index
                .as_ref()
                .expect("indexed")
                .unit("edited")
                .is_some()
        );
    }

    #[test]
    fn incremental_changes_apply_in_order() {
        use lsp_types::{Position, Range};
        let mut world = World::new(Encoding::Utf16, vec![], Resolver::default());
        let uri = Uri::from_str("file:///x.jev").expect("parses");
        world.open(uri.clone(), "program a\n".to_string(), 1);
        world.change(
            &uri,
            vec![
                TextDocumentContentChangeEvent {
                    range: Some(Range::new(Position::new(0, 8), Position::new(0, 9))),
                    range_length: None,
                    text: "demo".to_string(),
                },
                TextDocumentContentChangeEvent {
                    range: Some(Range::new(Position::new(1, 0), Position::new(1, 0))),
                    range_length: None,
                    text: "# é\n".to_string(),
                },
            ],
            2,
        );
        assert_eq!(world.text(&uri), Some("program demo\n# é\n"));
        assert_eq!(world.version(&uri), Some(2));
    }

    #[test]
    fn an_open_library_is_overlaid_and_its_importers_depend_on_it() {
        let mut world = World::new(Encoding::Utf16, vec![], Resolver::default());
        let library = canonical(&examples().join("lib/agent_loop.jev"));
        let library_uri = uri_from_path(&library);
        let importer = canonical(&examples().join("fix_issue.jev"));
        let importer_uri = uri_from_path(&importer);
        let other_uri = uri_from_path(&canonical(&examples().join("review_loop.jev")));
        for (uri, path) in [
            (&library_uri, &library),
            (&importer_uri, &importer),
            (&other_uri, &canonical(&examples().join("review_loop.jev"))),
        ] {
            let text = std::fs::read_to_string(path).expect("reads");
            world.open(uri.clone(), text, 1);
        }
        assert_eq!(
            world
                .linking_resolver()
                .overlay
                .get(&library)
                .map(String::len),
            world.text(&library_uri).map(str::len)
        );
        let dependents: Vec<String> = world
            .dependents(&library_uri)
            .iter()
            .map(|u| u.as_str().to_string())
            .collect();
        assert_eq!(dependents, [importer_uri.as_str().to_string()]);
        assert!(world.dependents(&importer_uri).is_empty());
    }

    #[test]
    fn the_prelude_is_a_module() {
        let mut world = World::new(Encoding::Utf16, vec![], Resolver::default());
        let root = world
            .file_at(&examples().join("fix_issue_inline.jev"))
            .expect("reads");
        let stuck = world
            .unit(&root, &["stuck".to_string()])
            .expect("the prelude's stuck");
        assert!(stuck.file.is_prelude());
        let qualified = world
            .unit(&root, &["std".to_string(), "repeats".to_string()])
            .expect("std.repeats");
        assert!(qualified.file.is_prelude());
    }
}
