//! What changed between two snapshots of one page (design P3.4).
//!
//! Both trees are the engine's YAML, one element a line, children indented
//! under their parent. The answer keeps only the lines that changed: `+ ` for
//! a line that is new, `- ` for one that is gone, and, unmarked, the lines of
//! the parents they sit under, so a reader can tell where each change is.
//! Every other line is left out.
//!
//! Refs can be compared across the two because the engine keeps an
//! element's ref for as long as the element lives in its document. That only
//! holds within one document, which the caller checks before asking.

/// Longest-common-subsequence work allowed on the part of the trees that
/// differ, in cells. Past it, that part counts as all removed and all added:
/// still a true answer, only a longer one.
const MAX_CELLS: usize = 4_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    Same,
    Added,
    Removed,
}

/// The changes from `before` to `after`, or None when there are none.
pub fn changes(before: &str, after: &str) -> Option<String> {
    let old: Vec<&str> = before.lines().collect();
    let new: Vec<&str> = after.lines().collect();
    let ops = diff(&old, &new);
    if ops.iter().all(|(op, _)| *op == Op::Same) {
        return None;
    }
    // Parents of the line being written, in the new tree, nearest last; the
    // flag says whether the line is already in the answer.
    let mut parents: Vec<(usize, &str, bool)> = Vec::new();
    let mut out = String::new();
    for (op, line) in ops {
        let depth = indent(line);
        if op != Op::Removed {
            while parents.last().is_some_and(|(d, _, _)| *d >= depth) {
                parents.pop();
            }
        }
        if op != Op::Same {
            for (d, parent, shown) in parents.iter_mut() {
                if *d < depth && !*shown {
                    out.push_str("  ");
                    out.push_str(parent);
                    out.push('\n');
                    *shown = true;
                }
            }
            out.push_str(if op == Op::Added { "+ " } else { "- " });
            out.push_str(line);
            out.push('\n');
        }
        if op != Op::Removed {
            parents.push((depth, line, op == Op::Added));
        }
    }
    out.pop();
    Some(out)
}

fn indent(line: &str) -> usize {
    line.len() - line.trim_start_matches(' ').len()
}

/// The two trees line by line, in order: kept, added or removed.
fn diff<'a>(old: &[&'a str], new: &[&'a str]) -> Vec<(Op, &'a str)> {
    let head = old.iter().zip(new).take_while(|(a, b)| a == b).count();
    let tail = old[head..]
        .iter()
        .rev()
        .zip(new[head..].iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    let (a, b) = (&old[head..old.len() - tail], &new[head..new.len() - tail]);
    let mut ops: Vec<(Op, &str)> = old[..head].iter().map(|l| (Op::Same, *l)).collect();
    if a.len().saturating_mul(b.len()) > MAX_CELLS {
        ops.extend(a.iter().map(|l| (Op::Removed, *l)));
        ops.extend(b.iter().map(|l| (Op::Added, *l)));
    } else {
        ops.extend(middle(a, b));
    }
    ops.extend(old[old.len() - tail..].iter().map(|l| (Op::Same, *l)));
    ops
}

/// The plain LCS diff of `a` against `b`, removals before additions.
fn middle<'a>(a: &[&'a str], b: &[&'a str]) -> Vec<(Op, &'a str)> {
    let w = b.len() + 1;
    // lcs[i * w + j]: the common length of a[i..] and b[j..].
    let mut lcs = vec![0u32; (a.len() + 1) * w];
    for i in (0..a.len()).rev() {
        for j in (0..b.len()).rev() {
            lcs[i * w + j] = if a[i] == b[j] {
                lcs[(i + 1) * w + j + 1] + 1
            } else {
                lcs[(i + 1) * w + j].max(lcs[i * w + j + 1])
            };
        }
    }
    let (mut i, mut j) = (0, 0);
    let mut ops = Vec::with_capacity(a.len() + b.len());
    while i < a.len() || j < b.len() {
        if i < a.len() && j < b.len() && a[i] == b[j] {
            ops.push((Op::Same, a[i]));
            i += 1;
            j += 1;
        } else if j == b.len() || (i < a.len() && lcs[(i + 1) * w + j] >= lcs[i * w + j + 1]) {
            ops.push((Op::Removed, a[i]));
            i += 1;
        } else {
            ops.push((Op::Added, b[j]));
            j += 1;
        }
    }
    ops
}

#[cfg(test)]
mod tests {
    use super::*;

    const BEFORE: &str = "\
- banner [ref=e1]:
  - heading \"Shop\" [level=1] [ref=e2]
- main [ref=e3]:
  - list [ref=e4]:
    - listitem [ref=e5]: Apples
    - listitem [ref=e6]: Pears
  - button \"Save\" [ref=e7]
- contentinfo [ref=e8]: Footer";

    #[test]
    fn an_unchanged_page_has_no_changes() {
        assert_eq!(changes(BEFORE, BEFORE), None);
    }

    #[test]
    fn a_change_comes_with_the_parents_it_sits_under_and_nothing_else() {
        let after = BEFORE
            .replace("  - button \"Save\" [ref=e7]", "  - status [ref=e9]: Saved")
            .replace("    - listitem [ref=e6]: Pears\n", "");
        assert_eq!(
            changes(BEFORE, &after).unwrap(),
            "  - main [ref=e3]:\n\
             \x20   - list [ref=e4]:\n\
             -     - listitem [ref=e6]: Pears\n\
             -   - button \"Save\" [ref=e7]\n\
             +   - status [ref=e9]: Saved"
        );
    }

    #[test]
    fn a_new_subtree_is_all_added_under_its_parent() {
        let after = BEFORE.replace(
            "- contentinfo",
            "- dialog \"Sure?\" [ref=e10]:\n  - button \"Yes\" [ref=e11]\n- contentinfo",
        );
        assert_eq!(
            changes(BEFORE, &after).unwrap(),
            "+ - dialog \"Sure?\" [ref=e10]:\n+   - button \"Yes\" [ref=e11]"
        );
    }

    #[test]
    fn a_tree_too_big_to_compare_line_by_line_is_still_told_truly() {
        let old: Vec<String> = (0..3000).map(|i| format!("- text: a{i}")).collect();
        let new: Vec<String> = (0..3000).map(|i| format!("- text: b{i}")).collect();
        let out = changes(&old.join("\n"), &new.join("\n")).unwrap();
        assert_eq!(out.lines().filter(|l| l.starts_with("- ")).count(), 3000);
        assert_eq!(out.lines().filter(|l| l.starts_with("+ ")).count(), 3000);
    }
}
