//! Target and prerequisite graph for ordinary POSIX.1-2024 make target rules.

use crate::{
    reader::{Entry, Makefile, Rule, SourceLine},
    Error, Location,
};
use std::collections::HashMap;

#[derive(Debug)]
pub struct Graph<'a> {
    nodes: Vec<Node<'a>>,
    indices: HashMap<&'a str, usize>,
    pub(crate) roots: Vec<usize>,
    default_build_order: Vec<usize>,
}

#[derive(Debug)]
pub struct Node<'a> {
    pub(crate) name: &'a str,
    pub(crate) location: Option<&'a Location>,
    pub(crate) has_rule: bool,
    pub(crate) prerequisites: Vec<Edge<'a>>,
    recipe_rule: Option<&'a Rule>,
}

/// Split a library target or prerequisite into its archive and member names.
pub(crate) fn archive_member(name: &str) -> Option<(&str, &str)> {
    let (archive, member) = name.strip_suffix(')')?.split_once('(')?;
    (!archive.is_empty()
        && !member.is_empty()
        && !archive.contains(')')
        && !member.contains(['(', ')']))
    .then_some((archive, member))
}

#[derive(Debug)]
pub(crate) struct Edge<'a> {
    pub(crate) target: Option<usize>,
    location: &'a Location,
}

impl<'a> Node<'a> {
    pub fn recipes(&self) -> &'a [SourceLine] {
        self.recipe_rule.map_or(&[], |rule| &rule.recipes)
    }

    pub(crate) fn inference(&self) -> Option<&'a crate::reader::Inference> {
        self.recipe_rule.and_then(|rule| rule.inference.as_ref())
    }
}

#[derive(Clone, Copy)]
enum Visit {
    Unseen,
    Active(usize),
    Done,
}

impl<'a> Graph<'a> {
    pub fn from_makefile(makefile: &'a Makefile) -> Result<Self, Error> {
        Self::for_targets(makefile, &[])
    }

    pub fn for_targets(makefile: &'a Makefile, targets: &'a [String]) -> Result<Self, Error> {
        let mut graph = Self {
            nodes: Vec::new(),
            indices: HashMap::new(),
            roots: Vec::new(),
            default_build_order: Vec::new(),
        };
        let mut default_target = None;
        for entry in &makefile.entries {
            if let Entry::Rule(rule) = entry {
                if rule.inference.is_none() && default_target.is_none() {
                    default_target = rule.targets.first().map(String::as_str);
                }
                graph.add_rule(rule)?;
            }
        }
        let mut roots: Vec<_> = targets
            .iter()
            .map(|target| graph.intern(target, None))
            .collect();
        if let Some(target) = default_target.filter(|_| roots.is_empty()) {
            roots.push(graph.indices[target]);
        }
        graph.default_build_order = graph.check_cycles(&roots)?;
        graph.roots = roots;
        Ok(graph)
    }

    pub fn node(&self, name: &str) -> Option<&Node<'a>> {
        self.indices.get(name).map(|&index| &self.nodes[index])
    }

    pub fn prerequisites(&self, name: &str) -> Option<impl Iterator<Item = &'a str> + '_> {
        Some(self.prerequisites_of(self.node(name)?))
    }

    pub(crate) fn prerequisites_of<'g>(
        &'g self,
        node: &'g Node<'a>,
    ) -> impl Iterator<Item = &'a str> + 'g {
        node.prerequisites
            .iter()
            .filter_map(|edge| edge.target.map(|target| self.nodes[target].name))
    }

    pub(crate) fn nodes(&self) -> &[Node<'a>] {
        &self.nodes
    }

    pub(crate) fn default_build_order(&self) -> &[usize] {
        &self.default_build_order
    }

    fn add_rule(&mut self, rule: &'a Rule) -> Result<(), Error> {
        // Derived rules point to the inference recipe, not the reference to
        // the target. Direct command-line goals have no makefile location.
        let location = rule.inference.is_none().then_some(&rule.location);
        let targets: Vec<_> = rule
            .targets
            .iter()
            .map(|name| self.intern(name, location))
            .collect();
        let prerequisites: Vec<_> = rule
            .prerequisites
            .iter()
            .map(|name| (name != ".WAIT").then(|| self.intern(name, location)))
            .collect();
        for target in targets {
            let node = &mut self.nodes[target];
            node.has_rule = true;
            if !rule.recipes.is_empty() {
                if let Some(previous) = node.recipe_rule {
                    if !std::ptr::eq(previous, rule) {
                        return Err(Error::new(
                            &rule.location,
                            format!("multiple recipes for target '{}'", node.name),
                        ));
                    }
                    continue;
                }
                node.recipe_rule = Some(rule);
            }
            node.prerequisites
                .extend(prerequisites.iter().map(|&target| Edge {
                    target,
                    location: &rule.location,
                }));
        }
        Ok(())
    }

    fn intern(&mut self, name: &'a str, location: Option<&'a Location>) -> usize {
        *self.indices.entry(name).or_insert_with(|| {
            let index = self.nodes.len();
            self.nodes.push(Node {
                name,
                location,
                has_rule: false,
                prerequisites: Vec::new(),
                recipe_rule: None,
            });
            index
        })
    }

    fn check_cycles(&self, roots: &[usize]) -> Result<Vec<usize>, Error> {
        let mut visits = vec![Visit::Unseen; self.nodes.len()];
        let mut default_build_order = Vec::new();
        // Each stack frame records a node and the next edge to visit. Use a
        // heap-backed stack so a long dependency chain cannot exhaust the call stack.
        let mut stack = Vec::new();
        let selected = roots.iter().copied().map(|root| (root, true));
        let remaining = (0..self.nodes.len()).map(|root| (root, false));
        for (root, build) in selected.chain(remaining) {
            if !matches!(visits[root], Visit::Unseen) {
                continue;
            }
            visits[root] = Visit::Active(0);
            stack.push((root, 0));
            while let Some((node, next_edge)) = stack.last_mut() {
                let Some(edge) = self.nodes[*node].prerequisites.get(*next_edge) else {
                    visits[*node] = Visit::Done;
                    if build {
                        default_build_order.push(*node);
                    }
                    stack.pop();
                    continue;
                };
                *next_edge += 1;
                let Some(target) = edge.target else {
                    continue;
                };
                match visits[target] {
                    Visit::Unseen => {
                        visits[target] = Visit::Active(stack.len());
                        stack.push((target, 0));
                    }
                    Visit::Active(start) => {
                        let cycle: Vec<_> = stack[start..]
                            .iter()
                            .map(|&(node, _)| self.nodes[node].name)
                            .chain(std::iter::once(self.nodes[target].name))
                            .collect();
                        return Err(Error::new(
                            edge.location,
                            format!("dependency cycle: {}", cycle.join(" -> ")),
                        ));
                    }
                    Visit::Done => (),
                }
            }
        }
        Ok(default_build_order)
    }
}
