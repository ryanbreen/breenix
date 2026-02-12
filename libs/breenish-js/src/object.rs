//! JavaScript object storage for the breenish-js engine.
//!
//! Objects are stored in a heap (Vec<JsObject>) and referenced by index.
//! Each object has a property map and optional array storage.

use alloc::vec::Vec;

use crate::string::StringId;
use crate::value::JsValue;

/// A property key (either a string or numeric index).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PropertyKey {
    String(StringId),
    Index(u32),
}

/// A single property with its value.
#[derive(Debug, Clone)]
struct Property {
    key: StringId,
    value: JsValue,
}

/// Object kind determines special behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectKind {
    /// A plain object `{}`.
    Ordinary,
    /// An array object `[]`.
    Array,
    /// A function object (index into function table).
    Function(u32),
}

/// A JavaScript object with named properties and optional indexed storage.
#[derive(Debug)]
pub struct JsObject {
    /// The kind of object.
    pub kind: ObjectKind,
    /// Named properties (linear scan; hash map for Phase 3+).
    properties: Vec<Property>,
    /// Indexed elements for arrays.
    elements: Vec<JsValue>,
    /// Prototype object index (None = null prototype).
    pub prototype: Option<u32>,
}

impl JsObject {
    /// Create a new ordinary object.
    pub fn new() -> Self {
        Self {
            kind: ObjectKind::Ordinary,
            properties: Vec::new(),
            elements: Vec::new(),
            prototype: None,
        }
    }

    /// Create a new array object.
    pub fn new_array() -> Self {
        Self {
            kind: ObjectKind::Array,
            properties: Vec::new(),
            elements: Vec::new(),
            prototype: None,
        }
    }

    /// Create a new function object.
    pub fn new_function(func_index: u32) -> Self {
        Self {
            kind: ObjectKind::Function(func_index),
            properties: Vec::new(),
            elements: Vec::new(),
            prototype: None,
        }
    }

    /// Get a named property.
    pub fn get(&self, key: StringId) -> JsValue {
        for prop in &self.properties {
            if prop.key == key {
                return prop.value;
            }
        }
        JsValue::undefined()
    }

    /// Set a named property.
    pub fn set(&mut self, key: StringId, value: JsValue) {
        for prop in &mut self.properties {
            if prop.key == key {
                prop.value = value;
                return;
            }
        }
        self.properties.push(Property { key, value });
    }

    /// Check if a named property exists.
    pub fn has(&self, key: StringId) -> bool {
        self.properties.iter().any(|p| p.key == key)
    }

    /// Delete a named property.
    pub fn delete(&mut self, key: StringId) -> bool {
        if let Some(pos) = self.properties.iter().position(|p| p.key == key) {
            self.properties.remove(pos);
            true
        } else {
            false
        }
    }

    /// Get all property keys.
    pub fn keys(&self) -> Vec<StringId> {
        self.properties.iter().map(|p| p.key).collect()
    }

    // --- Array / indexed element operations ---

    /// Get an indexed element.
    pub fn get_index(&self, index: u32) -> JsValue {
        self.elements
            .get(index as usize)
            .copied()
            .unwrap_or(JsValue::undefined())
    }

    /// Set an indexed element, growing the elements vector if needed.
    pub fn set_index(&mut self, index: u32, value: JsValue) {
        let idx = index as usize;
        if idx >= self.elements.len() {
            self.elements.resize(idx + 1, JsValue::undefined());
        }
        self.elements[idx] = value;
    }

    /// Get the number of indexed elements (array length).
    pub fn elements_len(&self) -> u32 {
        self.elements.len() as u32
    }

    /// Push a value onto the end of the elements array.
    pub fn push(&mut self, value: JsValue) {
        self.elements.push(value);
    }

    /// Pop a value from the end of the elements array.
    pub fn pop(&mut self) -> JsValue {
        self.elements.pop().unwrap_or(JsValue::undefined())
    }

    /// Get a reference to the elements slice.
    pub fn elements(&self) -> &[JsValue] {
        &self.elements
    }

    /// Get the number of named properties.
    pub fn property_count(&self) -> usize {
        self.properties.len()
    }
}

/// A managed object heap.
///
/// Objects are stored by index and can be accessed/modified via the heap.
/// In Phase 2 we use simple reference counting; a full cycle collector
/// will be added later.
pub struct ObjectHeap {
    objects: Vec<Option<JsObject>>,
    /// Free list for reuse of slots.
    free_list: Vec<u32>,
}

impl ObjectHeap {
    pub fn new() -> Self {
        Self {
            objects: Vec::new(),
            free_list: Vec::new(),
        }
    }

    /// Allocate a new object and return its index.
    pub fn alloc(&mut self, obj: JsObject) -> u32 {
        if let Some(idx) = self.free_list.pop() {
            self.objects[idx as usize] = Some(obj);
            idx
        } else {
            let idx = self.objects.len() as u32;
            self.objects.push(Some(obj));
            idx
        }
    }

    /// Get a reference to an object by index.
    pub fn get(&self, index: u32) -> Option<&JsObject> {
        self.objects.get(index as usize).and_then(|o| o.as_ref())
    }

    /// Get a mutable reference to an object by index.
    pub fn get_mut(&mut self, index: u32) -> Option<&mut JsObject> {
        self.objects.get_mut(index as usize).and_then(|o| o.as_mut())
    }

    /// Free an object slot for reuse.
    pub fn free(&mut self, index: u32) {
        if let Some(slot) = self.objects.get_mut(index as usize) {
            *slot = None;
            self.free_list.push(index);
        }
    }

    /// Get the total number of live objects.
    pub fn live_count(&self) -> usize {
        self.objects.iter().filter(|o| o.is_some()).count()
    }
}
