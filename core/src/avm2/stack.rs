//! Internal representation of the AVM2 value stack

use crate::avm2::method::Method;
use crate::avm2::value::Value;

use gc_arena::collect::Trace;
use gc_arena::{Collect, Gc, Mutation};
use std::cell::Cell;
use std::marker::PhantomData;
use std::mem::MaybeUninit;
use std::ptr::NonNull;

const PREALLOCATED_STACK_SIZE: usize = 200000;

/// The global, preallocated value stack. This has little use directly due to
/// the way AVM2 works; to do anything with it, a StackFrame must be
/// obtained first through the `get_stack_frame` method.
///
/// We use this instead of a `Vec<Value>` to allow for obtaining mutable references
/// to the stack without requiring a mutable borrow on the stack or the `Avm2`.
#[derive(Clone, Collect, Copy)]
#[collect(no_drop)]
pub struct Stack<'gc>(Gc<'gc, StackData<'gc>>);

struct StackData<'gc> {
    /// Stack data
    stack: NonNull<[MaybeUninit<Value<'gc>>]>,

    /// The number of values currently stored on the stack
    stack_pointer: Cell<usize>,
}

impl<'gc> Drop for StackData<'gc> {
    fn drop(&mut self) {
        // SAFETY: `self.stack` originally came from a Box; this releases the memory.
        let _ = unsafe { Box::from_raw(self.stack.as_ptr()) };
    }
}

unsafe impl<'gc> Collect<'gc> for StackData<'gc> {
    // SAFETY: The only way to access values on the stack is by using StackFrames
    // obtained from get_stack_frame. StackFrame doesn't implement Collect, so
    // StackFrames from before a collection can't be accessed after the collection.
    fn trace<C: Trace<'gc>>(&self, _cc: &mut C) {
        // There should be no values on the value stack when collection is triggered
        assert!(self.stack_pointer.get() == 0);
    }
}

impl<'gc> Stack<'gc> {
    pub fn new(mc: &Mutation<'gc>) -> Self {
        let stack = Box::new_uninit_slice(PREALLOCATED_STACK_SIZE);

        Stack(Gc::new(
            mc,
            StackData {
                // SAFETY: this is `Box::into_non_null`.
                stack: unsafe { NonNull::new_unchecked(Box::into_raw(stack)) },
                stack_pointer: Cell::new(0),
            },
        ))
    }

    /// Returns a slice of stack data for the specified method, starting at the
    /// current stack pointer. Stack frames obtained from this method must be
    /// properly disposed of by using the `dispose_stack_frame` method.
    pub fn get_stack_frame(&self, method: Method<'gc>) -> StackFrame<'_, 'gc> {
        // First calculate the frame size
        let body = method
            .body()
            .expect("Cannot execute non-native method without body");

        let start_offset = self.0.stack_pointer.get();
        let Some(frame_size) = (body.max_stack as usize)
            .checked_add(body.num_locals as usize)
            .filter(|size| start_offset.saturating_add(*size) <= self.0.stack.len())
        else {
            panic!("AVM2 value stack exhausted");
        };

        // Then actually allocate the stack frame
        // SAFETY:
        // - the check above ensures this doesn't go past the end of the main stack allocation;
        // - this doesn't access anything below `start_offset`;
        // - the check in `dispose_stack_frame` thus guarantees that this slice will stay
        //   disjoint from other live stack frames;
        // - this is a slice of MaybeUninit values, so there's no need to initialize anything.
        let slice = unsafe {
            let ptr = self.0.stack.cast::<MaybeUninit<_>>().add(start_offset);
            NonNull::slice_from_raw_parts(ptr, frame_size).as_mut()
        };

        // Bump the stack pointer by the size of the newly-created frame.
        self.0
            .stack_pointer
            .set(self.0.stack_pointer.get() + frame_size);

        StackFrame::from_raw(slice)
    }

    pub fn dispose_stack_frame(self, stack_frame: StackFrame<'_, 'gc>) {
        let slice = stack_frame.into_raw();
        let range = slice.as_ptr_range();

        let cur = self.0.stack_pointer.get();
        if self.0.stack.addr().get() == range.end.wrapping_sub(cur).addr() {
            let len = slice.len();
            // SAFETY: We've checked that the stack frame we've disposing of was the latest stack
            // frame we gave out; so we can recover its memory by resetting the stack pointer.
            self.0.stack_pointer.set(cur - len);
        } else {
            panic!("failed to dispose of stack frames in stack order");
        }
    }
}

/// A stack frame for a particular method. Despite its name, this stores both
/// method locals and method stack.
///
/// Somewhat like `Vec<Value<'gc>>`, but with the following important differences:
/// - doesn't own its allocation;
/// - no shared access to its contents, only mutable or by-value;
/// - in return, allows `pop`ping elements through `&self`.
pub struct StackFrame<'a, 'gc> {
    // Safety invariants:
    // - `ptr` points to a mutable slice with `capacity` elements;
    // - `len <= capacity`;
    // - elements in `0..len` are initialized;
    // - elements in `len..capacity` are either uninitialized or 'loaned out' to the caller;
    // - `len` can decrease through `&self`, but requires `&mut self` to increase.
    _marker: PhantomData<&'a mut [Value<'gc>]>,
    ptr: NonNull<Value<'gc>>,
    len: Cell<usize>,
    capacity: usize,
}

impl<'a, 'gc> StackFrame<'a, 'gc> {
    #[inline]
    pub fn empty() -> StackFrame<'a, 'gc> {
        Self::from_raw(&mut [])
    }

    #[inline]
    pub fn from_raw(raw: &'a mut [MaybeUninit<Value<'gc>>]) -> Self {
        Self {
            _marker: PhantomData,
            len: Cell::new(0),
            capacity: raw.len(),
            ptr: NonNull::from(raw).cast(),
        }
    }

    #[inline]
    pub fn into_raw(self) -> &'a mut [MaybeUninit<Value<'gc>>] {
        let mut ptr = NonNull::slice_from_raw_parts(self.ptr.cast(), self.capacity);
        // SAFETY: This is the converse of `from_raw`.
        // Logically forgets the contents, which is fine as elements are `Copy`.
        unsafe { ptr.as_mut() }
    }

    /// Get the number of entries currently on the stack.
    #[inline(always)]
    pub fn len(&self) -> usize {
        self.len.get()
    }

    /// Push a value onto the operand stack.
    #[inline]
    pub fn push(&mut self, value: Value<'gc>) {
        let len = self.len();
        if len >= self.capacity {
            Self::panic_stack_overflow()
        }
        // SAFETY:
        // - The check above guarantees the validity of the ptr offset;
        // - We have mutable access, so we're free to initialize a new element.
        unsafe {
            self.ptr.add(len).write(value);
            self.len.set(len + 1);
        }
    }

    #[inline]
    pub fn as_slice_mut(&mut self) -> &'a mut [Value<'gc>] {
        let mut ptr = NonNull::slice_from_raw_parts(self.ptr, self.len());
        // SAFETY: We have mutable access, and elements up to `len` are initialized.
        unsafe { ptr.as_mut() }
    }

    #[inline]
    pub fn value_at(&self, index: usize) -> Value<'gc> {
        assert!(index < self.len());
        // SAFETY:
        // - The check above guarantees the element exists and is initialized;
        // - Elements are `Copy`;
        // - We don't create any reference that could conflict with `pop` operations.
        unsafe { self.ptr.add(index).read() }
    }

    #[inline]
    pub fn set_value_at(&mut self, index: usize, value: Value<'gc>) {
        self.as_slice_mut()[index] = value;
    }

    /// Peek the n-th value from the end of the operand stack.
    #[inline]
    pub fn peek(&self, index: usize) -> Value<'gc> {
        self.value_at(self.len() - index - 1)
    }

    #[inline]
    pub fn stack_top(&mut self) -> &'a mut Value<'gc> {
        &mut self.as_slice_mut()[self.len() - 1]
    }

    #[inline]
    pub fn truncate(&self, size: usize) {
        if self.len() >= size {
            // SAFETY:
            // - Per self's invariant, decreasing `len` through `&self` is allowed;
            // - Logically forgets elements in `size..len`, which is fine as they are `Copy`.
            self.len.set(size);
        } else {
            Self::panic_stack_underflow();
        }
    }

    #[inline]
    fn decrement_len(&self, n: usize) -> usize {
        let Some(new_len) = self.len().checked_sub(n) else {
            Self::panic_stack_underflow()
        };
        // SAFETY: Per self's invariant, decreasing `len` through `&self` is allowed.
        self.len.set(new_len);
        new_len
    }

    /// Pops a value off the operand stack.
    #[inline]
    pub fn pop(&self) -> Value<'gc> {
        let offset = self.decrement_len(1);
        // SAFETY: `decrement_len` gave us ownership of the top-most element,
        // which we return to the caller.
        unsafe { self.ptr.add(offset).read() }
    }

    /// Pops multiple values off the operand stack.
    #[inline]
    #[expect(clippy::mut_from_ref)]
    pub fn pop_slice(&self, n: usize) -> &mut [Value<'gc>] {
        let offset = self.decrement_len(n);
        // SAFETY:
        // - `decrement_len` gave us ownership of the `n` top-most elements;
        // - Up until the next `push` call, which will terminate the returned slice's lifetime;
        // - Elements are `Copy`, so there is no need to drop them.
        unsafe {
            let ptr = self.ptr.add(offset);
            NonNull::slice_from_raw_parts(ptr, n).as_mut()
        }
    }

    #[cold]
    #[inline(never)]
    fn panic_stack_overflow() -> ! {
        panic!("stack overflow")
    }

    #[cold]
    #[inline(never)]
    fn panic_stack_underflow() -> ! {
        panic!("stack underflow")
    }
}
