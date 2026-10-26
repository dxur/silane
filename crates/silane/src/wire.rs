use crate::circuit::Circuit;
use std::{marker::PhantomData, ops::Deref};

/// Typestate marker for an unbound (unassigned driver) wire.
/// Cannot be copied freely (single driver enforcement).
pub struct UnBound;

/// Typestate marker for a bound (readable net) wire.
/// Implements `Copy` so it can be tapped into multiple primitive blocks.
pub struct Bound;

/// `Wire<T, N>` is the pure topology net handle carrying runtime payload `value: T`.
/// It carries a node ID in the circuit graph, the elaboration that graph belongs to,
/// and the signal value representation.
///
/// `repr(C)`: an unbound wire is reinterpreted as a bound one (see `Deref`), so both
/// must have the same layout.
#[derive(Debug)]
#[repr(C)]
pub struct Wire<T, N = Bound>
where
    N: 'static,
{
    pub(crate) node: usize,
    pub(crate) circuit: u32,
    pub value: T,
    pub(crate) _marker: PhantomData<N>,
}

impl<T, N> Wire<T, N>
where
    N: 'static,
{
    /// Constructs a `Wire` of the active elaboration from a raw circuit node ID and
    /// payload value.
    pub fn new_dirty(node: usize, value: T) -> Self {
        Wire {
            node,
            circuit: Circuit::current_id(),
            value,
            _marker: PhantomData,
        }
    }

    /// Returns the raw circuit node ID of this wire.
    ///
    /// # Panics
    /// During an elaboration other than the one that created the wire: its node ID
    /// would name an unrelated net.
    pub fn node_dirty(&self) -> usize {
        let current = Circuit::current_id();
        assert!(
            current == 0 || current == self.circuit,
            "wire of another elaboration used here: wires cannot be shared between circuits"
        );
        self.node
    }
}

impl<T: Clone> Clone for Wire<T, Bound> {
    fn clone(&self) -> Self {
        Wire {
            node: self.node,
            circuit: self.circuit,
            value: self.value.clone(),
            _marker: PhantomData,
        }
    }
}

// Bound wires represent readable physical nets: trivial Copy if T: Copy!
impl<T: Copy> Copy for Wire<T, Bound> {}

impl<T: Clone> Wire<T, UnBound> {
    /// Obtains a readable `Bound` wire handle for this net.
    pub fn read(&self) -> Wire<T, Bound> {
        Wire {
            node: self.node,
            circuit: self.circuit,
            value: self.value.clone(),
            _marker: PhantomData,
        }
    }
}

impl<T: Clone> From<&Wire<T, UnBound>> for Wire<T, Bound> {
    fn from(val: &Wire<T, UnBound>) -> Self {
        val.read()
    }
}

impl<T> Deref for Wire<T, UnBound> {
    type Target = Wire<T, Bound>;

    fn deref(&self) -> &Self::Target {
        unsafe { &*(self as *const Wire<T, UnBound> as *const Wire<T, Bound>) }
    }
}