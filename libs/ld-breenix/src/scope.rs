use core::ops::Range;

use crate::object::LinkString;
use crate::{Error, ErrorKind, Object};

/// One edge of the resolved dependency graph, stored in caller-owned space.
#[derive(Clone, Copy, Default)]
pub struct Dependency {
    from: usize,
    to: usize,
}

/// Scratch state for walking the dependency graph without recursion.
#[derive(Clone, Copy, Default)]
pub struct InitState {
    next_edge: Option<usize>,
    parent: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InitKind {
    Preinit,
    Init,
}

#[derive(Debug, PartialEq, Eq)]
pub struct InitArray {
    pub object: usize,
    pub kind: InitKind,
    /// Runtime addresses of the relocated function-pointer slots.
    pub range: Range<u64>,
}

pub struct Scope<'a, 's> {
    objects: &'s [Object<'a>],
    edges: &'s [Dependency],
}

impl<'a, 's> Scope<'a, 's> {
    /// Resolve immediate dependencies in DT_NEEDED order, visiting the
    /// executable and then each level of dependencies in breadth-first order.
    ///
    /// The resolver supplies parsed DSOs with stable canonical names. A missing or
    /// ABI-incompatible candidate continues the search. An error aborts it.
    /// Storage holds one object per canonical name and one edge per DT_NEEDED.
    pub fn load(
        root: Object<'a>,
        objects: &'s mut [Object<'a>],
        edges: &'s mut [Dependency],
        path: &mut [u8],
        mut open: impl FnMut(&[u8]) -> Result<Option<Object<'a>>, Error<'a>>,
    ) -> Result<Self, Error<'a>> {
        *objects
            .first_mut()
            .ok_or_else(|| root.at(0, ErrorKind::Capacity))? = root;
        let mut count = 1;
        let mut edge_count = 0;
        let mut from = 0;
        while from < count {
            let requester = objects[from];
            for needed in requester.needed() {
                let needed = needed?;
                let slot = edges
                    .get_mut(edge_count)
                    .ok_or_else(|| requester.at(needed.offset, ErrorKind::Capacity))?;
                let target = find_needed(requester, needed, path, |candidate| {
                    if let Some(object) =
                        objects[..count].iter().find(|obj| obj.name() == candidate)
                    {
                        return Ok(Some(*object));
                    }
                    open(candidate)
                })?;
                let to = if let Some(existing) = objects[..count]
                    .iter()
                    .position(|obj| obj.name() == target.name())
                {
                    existing
                } else {
                    if !target.is_shared() {
                        return Err(dependency_error(
                            requester,
                            needed,
                            ErrorKind::InvalidDependency,
                        ));
                    }
                    *objects
                        .get_mut(count)
                        .ok_or_else(|| requester.at(needed.offset, ErrorKind::Capacity))? = target;
                    count += 1;
                    count - 1
                };
                *slot = Dependency { from, to };
                edge_count += 1;
            }
            from += 1;
        }
        Ok(Self {
            objects: &objects[..count],
            edges: &edges[..edge_count],
        })
    }

    pub fn objects(&self) -> &[Object<'a>] {
        self.objects
    }

    #[cfg(feature = "interpreter")]
    pub(crate) fn dependencies(&self, from: usize) -> impl Iterator<Item = usize> + '_ {
        self.edges
            .iter()
            .filter(move |edge| edge.from == from)
            .map(|edge| edge.to)
    }

    #[cfg(feature = "interpreter")]
    pub(crate) fn open_name(
        requester: Object<'a>,
        name: &'a [u8],
        path: &mut [u8],
        mut open: impl FnMut(&[u8]) -> Result<Option<Object<'a>>, Error<'a>>,
    ) -> Result<Object<'a>, Error<'a>> {
        if name.contains(&b'/') {
            return open(name)?.ok_or_else(|| requester.at(0, ErrorKind::MissingDependency));
        }
        find_needed(
            requester,
            LinkString {
                bytes: name,
                offset: 0,
            },
            path,
            open,
        )
    }

    /// Emit the executable's PREINIT_ARRAY, then INIT_ARRAYs in dependency
    /// order. Walk each array from its lowest slot to its highest slot.
    /// Each object is visited once. Back edges in cycles are ignored.
    pub fn initializers(
        &self,
        state: &mut [InitState],
        mut emit: impl FnMut(InitArray),
    ) -> Result<(), Error<'a>> {
        let root = self.objects[0];
        let state = state
            .get_mut(..self.objects.len())
            .ok_or_else(|| root.at(0, ErrorKind::Capacity))?;
        state.fill(InitState::default());
        if let Some(preinit) = root.preinit.filter(|_| !root.is_shared()) {
            emit(InitArray {
                object: 0,
                kind: InitKind::Preinit,
                range: preinit.range(),
            });
        }
        // Edges are grouped by source index by load(). Keep each active
        // node's cursor and parent so deep dependency chains use no stack.
        let mut object = 0;
        let mut edge_index = 0;
        state[0].next_edge = Some(0);
        loop {
            if let Some(edge) = self
                .edges
                .get(edge_index)
                .filter(|edge| edge.from == object)
            {
                edge_index += 1;
                if state[edge.to].next_edge.is_none() {
                    state[object].next_edge = Some(edge_index);
                    edge_index = self.edges.partition_point(|other| other.from < edge.to);
                    state[edge.to] = InitState {
                        next_edge: Some(edge_index),
                        parent: Some(object),
                    };
                    object = edge.to;
                }
            } else {
                if let Some(init) = self.objects[object].init {
                    emit(InitArray {
                        object,
                        kind: InitKind::Init,
                        range: init.range(),
                    });
                }
                let Some(parent) = state[object].parent else {
                    return Ok(());
                };
                object = parent;
                // A parent's cursor is saved before descending.
                edge_index = state[parent].next_edge.unwrap_or(0);
            }
        }
    }
}

fn dependency_error<'a>(object: Object<'a>, needed: LinkString<'a>, kind: ErrorKind) -> Error<'a> {
    Error {
        detail: needed.bytes,
        ..object.at(needed.offset, kind)
    }
}

fn find_needed<'a>(
    object: Object<'a>,
    needed: LinkString<'a>,
    path: &mut [u8],
    mut open: impl FnMut(&[u8]) -> Result<Option<Object<'a>>, Error<'a>>,
) -> Result<Object<'a>, Error<'a>> {
    if needed.bytes.is_empty() {
        return Err(dependency_error(
            object,
            needed,
            ErrorKind::InvalidDependency,
        ));
    }
    let mut candidate = Path {
        object,
        data: path,
        len: 0,
    };
    candidate.expand(needed)?;
    if candidate.bytes().contains(&b'/') {
        return open(candidate.bytes())?
            .ok_or_else(|| dependency_error(object, needed, ErrorKind::MissingDependency));
    }

    let runpath = object.runpath.into_iter().flat_map(|entry| {
        entry
            .bytes
            .split(|&byte| byte == b':')
            .map(move |bytes| LinkString { bytes, ..entry })
    });
    let defaults = [b"/lib".as_slice(), b"/usr/lib"].map(|bytes| LinkString {
        bytes,
        offset: needed.offset,
    });
    for dir in runpath.chain(defaults) {
        candidate.len = 0;
        candidate.expand(dir)?;
        if !candidate.bytes().is_empty() && !candidate.bytes().ends_with(b"/") {
            candidate.append(b"/", dir.offset)?;
        }
        candidate.expand(needed)?;
        if let Some(dependency) = open(candidate.bytes())? {
            return Ok(dependency);
        }
    }
    Err(dependency_error(
        object,
        needed,
        ErrorKind::MissingDependency,
    ))
}

struct Path<'a, 'p> {
    object: Object<'a>,
    data: &'p mut [u8],
    len: usize,
}

impl<'a> Path<'a, '_> {
    fn bytes(&self) -> &[u8] {
        &self.data[..self.len]
    }

    fn append(&mut self, text: &[u8], offset: u64) -> Result<(), Error<'a>> {
        let dst = self
            .data
            .get_mut(self.len..)
            .and_then(|tail| tail.get_mut(..text.len()))
            .ok_or_else(|| self.object.at(offset, ErrorKind::Capacity))?;
        dst.copy_from_slice(text);
        self.len += text.len();
        Ok(())
    }

    fn expand(&mut self, entry: LinkString<'a>) -> Result<(), Error<'a>> {
        let mut rest = entry.bytes;
        while let Some(dollar) = rest.iter().position(|&byte| byte == b'$') {
            let (prefix, token) = rest.split_at(dollar);
            self.append(prefix, entry.offset)?;
            let after = token.strip_prefix(b"${ORIGIN}").or_else(|| {
                token.strip_prefix(b"$ORIGIN").filter(|tail| {
                    !tail
                        .first()
                        .is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
                })
            });
            let Some(after) = after else {
                return Err(dependency_error(
                    self.object,
                    entry,
                    ErrorKind::UnsupportedSubstitution,
                ));
            };
            let name = self.object.name();
            let origin = name
                .iter()
                .rposition(|&byte| byte == b'/')
                .filter(|_| name.starts_with(b"/"))
                .map(|last| &name[..last.max(1)])
                .ok_or_else(|| {
                    dependency_error(self.object, entry, ErrorKind::InvalidDependency)
                })?;
            // At filesystem root the token and its suffix may share a slash.
            if origin != b"/" || !after.starts_with(b"/") {
                self.append(origin, entry.offset)?;
            }
            rest = after;
        }
        self.append(rest, entry.offset)
    }
}
