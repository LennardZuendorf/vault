//! The two kinds of glob a toolset manifest uses (`files`, `steps`): `*`
//! within one path segment and `**` across any number of them. Written by
//! hand because nothing else needs more, and a manifest's globs must never
//! reach outside the spec folder, so there is one place that says what a
//! safe relative path is.

/// A relative path that stays inside the folder it is relative to: not
/// empty, not absolute, no `.` or `..` or empty segment, no backslash or
/// NUL. Used for globs too, where `*` is an ordinary character.
pub fn is_safe_rel(rel: &str) -> bool {
    !rel.is_empty()
        && !rel.contains(['\\', '\0'])
        && rel.split('/').all(|seg| !seg.is_empty() && seg != "." && seg != "..")
}

/// Whether `rel` (a safe relative path, `/`-separated) matches `pattern`.
/// A segment `**` matches zero or more whole segments; a `*` inside any
/// other segment matches any run of characters within that segment. A `rel`
/// that is not safe matches nothing.
pub fn matches(pattern: &str, rel: &str) -> bool {
    if !is_safe_rel(rel) {
        return false;
    }
    let pat: Vec<&str> = pattern.split('/').collect();
    let path: Vec<&str> = rel.split('/').collect();
    segments(&pat, &path)
}

fn segments(pat: &[&str], path: &[&str]) -> bool {
    match pat.split_first() {
        None => path.is_empty(),
        Some((&"**", rest)) => (0..=path.len()).any(|skip| segments(rest, &path[skip..])),
        Some((first, rest)) => match path.split_first() {
            Some((seg, tail)) => segment(first, seg) && segments(rest, tail),
            None => false,
        },
    }
}

/// One segment against one pattern segment, `*` matching any run. Greedy
/// with a single backtrack point, so it cannot blow up on `a*a*a*a*b`.
fn segment(pat: &str, text: &str) -> bool {
    let (p, t): (Vec<char>, Vec<char>) = (pat.chars().collect(), text.chars().collect());
    let (mut pi, mut ti) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while ti < t.len() {
        if pi < p.len() && p[pi] == '*' {
            star = Some((pi, ti));
            pi += 1;
        } else if pi < p.len() && p[pi] == t[ti] {
            pi += 1;
            ti += 1;
        } else if let Some((sp, st)) = star {
            pi = sp + 1;
            ti = st + 1;
            star = Some((sp, st + 1));
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|&c| c == '*')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn double_star_matches_any_depth_and_star_one_segment() {
        assert!(matches("openspec/**/*.md", "openspec/changes/a/specs/x/spec.md"));
        assert!(matches("openspec/**/*.md", "openspec/readme.md"));
        assert!(matches("docs/superpowers/plans/*.md", "docs/superpowers/plans/a.md"));
        assert!(!matches("docs/superpowers/plans/*.md", "docs/superpowers/plans/a/b.md"));
        assert!(matches("openspec/changes/*/tasks.md", "openspec/changes/add-x/tasks.md"));
        assert!(!matches("openspec/changes/*/tasks.md", "openspec/changes/tasks.md"));
        assert!(matches("tasks.md", "tasks.md"));
        assert!(!matches("tasks.md", "design.md"));
        assert!(!matches("openspec/**/*.md", "other/a.md"));
    }

    #[test]
    fn a_star_inside_a_segment_matches_part_of_its_name() {
        assert!(matches("specs/req-*.md", "specs/req-1.md"));
        assert!(!matches("specs/req-*.md", "specs/design-1.md"));
        assert!(matches("a*b*c", "aXXbYYc"));
        assert!(!matches("a*b*c", "aXXbYY"));
    }

    #[test]
    fn a_path_that_is_not_safe_never_matches() {
        assert!(!matches("**", "../a.md"));
        assert!(!matches("**", "/a.md"));
    }

    #[test]
    fn unsafe_relative_paths_are_rejected() {
        for bad in ["", "../a", "a/../../b", "/a", "a\\b", "a//b", "./a", "a/", "a/..", "a\0b"] {
            assert!(!is_safe_rel(bad), "{bad:?} should be unsafe");
        }
        for good in ["a", "a/b.md", "openspec/**/*.md", "specs/{task}", "..a/b", "a/b..c"] {
            assert!(is_safe_rel(good), "{good:?} should be safe");
        }
    }
}
