//! The checkbox steps of any toolset's step file (Kiro's `tasks.md`,
//! OpenSpec's `changes/*/tasks.md`, a superpowers plan) and which repository
//! owns each. Nothing here touches the disk.

// Later tasks call all of this; until then nothing outside the tests does.
#![allow(dead_code)]

use super::{checkbox, list_item};

/// One `- [ ]` step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnedStep {
    /// 1-based place among every checkbox of the file outside code blocks,
    /// nested ones included. This is how a step is addressed.
    pub position: u32,
    /// The number as the file writes it (`2.1`, `Step 2`), or "".
    pub label: String,
    pub text: String,
    pub done: bool,
    /// The folders that own the step; empty means every repository.
    pub repos: Vec<String>,
}

pub fn owned_steps(text: &str, folders: &[String]) -> Vec<OwnedStep> {
    let mut found = Vec::new();
    for line in walk(text, folders) {
        let Some(step) = &line.step else { continue };
        let (label, text) = split_label(step.rest);
        found.push(OwnedStep {
            position: found.len() as u32 + 1,
            label: label.to_string(),
            text: text.to_string(),
            done: step.done,
            repos: line.owner().into_iter().collect(),
        });
    }
    found
}

/// `text` with the box at `position` ticked or unticked. None when the file
/// has no such box.
pub fn tick_at(text: &str, position: u32, done: bool) -> Option<String> {
    let mut place = 0;
    let mut out = String::with_capacity(text.len());
    let mut hit = false;
    for line in walk(text, &[]) {
        if line.step.is_some() {
            place += 1;
        }
        // The first `[` of a list item is its box.
        match line.body.find('[') {
            Some(at) if place == position && line.step.is_some() && !hit => {
                hit = true;
                out.push_str(&line.raw[..at]);
                out.push_str(if done { "[x]" } else { "[ ]" });
                out.push_str(&line.raw[at + 3..]);
            }
            _ => out.push_str(line.raw),
        }
    }
    hit.then_some(out)
}

/// What goes into `folder` of a step file: everything above the first
/// section owned by a repository, and then only the sections and steps that
/// belong to `folder` or to everyone.
pub fn own_part(text: &str, folder: &str, folders: &[String]) -> String {
    let lines = walk(text, folders);
    let mut out = String::with_capacity(text.len());
    // The indentation of a dropped step: what hangs under it goes too.
    let mut dropped: Option<usize> = None;
    for (i, line) in lines.iter().enumerate() {
        if let Some(indent) = dropped {
            let hangs = if line.body.trim().is_empty() {
                lines[i + 1..]
                    .iter()
                    .find(|l| !l.body.trim().is_empty())
                    .is_some_and(|l| l.indent > indent && !l.heading)
            } else {
                line.indent > indent && !line.heading
            };
            if hangs {
                continue;
            }
            dropped = None;
        }
        let mine = line.owner().is_none_or(|owner| owner.eq_ignore_ascii_case(folder));
        if mine {
            out.push_str(line.raw);
        } else if line.step.is_some() {
            dropped = Some(line.indent);
        }
    }
    out
}

/// One line of a step file and what it stands under.
struct Line<'a> {
    /// The line with its line ending.
    raw: &'a str,
    /// The line without its line ending.
    body: &'a str,
    indent: usize,
    heading: bool,
    step: Option<StepLine<'a>>,
    /// The folder of the nearest section above that names one.
    section: Option<String>,
}

struct StepLine<'a> {
    done: bool,
    /// The text after the box.
    rest: &'a str,
    /// The folder a `[tag]` on the step's own line names.
    tag: Option<String>,
}

impl Line<'_> {
    /// The folder that owns the line: the step's own tag, else its section.
    fn owner(&self) -> Option<String> {
        self.step.as_ref().and_then(|s| s.tag.clone()).or_else(|| self.section.clone())
    }
}

/// Every line, outside code blocks told apart as headings and steps.
fn walk<'a>(text: &'a str, folders: &[String]) -> Vec<Line<'a>> {
    let mut fence = false;
    // (heading level, folder it names), innermost last.
    let mut stack: Vec<(usize, Option<String>)> = Vec::new();
    let mut lines = Vec::new();
    for raw in text.split_inclusive('\n') {
        let body = raw.strip_suffix('\n').unwrap_or(raw);
        let body = body.strip_suffix('\r').unwrap_or(body);
        let trimmed = body.trim_start();
        let indent = body.len() - trimmed.len();
        let mut heading = false;
        let mut step = None;
        if trimmed.starts_with("```") {
            fence = !fence;
        } else if !fence {
            if let Some((level, title)) = heading_of(trimmed) {
                heading = true;
                while stack.last().is_some_and(|(l, _)| *l >= level) {
                    stack.pop();
                }
                stack.push((level, named(title, folders, true)));
            } else if let Some((done, rest)) = list_item(trimmed).and_then(checkbox) {
                step = Some(StepLine { done, rest, tag: named(rest, folders, false) });
            }
        }
        let section = stack.iter().rev().find_map(|(_, owner)| owner.clone());
        lines.push(Line { raw, body, indent, heading, step, section });
    }
    lines
}

/// `## Title` as (2, "Title").
fn heading_of(line: &str) -> Option<(usize, &str)> {
    let level = line.len() - line.trim_start_matches('#').len();
    let rest = line[level..].strip_prefix(' ').or_else(|| line[level..].is_empty().then_some(""))?;
    (1..=6).contains(&level).then(|| (level, rest.trim().trim_end_matches('#').trim()))
}

/// The folder `text` names: all of it (a heading may be just the folder), or
/// a `[tag]` in it. A link's text is not a tag.
fn named(text: &str, folders: &[String], whole: bool) -> Option<String> {
    let folder = |name: &str| folders.iter().find(|f| f.eq_ignore_ascii_case(name.trim())).cloned();
    if whole {
        if let Some(found) = folder(text) {
            return Some(found);
        }
    }
    let mut from = 0;
    while let Some(open) = text[from..].find('[') {
        let open = from + open;
        let Some(close) = text[open..].find(']') else { break };
        let close = open + close;
        if !text[close + 1..].starts_with('(') {
            if let Some(found) = folder(&text[open + 1..close]) {
                return Some(found);
            }
        }
        from = open + 1;
    }
    None
}

/// `2.1 Add x` as ("2.1", "Add x"), `**Step 2: Run it**` as ("Step 2",
/// "Run it"), and a step with no number as ("", its text). The stars of a
/// bold step go, both ends.
fn split_label(rest: &str) -> (&str, &str) {
    let body = rest.trim_start_matches('*');
    let bold = body.len() != rest.len();
    let word = ["step ", "task "]
        .iter()
        .find(|w| body.get(..w.len()).is_some_and(|b| b.eq_ignore_ascii_case(w)))
        .map_or(0, |w| w.len());
    let number = &body[word..];
    let digits = number.len() - number.trim_start_matches(|c: char| c.is_ascii_digit() || c == '.').len();
    let token = number[..digits].trim_end_matches('.');
    let boundary = number[token.len()..].chars().next().is_none_or(|c| !c.is_alphanumeric());
    if token.is_empty() || token.starts_with('.') || !boundary {
        return ("", unbold(body.trim(), bold));
    }
    let end = word + token.len();
    let text = body[end..].trim_start_matches(['.', ':', ')', '*', '-', '—', '–', ' ']).trim();
    (&body[..end], unbold(text, bold))
}

/// `text` without the stars that closed a bold step.
fn unbold(text: &str, bold: bool) -> &str {
    if bold {
        text.trim_end_matches('*').trim_end()
    } else {
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folders() -> Vec<String> {
        vec!["api".into(), "web".into()]
    }

    fn repos(text: &str) -> Vec<Vec<String>> {
        owned_steps(text, &folders()).into_iter().map(|s| s.repos).collect()
    }

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_heading_named_after_a_repository_owns_the_steps_under_it() {
        let steps = owned_steps("## api\n- [ ] a\n## web\n- [ ] b", &folders());
        assert_eq!(steps.len(), 2);
        assert_eq!(steps[0].repos, names(&["api"]));
        assert_eq!(steps[1].repos, names(&["web"]));
    }

    #[test]
    fn a_heading_names_a_repository_whatever_its_case() {
        assert_eq!(repos("## API\n- [ ] a"), vec![names(&["api"])]);
    }

    #[test]
    fn a_bracket_tag_on_a_heading_owns_its_steps() {
        let steps = owned_steps("## 2. API [api]\n- [ ] 2.1 x", &folders());
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].repos, names(&["api"]));
        assert_eq!(steps[0].label, "2.1");
        assert_eq!(steps[0].text, "x");
        assert!(!steps[0].done);
    }

    #[test]
    fn a_heading_that_only_contains_a_repository_name_owns_nothing() {
        assert_eq!(repos("## 2. API setup\n- [ ] x"), vec![Vec::<String>::new()]);
        assert_eq!(repos("## Task [other]\n- [ ] x"), vec![Vec::<String>::new()]);
    }

    #[test]
    fn a_link_is_not_a_tag() {
        assert_eq!(repos("## Docs\n- [ ] read [api](https://x.y)"), vec![Vec::<String>::new()]);
    }

    #[test]
    fn a_heading_naming_no_repository_keeps_the_owner_above_it() {
        assert_eq!(repos("## api\n### Task 3\n- [ ] x"), vec![names(&["api"])]);
    }

    #[test]
    fn a_heading_of_the_same_level_ends_the_owner_above_it() {
        assert_eq!(repos("## api\n- [ ] a\n## 3. Shared\n- [ ] b"), vec![names(&["api"]), vec![]]);
    }

    #[test]
    fn openspec_sections_leave_steps_owned_by_everyone() {
        let steps = owned_steps("## 1. Setup\n- [ ] 1.1 Add x", &folders());
        assert_eq!(steps.len(), 1);
        assert!(steps[0].repos.is_empty());
        assert_eq!(steps[0].label, "1.1");
    }

    #[test]
    fn a_tag_on_the_step_line_wins_over_the_heading() {
        let steps = owned_steps("## api\n- [ ] a [web]\n- [ ] b\n## Shared\n- [x] c [api]", &folders());
        let owners: Vec<_> = steps.iter().map(|s| s.repos.clone()).collect();
        assert_eq!(owners, vec![names(&["web"]), names(&["api"]), names(&["api"])]);
        assert!(steps[2].done);
    }

    #[test]
    fn positions_count_nested_checkboxes_in_file_order() {
        let plan = "\
### Task 1: Parse

- [ ] **Step 1: Write the test**
  - [ ] nested
- [ ] **Step 2: Run it**

### Task 2: Wire

- [x] **Step 1: Call it**
";
        let steps = owned_steps(plan, &folders());
        let positions: Vec<u32> = steps.iter().map(|s| s.position).collect();
        assert_eq!(positions, vec![1, 2, 3, 4]);
        assert_eq!(steps[0].label, "Step 1");
        assert_eq!(steps[1].text, "nested");
        assert!(steps[3].done);
    }

    #[test]
    fn a_bold_step_has_no_stars_left_in_its_text() {
        let steps = owned_steps(
            "- [ ] **Step 1: Write failing tests:**\n- [ ] **2.1 Add x**\n- [ ] **Add y**\n- [ ] **Step 2**: Run it",
            &folders(),
        );
        let got: Vec<_> = steps.iter().map(|s| (s.label.as_str(), s.text.as_str())).collect();
        assert_eq!(
            got,
            vec![("Step 1", "Write failing tests:"), ("2.1", "Add x"), ("", "Add y"), ("Step 2", "Run it")]
        );
    }

    #[test]
    fn a_step_without_a_number_has_no_label() {
        let steps = owned_steps("- [ ] Add the thing\n- [ ] 3D view\n- [ ] 4. Four", &folders());
        let labels: Vec<_> = steps.iter().map(|s| s.label.as_str()).collect();
        assert_eq!(labels, vec!["", "", "4"]);
        assert_eq!(steps[1].text, "3D view");
        assert_eq!(steps[2].text, "Four");
    }

    #[test]
    fn ticking_by_position_changes_only_that_box() {
        let text = "## api\n- [ ] a\n  - [ ] b\n- [ ] c\n";
        assert_eq!(tick_at(text, 3, true).as_deref(), Some("## api\n- [ ] a\n  - [ ] b\n- [x] c\n"));
        assert_eq!(tick_at(&tick_at(text, 2, true).unwrap(), 2, false).as_deref(), Some(text));
        assert_eq!(tick_at(text, 4, true), None);
        assert_eq!(tick_at(text, 0, true), None);
    }

    #[test]
    fn ticking_keeps_the_line_endings() {
        assert_eq!(tick_at("- [ ] a\r\n- [ ] b", 1, true).as_deref(), Some("- [x] a\r\n- [ ] b"));
    }

    #[test]
    fn checkboxes_inside_code_fences_are_not_steps() {
        let text = "- [ ] a\n```\n- [ ] not\n## api\n```\n- [ ] b\n";
        let steps = owned_steps(text, &folders());
        assert_eq!(steps.iter().map(|s| s.text.as_str()).collect::<Vec<_>>(), vec!["a", "b"]);
        assert!(steps[1].repos.is_empty());
        assert_eq!(tick_at(text, 2, true).as_deref(), Some("- [ ] a\n```\n- [ ] not\n## api\n```\n- [x] b\n"));
    }

    #[test]
    fn own_part_keeps_the_preamble_and_only_this_repositorys_sections() {
        let text =
            "# Plan\n\nIntro\n- [ ] shared first\n\n## api\n- [ ] a\n\n## web\n- [ ] b\n\n## 3. Shared\n- [ ] c\n";
        assert_eq!(
            own_part(text, "api", &folders()),
            "# Plan\n\nIntro\n- [ ] shared first\n\n## api\n- [ ] a\n\n## 3. Shared\n- [ ] c\n"
        );
        assert_eq!(
            own_part(text, "web", &folders()),
            "# Plan\n\nIntro\n- [ ] shared first\n\n## web\n- [ ] b\n\n## 3. Shared\n- [ ] c\n"
        );
    }

    #[test]
    fn own_part_drops_a_tagged_step_of_another_repository_with_what_hangs_under_it() {
        let text = "## Work\n- [ ] a [api]\n  detail\n  - [ ] a1\n- [ ] b [web]\n  more\n- [ ] c\n";
        assert_eq!(own_part(text, "api", &folders()), "## Work\n- [ ] a [api]\n  detail\n  - [ ] a1\n- [ ] c\n");
        assert_eq!(own_part(text, "web", &folders()), "## Work\n- [ ] b [web]\n  more\n- [ ] c\n");
    }

    #[test]
    fn own_part_leaves_a_file_without_owners_whole() {
        let text = "## 1. Setup\n- [ ] 1.1 Add x\n";
        assert_eq!(own_part(text, "api", &folders()), text);
    }
}
