use rand::prelude::Rng;
use std::fmt::Write;
use tree_sitter::{Node, Tree, TreeCursor};

pub struct Pattern {
    kind: Option<&'static str>,
    named: bool,
    field: Option<&'static str>,
    capture: Option<String>,
    children: Vec<Pattern>,
}

#[derive(Clone)]
pub struct Match<'a, 'tree> {
    pub captures: Vec<(&'a str, Node<'tree>)>,
    pub last_node: Node<'tree>,
}

const CAPTURE_NAMES: &'static [&'static str] = &[
    "one", "two", "three", "four", "five", "six", "seven", "eight",
];

impl Pattern {
    pub fn random_pattern_in_tree(tree: &Tree, rng: &mut impl Rng) -> Self {
        let mut cursor = tree.walk();

        // Descend to the node at a random byte offset and a depth.
        let mut max_depth = 0;
        let byte_offset = rng.gen_range(0..cursor.node().end_byte());
        while cursor.goto_first_child_for_byte(byte_offset).is_some() {
            max_depth += 1;
        }
        let depth = rng.gen_range(0..max_depth);
        for _ in 0..depth {
            cursor.goto_parent();
        }

        // Build a pattern that matches that node.
        // Sometimes include subsequent siblings of the node.
        let mut roots = vec![Self::random_pattern_for_node(&mut cursor, rng)];
        while cursor.goto_next_sibling() {
            if rng.gen_bool(0.2) {
                roots.push(Self::random_pattern_for_node(&mut cursor, rng));
            }
        }

        // Right now, it's not syntactically valid to have a parenthesized
        // list of sibling patterns where the first pattern in the list has
        // a field name.
        if roots.len() > 1 {
            roots[0].field = None;
            Self {
                kind: None,
                named: true,
                field: None,
                capture: None,
                children: roots,
            }
        } else {
            roots.pop().unwrap()
        }
    }

    fn random_pattern_for_node(cursor: &mut TreeCursor, rng: &mut impl Rng) -> Self {
        let node = cursor.node();
        let (kind, named) = if rng.gen_bool(0.9) {
            (Some(node.kind()), node.is_named())
        } else {
            (Some("_"), true)
        };

        let field = if rng.gen_bool(0.75) {
            cursor.field_name()
        } else {
            None
        };

        let capture = if rng.gen_bool(0.7) {
            Some(CAPTURE_NAMES[rng.gen_range(0..CAPTURE_NAMES.len())].to_string())
        } else {
            None
        };

        let mut children = Vec::new();
        if cursor.goto_first_child() {
            let max_children = rng.gen_range(0..4);
            while cursor.goto_next_sibling() {
                if rng.gen_bool(0.6) {
                    let child_ast = Self::random_pattern_for_node(cursor, rng);
                    children.push(child_ast);
                    if children.len() >= max_children {
                        break;
                    }
                }
            }
            cursor.goto_parent();
        }

        Self {
            kind,
            named,
            field,
            capture,
            children,
        }
    }

    pub fn to_string(&self) -> String {
        let mut result = String::new();
        self.write_to_string(&mut result);
        result
    }

    fn write_to_string(&self, string: &mut String) {
        if let Some(field) = self.field {
            write!(string, "{}: ", field).unwrap();
        }

        if self.named {
            string.push('(');
            let mut has_contents = false;
            if let Some(kind) = &self.kind {
                write!(string, "{}", kind).unwrap();
                has_contents = true;
            }
            for child in &self.children {
                if has_contents {
                    string.push(' ');
                }
                child.write_to_string(string);
                has_contents = true;
            }
            string.push(')');
        } else {
            write!(string, "\"{}\"", self.kind.unwrap().replace("\"", "\\\"")).unwrap();
        }

        if let Some(capture) = &self.capture {
            write!(string, " @{}", capture).unwrap();
        }
    }

    pub fn matches_in_tree<'tree>(&self, tree: &'tree Tree) -> Vec<Match<'_, 'tree>> {
        let mut matches = Vec::new();
        let mut cursor = tree.walk();
        let mut ascending = false;
        loop {
            if ascending {
                if cursor.goto_next_sibling() {
                    ascending = false;
                } else if !cursor.goto_parent() {
                    break;
                }
            } else {
                let matches_here = self.match_node(&mut cursor);
                matches.extend_from_slice(&matches_here);
                if !cursor.goto_first_child() {
                    ascending = true;
                }
            }
        }
        matches.sort_unstable_by_key(|mat| {
            let range = mat.last_node.byte_range();
            (range.start, -(range.end as isize))
        });
        matches.dedup_by(|a, b| a.captures == b.captures);
        matches
    }

    pub fn match_node<'tree>(&self, cursor: &mut TreeCursor<'tree>) -> Vec<Match<'_, 'tree>> {
        let node = cursor.node();

        // If a kind is specified, check that it matches the node.
        if let Some(kind) = &self.kind {
            if *kind != "_" && node.kind() != *kind || node.is_named() != self.named {
                return Vec::new();
            }
        }

        // If a field is specified, check that it matches the node.
        if let Some(field) = &self.field {
            if cursor.field_name() != Some(*field) {
                return Vec::new();
            }
        }

        let mut captures = Vec::new();
        if let Some(name) = &self.capture {
            captures.push((name.as_str(), node));
        }
        let mat = Match {
            captures,
            last_node: node,
        };

        if self.children.is_empty() {
            return vec![mat];
        }

        // Find every matching combination of child patterns and child nodes.
        if !cursor.goto_first_child() {
            return Vec::new();
        }

        let mut match_states = vec![(0, mat)];
        let mut finished_matches = Vec::new();
        loop {
            let mut new_match_states = Vec::new();
            for (pattern_index, mat) in &match_states {
                let child_pattern = &self.children[*pattern_index];
                let child_matches = child_pattern.match_node(cursor);
                for child_match in child_matches {
                    let mut combined_match = mat.clone();
                    combined_match.last_node = child_match.last_node;
                    combined_match
                        .captures
                        .extend_from_slice(&child_match.captures);
                    if pattern_index + 1 < self.children.len() {
                        new_match_states.push((*pattern_index + 1, combined_match));
                    } else {
                        finished_matches.push(combined_match);
                    }
                }
            }
            match_states.extend_from_slice(&new_match_states);
            if !cursor.goto_next_sibling() {
                break;
            }
        }
        cursor.goto_parent();

        finished_matches
    }
}
