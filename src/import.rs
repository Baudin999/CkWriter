use crate::book::entity::{slugify, Entities, Entity, EntityKind};
use anyhow::Result;
use std::path::Path;

/// Parse `Info/Characters/Personae.txt` (or whatever lives there) and write
/// per-character JSON files. Idempotent — never overwrites existing JSON.
/// Returns the number of new entities written.
pub fn import_personae(root: &Path) -> Result<usize> {
    let personae = root.join("Info/Characters/Personae.txt");
    let text = std::fs::read_to_string(&personae)?;
    let mut existing = Entities::load(root);
    let mut written = 0usize;

    let mut current_section = String::from("Characters");
    for raw_line in text.lines() {
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }
        if line.ends_with(':') {
            current_section = line.trim_end_matches(':').to_string();
            continue;
        }
        if line.starts_with("WORKING AT") || line.eq_ignore_ascii_case("Characters of interest") {
            continue;
        }

        // Format: `Name (paren_notes)` or just `Name`. Long descriptive paragraphs land
        // as long lines starting with a capitalized name; we still take the first
        // word-run as the name and the rest as free_text.
        let (name, rest) = split_name_rest(line);
        if name.is_empty() {
            continue;
        }
        let id = slugify(&name);
        if existing.get(&id).is_some() {
            continue;
        }
        let mut e = Entity::new(EntityKind::Character, &id, &name);
        e.role = current_section.clone();
        if !rest.is_empty() {
            e.free_text = rest.to_string();
            // Pull aliases out of "(Foo, Bar)" notation if at start.
            if let Some(aliases) = pull_aliases(rest) {
                e.aliases = aliases;
            }
        }
        e.tags
            .push(current_section.to_lowercase().replace(' ', "-"));
        existing.save(root, e)?;
        written += 1;
    }
    Ok(written)
}

fn split_name_rest(line: &str) -> (String, &str) {
    // Take everything up to the first '(', '-', '—', or end as the name.
    // Heuristic: a "name" is at most 4 capitalized tokens.
    let mut name_end = 0usize;
    let mut tokens = 0usize;
    for (i, ch) in line.char_indices() {
        if ch == '(' || ch == '-' || ch == '—' || ch == ',' {
            name_end = i;
            break;
        }
        if ch == ' ' {
            tokens += 1;
            if tokens >= 4 {
                name_end = i;
                break;
            }
        }
        if !ch.is_alphabetic() && ch != ' ' && ch != '\'' {
            name_end = i;
            break;
        }
        name_end = i + ch.len_utf8();
    }
    let (name, rest) = line.split_at(name_end);
    let rest = rest.trim_start_matches([' ', ',', '-', '—']);
    (name.trim().to_string(), rest.trim())
}

fn pull_aliases(rest: &str) -> Option<Vec<String>> {
    if !rest.starts_with('(') {
        return None;
    }
    let end = rest.find(')')?;
    let inside = &rest[1..end];
    let aliases: Vec<String> = inside
        .split([',', ';'])
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty() && s.chars().next().is_some_and(|c| c.is_uppercase()))
        .collect();
    if aliases.is_empty() {
        None
    } else {
        Some(aliases)
    }
}

/// Quick scan for proper-noun "names" listed one-per-line in the simpler
/// Info/Characters/*.tex files. Best-effort.
#[allow(dead_code)]
pub fn import_locations_seed(root: &Path, names: &[&str]) -> Result<usize> {
    let mut existing = Entities::load(root);
    let mut written = 0usize;
    for n in names {
        let id = slugify(n);
        if existing.get(&id).is_some() {
            continue;
        }
        let e = Entity::new(EntityKind::Location, &id, *n);
        existing.save(root, e)?;
        written += 1;
    }
    Ok(written)
}
