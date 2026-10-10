//! `.stignore` validation by datastore kind. A PBS datastore is restorable only
//! with its chunk store, index files, blobs and owner files; ignoring any of them
//! leaves a replica of empty snapshot skeletons that looks healthy in Syncthing.
//!
//! Pattern semantics follow Syncthing's ignore docs: the first matching pattern
//! decides; `!` re-includes; `(?i)` folds case; `(?d)` marks deletable; prefixes
//! combine in any order. `*` and `?` stay within a path component, `**` crosses
//! components, `[...]` is a character class, `{a,b}` is alternation, and the
//! escape character (`\` unless set by a `#escape=X` line) makes the next
//! character literal. Unanchored patterns match at any depth, and a pattern that
//! matches a directory also covers its contents.

/// Representative PBS datastore paths that must replicate.
pub const PBS_REQUIRED: &[&str] = &[
    ".chunks/0a1b",
    ".chunks/0a1b/0a1b2c3d4e5f60718293a4b5c6d7e8f90a1b2c3d4e5f60718293a4b5c6d7e8f9",
    "vm/100/owner",
    "vm/100/2026-01-01T00:00:00Z/drive-scsi0.img.fidx",
    "vm/100/2026-01-01T00:00:00Z/index.json.blob",
    "ct/101/2026-01-01T00:00:00Z/root.pxar.didx",
    "ct/101/2026-01-01T00:00:00Z/catalog.pcat1.didx",
    "host/backup-host/2026-01-01T00:00:00Z/root.pxar.didx",
    "host/backup-host/2026-01-01T00:00:00Z/client.log.blob",
    "ns/site/vm/100/2026-01-01T00:00:00Z/drive-scsi0.img.fidx",
    "ns/site/ct/101/2026-01-01T00:00:00Z/root.pxar.didx",
    "ns/site/host/backup-host/2026-01-01T00:00:00Z/root.pxar.didx",
    "ns/site/ns/nested/vm/100/2026-01-01T00:00:00Z/index.json.blob",
];

use plugin_toolkit::prelude::*;

/// A required path that the ignore patterns exclude.
#[orca_struct]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Violation {
    pub path: String,
    pub pattern: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Pattern {
    raw: String,
    negate: bool,
    fold: bool,
    anchored: bool,
    /// Brace-expanded glob bodies.
    globs: Vec<Vec<Tok>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Tok {
    Lit(char),
    Any,
    Star,
    DoubleStar,
    Class {
        negated: bool,
        items: Vec<(char, char)>,
    },
}

/// Ignore violations for a PBS datastore folder, given its ignore lines.
/// `#include` lines are not followed; pass already-expanded lines.
pub fn pbs_violations(stignore: &str) -> Vec<Violation> {
    let patterns = parse(stignore);
    PBS_REQUIRED
        .iter()
        .filter_map(|path| {
            let hit = patterns.iter().find(|p| p.matches(path))?;
            (!hit.negate).then(|| Violation {
                path: path.to_string(),
                pattern: hit.raw.clone(),
            })
        })
        .collect()
}

fn parse(stignore: &str) -> Vec<Pattern> {
    let mut escape = Some('\\');
    let mut out = Vec::new();
    for line in stignore.lines().map(str::trim) {
        if line.is_empty() || line.starts_with("//") || line.starts_with("#include") {
            continue;
        }
        if let Some(rest) = line.strip_prefix("#escape") {
            escape = rest
                .trim_start()
                .strip_prefix('=')
                .and_then(|c| c.trim().chars().next());
            continue;
        }
        out.push(Pattern::new(line, escape));
    }
    out
}

impl Pattern {
    fn new(raw: &str, escape: Option<char>) -> Self {
        let (mut negate, mut fold) = (false, false);
        let mut body = raw;
        loop {
            if let (false, Some(r)) = (negate, body.strip_prefix('!')) {
                negate = true;
                body = r;
            } else if let (false, Some(r)) = (fold, body.strip_prefix("(?i)")) {
                fold = true;
                body = r;
            } else if let Some(r) = body.strip_prefix("(?d)") {
                body = r;
            } else {
                break;
            }
        }
        let anchored = body.starts_with('/');
        let body = body.trim_start_matches('/');
        let body = body.strip_suffix('/').unwrap_or(body);
        let globs = expand_braces(body, escape)
            .iter()
            .map(|g| {
                let g = if fold { g.to_lowercase() } else { g.clone() };
                compile(&g, escape)
            })
            .collect();
        Pattern {
            raw: raw.to_string(),
            negate,
            fold,
            anchored,
            globs,
        }
    }

    fn matches(&self, path: &str) -> bool {
        let path = if self.fold {
            path.to_lowercase()
        } else {
            path.to_string()
        };
        let comps: Vec<&str> = path.split('/').collect();
        let starts = if self.anchored { 0..1 } else { 0..comps.len() };
        starts.into_iter().any(|s| {
            (s + 1..=comps.len()).any(|e| {
                let sub: Vec<char> = comps[s..e].join("/").chars().collect();
                self.globs.iter().any(|g| glob(g, &sub))
            })
        })
    }
}

/// `{a,b}` alternation, nested and escaped braces respected.
fn expand_braces(s: &str, escape: Option<char>) -> Vec<String> {
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    let mut depth = 0;
    let mut open = None;
    let mut commas = Vec::new();
    while i < chars.len() {
        let c = chars[i];
        if Some(c) == escape {
            i += 2;
            continue;
        }
        match c {
            '{' => {
                if depth == 0 {
                    open = Some(i);
                    commas.clear();
                }
                depth += 1;
            }
            ',' if depth == 1 => commas.push(i),
            '}' if depth > 0 => {
                depth -= 1;
                if depth == 0 {
                    let o = open.unwrap_or(0);
                    let pre: String = chars[..o].iter().collect();
                    let post: String = chars[i + 1..].iter().collect();
                    let mut bounds = vec![o];
                    bounds.extend(&commas);
                    bounds.push(i);
                    let mut out = Vec::new();
                    for w in bounds.windows(2) {
                        let alt: String = chars[w[0] + 1..w[1]].iter().collect();
                        out.extend(
                            expand_braces(&format!("{alt}{post}"), escape)
                                .into_iter()
                                .map(|tail| format!("{pre}{tail}")),
                        );
                    }
                    return out;
                }
            }
            _ => {}
        }
        i += 1;
    }
    vec![s.to_string()]
}

fn compile(s: &str, escape: Option<char>) -> Vec<Tok> {
    let chars: Vec<char> = s.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if Some(c) == escape {
            if let Some(&n) = chars.get(i + 1) {
                out.push(Tok::Lit(n));
            }
            i += 2;
            continue;
        }
        match c {
            '*' if chars.get(i + 1) == Some(&'*') => {
                out.push(Tok::DoubleStar);
                i += 2;
                continue;
            }
            '*' => out.push(Tok::Star),
            '?' => out.push(Tok::Any),
            '[' => {
                if let Some((tok, next)) = class(&chars, i, escape) {
                    out.push(tok);
                    i = next;
                    continue;
                }
                out.push(Tok::Lit('['));
            }
            c => out.push(Tok::Lit(c)),
        }
        i += 1;
    }
    out
}

/// Parse `[...]` starting at `start`; `None` when unterminated (then literal).
fn class(chars: &[char], start: usize, escape: Option<char>) -> Option<(Tok, usize)> {
    let mut i = start + 1;
    let negated = matches!(chars.get(i), Some('!') | Some('^'));
    if negated {
        i += 1;
    }
    let mut items = Vec::new();
    let mut first = true;
    while i < chars.len() {
        let mut c = chars[i];
        if c == ']' && !first {
            return Some((Tok::Class { negated, items }, i + 1));
        }
        if Some(c) == escape {
            i += 1;
            c = *chars.get(i)?;
        }
        first = false;
        if chars.get(i + 1) == Some(&'-') && chars.get(i + 2).is_some_and(|&h| h != ']') {
            items.push((c, chars[i + 2]));
            i += 3;
        } else {
            items.push((c, c));
            i += 1;
        }
    }
    None
}

fn glob(p: &[Tok], s: &[char]) -> bool {
    match p.split_first() {
        None => s.is_empty(),
        Some((Tok::DoubleStar, rest)) => (0..=s.len()).any(|i| glob(rest, &s[i..])),
        Some((Tok::Star, rest)) => (0..=s.len())
            .take_while(|&i| i == 0 || s[i - 1] != '/')
            .any(|i| glob(rest, &s[i..])),
        Some((tok, rest)) => match s.split_first() {
            Some((&c, tail)) if c != '/' || matches!(tok, Tok::Lit('/')) => {
                let hit = match tok {
                    Tok::Lit(l) => *l == c,
                    Tok::Any => true,
                    Tok::Class { negated, items } => {
                        items.iter().any(|&(lo, hi)| lo <= c && c <= hi) != *negated
                    }
                    Tok::Star | Tok::DoubleStar => unreachable!(),
                };
                hit && glob(rest, tail)
            }
            _ => false,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pats(v: &[Violation]) -> Vec<&str> {
        let mut p: Vec<&str> = Vec::new();
        for v in v {
            if !p.contains(&v.pattern.as_str()) {
                p.push(&v.pattern);
            }
        }
        p
    }

    #[test]
    fn the_incident_stignore_is_rejected() {
        assert_eq!(
            pats(&pbs_violations(".chunks\n*.fidx\n*.didx\n")),
            vec![".chunks", "*.fidx", "*.didx"]
        );
    }

    #[test]
    fn safe_pbs_ignores_pass() {
        assert!(pbs_violations("// PBS\n.lock\n*.tmp_*\n(?d).DS_Store\n").is_empty());
    }

    #[test]
    fn negation_before_a_broad_ignore_re_includes() {
        assert!(pbs_violations("!.chunks\n!vm\n!ct\n!host\n!ns\n*\n").is_empty());
        assert_eq!(pbs_violations("*\n").len(), PBS_REQUIRED.len());
    }

    #[test]
    fn prefixes_parse_in_any_order_once() {
        assert!(pbs_violations("(?i)!.CHUNKS\n*\n").len() < PBS_REQUIRED.len());
        assert!(pbs_violations("(?d)(?i)!.CHUNKS\n.chunks\n").is_empty());
        // A second `!` is part of the body, so this is a plain ignore of `!x`.
        assert!(pbs_violations("!!x\n").is_empty());
    }

    #[test]
    fn anchored_and_case_folded_patterns() {
        assert_eq!(pbs_violations("/.chunks/\n").len(), 2);
        assert!(pbs_violations("/vm/.chunks\n").is_empty());
        assert!(!pbs_violations("(?i)*.FIDX\n").is_empty());
        assert!(pbs_violations("*.FIDX\n").is_empty());
    }

    #[test]
    fn double_star_crosses_directories() {
        assert!(!pbs_violations("vm/**/*.blob\n").is_empty());
        assert!(pbs_violations("vm/*.blob\n").is_empty());
    }

    #[test]
    fn classes_alternation_and_escapes() {
        assert!(!pbs_violations("*.[fd]idx\n").is_empty());
        assert!(pbs_violations("*.[!fd]idx\n").is_empty());
        assert!(!pbs_violations("*.{fidx,didx}\n").is_empty());
        assert!(!pbs_violations("{vm,ct}/*/owner\n").is_empty());
        assert!(pbs_violations("\\*.fidx\n").is_empty());
        assert!(pbs_violations("#escape=|\n|*.fidx\n").is_empty());
        assert!(pbs_violations("#escape=|\n\\.chunks\n").is_empty());
    }

    #[test]
    fn namespaces_and_host_groups_are_covered() {
        assert_eq!(pbs_violations("ns\n").len(), 4);
        // Two top-level host paths plus one namespaced.
        assert_eq!(pbs_violations("host\n").len(), 3);
    }
}
