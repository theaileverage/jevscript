//! Plain-terminal diagnostic presentation (spec sections 12 and 15.8).

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::OnceLock;

use jevscript_ir::Ir;
use jevscript_runtime::{Pause, RuntimeError};
use jevscript_syntax::{Diagnostic, Severity, Span};
use unicode_width::UnicodeWidthStr;

/// The stable published reference used by the editor as well (spec section 12).
const REFERENCE: &str =
    "https://github.com/theaileverage/jevscript/blob/main/docs/error-reference.md";
const GUIDE: &str = include_str!("../../../docs/error-reference.md");

/// Source consumed from `-`, retained for the run's later error pauses.
pub static STDIN_SOURCE: OnceLock<String> = OnceLock::new();

/// A pause carries a span but no module identity (spec section 10.2). With
/// imported code, attributing that span to the root file could show an
/// unrelated line, so leave the file unknown (spec section 12).
pub fn runtime_file<'a>(ir: &Ir, root: Option<&'a str>) -> Option<&'a str> {
    if ir
        .modules
        .iter()
        .any(|module| !module.alias.is_empty() && module.alias != "std")
    {
        None
    } else {
        root
    }
}

/// Print every compile diagnostic with its own source file.
pub fn report(file: &Path, diagnostics: &[Diagnostic]) {
    let mut sources = BTreeMap::new();
    for diagnostic in diagnostics {
        let path = diagnostic
            .file
            .as_deref()
            .unwrap_or_else(|| file.to_str().unwrap_or("<source>"));
        let severity = match diagnostic.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
        };
        render(
            &mut sources,
            path,
            diagnostic.span,
            severity,
            diagnostic.code.as_str(),
            &diagnostic.message,
        );
        for related in &diagnostic.related {
            let related_path = related.file.as_deref().unwrap_or(path);
            eprintln!(
                "  related: {}:{}:{}: {}",
                related_path, related.span.start.line, related.span.start.column, related.message
            );
            excerpt(&mut sources, related_path, related.span);
        }
    }
}

/// Print an error pause without changing its JSON representation on stdout.
pub fn pause(file: Option<&str>, pause: &Pause) {
    if let Pause::Error {
        common,
        code,
        message,
        ..
    } = pause
    {
        let mut sources = BTreeMap::new();
        render(
            &mut sources,
            file.unwrap_or("<source unavailable>"),
            common.source,
            "error",
            code.as_str(),
            message,
        );
    }
}

/// Print a runtime error raised before a pause can be made.
pub fn runtime_error(file: Option<&str>, error: &RuntimeError) {
    let mut sources = BTreeMap::new();
    render(
        &mut sources,
        file.unwrap_or("<source unavailable>"),
        error.span,
        "error",
        error.code.as_str(),
        &error.message,
    );
}

/// Present a host-side check with no structured source span.
pub fn unlocated(file: &Path, code: &str, message: &str) {
    let mut sources = BTreeMap::new();
    render(
        &mut sources,
        &file.display().to_string(),
        Span::default(),
        "error",
        code,
        message,
    );
}

fn render(
    sources: &mut BTreeMap<String, Option<String>>,
    file: &str,
    span: Span,
    severity: &str,
    code: &str,
    message: &str,
) {
    let marker = if severity == "warning" {
        "warning: "
    } else {
        ""
    };
    eprintln!(
        "{file}:{}:{}: {marker}{code}: {message}",
        span.start.line, span.start.column
    );
    excerpt(sources, file, span);
    if let Some((cause, repair)) = reference_entry(code) {
        eprintln!("  cause: {cause}");
        if !repair.is_empty() {
            eprintln!("  help: {repair}");
        }
    } else {
        eprintln!("  cause: {message}");
    }
    eprintln!("  docs: {REFERENCE}#{code}");
}

fn excerpt(sources: &mut BTreeMap<String, Option<String>>, file: &str, span: Span) {
    if span.start.line == 0 {
        eprintln!("  | source location unavailable");
        return;
    }
    if file == "<source unavailable>" {
        eprintln!("  | source file unavailable for linked program");
        return;
    }
    let source = sources.entry(file.to_string()).or_insert_with(|| {
        if file == "<stdin>" {
            STDIN_SOURCE.get().cloned()
        } else {
            std::fs::read_to_string(file).ok()
        }
    });
    let Some(source) = source else {
        eprintln!("  | source excerpt unavailable");
        return;
    };
    let Some(line) = source.lines().nth((span.start.line - 1) as usize) else {
        eprintln!("  | source excerpt unavailable");
        return;
    };
    let line = line.trim_end_matches('\r');
    let start = (span.start.column as usize).min(line.chars().count());
    let end = if span.end.line == span.start.line {
        (span.end.column as usize)
            .max(start + 1)
            .min(line.chars().count().max(start + 1))
    } else {
        line.chars().count().max(start + 1)
    };
    let prefix = line
        .chars()
        .take(start)
        .collect::<String>()
        .replace('\t', "    ");
    let marked = line
        .chars()
        .skip(start)
        .take(end - start)
        .collect::<String>()
        .replace('\t', "    ");
    let column = UnicodeWidthStr::width(prefix.as_str());
    let width = UnicodeWidthStr::width(marked.as_str()).max(1);
    let gutter = span.start.line.to_string().len();
    eprintln!(
        "  {:>gutter$} | {}",
        span.start.line,
        line.replace('\t', "    ")
    );
    eprintln!(
        "  {:gutter$} | {}{}",
        "",
        " ".repeat(column),
        "^".repeat(width)
    );
}

fn reference_entry(code: &str) -> Option<(&'static str, &'static str)> {
    let marker = format!("<a id=\"{code}\"></a>");
    let line = GUIDE.lines().find(|line| line.contains(&marker))?;
    let columns: Vec<&str> = line.split('|').map(str::trim).collect();
    Some((columns.get(2)?.trim(), columns.get(3)?.trim()))
}

#[cfg(test)]
mod tests {
    use super::reference_entry;

    #[test]
    fn unknown_guidance_is_not_invented() {
        assert_eq!(reference_entry("future_code"), None);
    }
}
