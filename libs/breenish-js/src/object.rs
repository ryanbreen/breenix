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
#[derive(Debug, Clone, PartialEq)]
pub enum ObjectKind {
    /// A plain object `{}`.
    Ordinary,
    /// An array object `[]`.
    Array,
    /// A function object (index into function table).
    Function(u32),
    /// A closure (function index + captured upvalue values).
    Closure(u32, Vec<JsValue>),
    /// A native function (index into native function table).
    NativeFunction(u32),
    /// A Promise with its fulfillment state.
    Promise(PromiseState),
    /// A Map object (ordered key-value pairs).
    Map(Vec<(JsValue, JsValue)>),
    /// A Set object (ordered unique values).
    Set(Vec<JsValue>),
    /// A command function resolved from PATH (stores the command name).
    CommandFunction(StringId),
}

/// The state of a Promise.
#[derive(Debug, Clone, PartialEq)]
pub enum PromiseState {
    /// The promise is fulfilled with a value.
    Fulfilled(JsValue),
    /// The promise is rejected with a reason.
    Rejected(JsValue),
    /// The promise is pending (not yet resolved).
    Pending,
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
    /// GC mark bit for mark-sweep collection.
    pub marked: bool,
}

impl JsObject {
    /// Create a new ordinary object.
    pub fn new() -> Self {
        Self {
            kind: ObjectKind::Ordinary,
            properties: Vec::new(),
            elements: Vec::new(),
            prototype: None,
            marked: false,
        }
    }

    /// Create a new array object.
    pub fn new_array() -> Self {
        Self {
            kind: ObjectKind::Array,
            properties: Vec::new(),
            elements: Vec::new(),
            prototype: None,
            marked: false,
        }
    }

    /// Create a new function object.
    pub fn new_function(func_index: u32) -> Self {
        Self {
            kind: ObjectKind::Function(func_index),
            properties: Vec::new(),
            elements: Vec::new(),
            prototype: None,
            marked: false,
        }
    }

    /// Create a new fulfilled Promise object.
    pub fn new_promise_fulfilled(value: JsValue) -> Self {
        Self {
            kind: ObjectKind::Promise(PromiseState::Fulfilled(value)),
            properties: Vec::new(),
            elements: Vec::new(),
            prototype: None,
            marked: false,
        }
    }

    /// Create a new rejected Promise object.
    pub fn new_promise_rejected(reason: JsValue) -> Self {
        Self {
            kind: ObjectKind::Promise(PromiseState::Rejected(reason)),
            properties: Vec::new(),
            elements: Vec::new(),
            prototype: None,
            marked: false,
        }
    }

    /// Create a new pending Promise object.
    pub fn new_promise_pending() -> Self {
        Self {
            kind: ObjectKind::Promise(PromiseState::Pending),
            properties: Vec::new(),
            elements: Vec::new(),
            prototype: None,
            marked: false,
        }
    }

    /// Create a new Map object.
    pub fn new_map() -> Self {
        Self {
            kind: ObjectKind::Map(Vec::new()),
            properties: Vec::new(),
            elements: Vec::new(),
            prototype: None,
            marked: false,
        }
    }

    /// Create a new Set object.
    pub fn new_set() -> Self {
        Self {
            kind: ObjectKind::Set(Vec::new()),
            properties: Vec::new(),
            elements: Vec::new(),
            prototype: None,
            marked: false,
        }
    }

    /// Create a new native function object.
    pub fn new_native_function(native_index: u32) -> Self {
        Self {
            kind: ObjectKind::NativeFunction(native_index),
            properties: Vec::new(),
            elements: Vec::new(),
            prototype: None,
            marked: false,
        }
    }

    /// Create a new command function object (PATH-resolved executable).
    pub fn new_command_function(name_sid: StringId) -> Self {
        Self {
            kind: ObjectKind::CommandFunction(name_sid),
            properties: Vec::new(),
            elements: Vec::new(),
            prototype: None,
            marked: false,
        }
    }

    /// Create a new closure object (function + captured upvalue values).
    pub fn new_closure(func_index: u32, upvalues: Vec<JsValue>) -> Self {
        Self {
            kind: ObjectKind::Closure(func_index, upvalues),
            properties: Vec::new(),
            elements: Vec::new(),
            prototype: None,
            marked: false,
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

    /// Re-key all properties using the given string pool.
    ///
    /// For each existing property, looks up the original key string in `old_pool`,
    /// interns it in `new_pool`, and updates the stored key.
    pub fn rekey_properties(&mut self, old_pool: &crate::string::StringPool, new_pool: &mut crate::string::StringPool) {
        for prop in &mut self.properties {
            let name = old_pool.get(prop.key);
            prop.key = new_pool.intern(name);
        }
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

    /// Iterate over all JsValues referenced by this object (for GC tracing).
    pub fn referenced_values(&self) -> Vec<JsValue> {
        let mut refs = Vec::new();
        for prop in &self.properties {
            refs.push(prop.value);
        }
        for elem in &self.elements {
            refs.push(*elem);
        }
        match &self.kind {
            ObjectKind::Closure(_, ref upvalues) => {
                for uv in upvalues {
                    refs.push(*uv);
                }
            }
            ObjectKind::Promise(PromiseState::Fulfilled(v)) => refs.push(*v),
            ObjectKind::Promise(PromiseState::Rejected(v)) => refs.push(*v),
            ObjectKind::Map(ref entries) => {
                for (k, v) in entries {
                    refs.push(*k);
                    refs.push(*v);
                }
            }
            ObjectKind::Set(ref values) => {
                for v in values {
                    refs.push(*v);
                }
            }
            _ => {}
        }
        refs
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

    /// Clear all mark bits in preparation for a mark phase.
    pub fn unmark_all(&mut self) {
        for slot in &mut self.objects {
            if let Some(obj) = slot {
                obj.marked = false;
            }
        }
    }

    /// Mark an object as reachable and recursively mark all objects it references.
    /// Returns without action if the object is already marked or doesn't exist.
    pub fn mark(&mut self, index: u32) {
        let idx = index as usize;
        if idx >= self.objects.len() {
            return;
        }
        if let Some(ref obj) = self.objects[idx] {
            if obj.marked {
                return;
            }
        } else {
            return;
        }

        // Mark this object
        self.objects[idx].as_mut().unwrap().marked = true;

        // Collect values that reference other objects, then mark them
        let refs = self.objects[idx].as_ref().unwrap().referenced_values();
        if let Some(proto) = self.objects[idx].as_ref().unwrap().prototype {
            self.mark(proto);
        }
        for val in refs {
            if val.is_object() {
                self.mark(val.as_object_index());
            }
        }
    }

    /// Sweep phase: free all unmarked objects and return the number freed.
    pub fn sweep(&mut self) -> usize {
        let mut freed = 0;
        for i in 0..self.objects.len() {
            if let Some(ref obj) = self.objects[i] {
                if !obj.marked {
                    self.objects[i] = None;
                    self.free_list.push(i as u32);
                    freed += 1;
                }
            }
        }
        freed
    }
}
