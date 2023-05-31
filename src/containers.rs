use crate::node::{Active, ActiveArr};
use core::marker::PhantomData;
use core::mem::MaybeUninit;
use core::ptr::{addr_of, addr_of_mut, drop_in_place};
use core::slice::{from_raw_parts, from_raw_parts_mut};
use core::sync::atomic::{AtomicUsize, Ordering};
use core::{
    fmt,
    mem::forget,
    ops::{Deref, DerefMut},
    ptr::NonNull,
};

/// An Anachro Heap Box Type
pub struct HeapBox<T> {
    pub(crate) ptr: NonNull<Active<T>>,
    pub(crate) pd: PhantomData<Active<T>>,
}

pub(crate) struct ArcInner<T> {
    pub(crate) data: T,
    pub(crate) refcnt: AtomicUsize,
}

pub struct HeapArc<T> {
    pub(crate) ptr: NonNull<Active<ArcInner<T>>>,
    pub(crate) pd: PhantomData<Active<ArcInner<T>>>,
}

/// An Anachro Heap Array Type
pub struct HeapArray<T> {
    pub(crate) ptr: NonNull<ActiveArr<T>>,
    pub(crate) pd: PhantomData<Active<T>>,
}

/// An Anachro Heap Array Type
pub struct HeapFixedVec<T> {
    pub(crate) ptr: NonNull<ActiveArr<MaybeUninit<T>>>,
    pub(crate) len: usize,
    pub(crate) pd: PhantomData<Active<MaybeUninit<T>>>,
}

// === impl HeapBox ===

unsafe impl<T: Send> Send for HeapBox<T> {}
unsafe impl<T: Sync> Sync for HeapBox<T> {}

impl<T> Deref for HeapBox<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        unsafe { &*Active::<T>::data(self.ptr).as_ptr() }
    }
}

impl<T> DerefMut for HeapBox<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { &mut *Active::<T>::data(self.ptr).as_ptr() }
    }
}

impl<T> HeapBox<T> {
    pub unsafe fn from_leaked(ptr: NonNull<T>) -> Self {
        Self {
            ptr: Active::<T>::from_leaked_ptr(ptr),
            pd: PhantomData,
        }
    }

    /// Leak the contents of this box, never to be recovered (probably)
    pub fn leak(self) -> NonNull<T> {
        let nn = unsafe { Active::<T>::data(self.ptr) };
        forget(self);
        nn
    }
}

impl<T> Drop for HeapBox<T> {
    fn drop(&mut self) {
        unsafe {
            let item_ptr = Active::<T>::data(self.ptr).as_ptr();
            drop_in_place(item_ptr);
            Active::<T>::yeet(self.ptr);
        }
    }
}

// === impl ArcInner ===

impl<T> ArcInner<T> {
    pub unsafe fn from_leaked_ptr(data: NonNull<T>) -> NonNull<ArcInner<T>> {
        let ptr = data
            .cast::<u8>()
            .as_ptr()
            .offset(Self::data_offset())
            .cast::<ArcInner<T>>();
        NonNull::new_unchecked(ptr)
    }

    #[inline(always)]
    fn data_offset() -> isize {
        let dummy: ArcInner<MaybeUninit<T>> = ArcInner {
            data: MaybeUninit::uninit(),
            refcnt: AtomicUsize::new(0),
        };
        let dummy_ptr: *const ArcInner<MaybeUninit<T>> = &dummy;
        let data_ptr = unsafe { addr_of!((*dummy_ptr).data) };
        unsafe { dummy_ptr.cast::<u8>().offset_from(data_ptr.cast::<u8>()) }
    }
}

// === impl HeapArc ===

// These require the same bounds as `alloc::sync::Arc`'s `Send` and `Sync`
// impls.
unsafe impl<T: Send + Sync> Send for HeapArc<T> {}
unsafe impl<T: Send + Sync> Sync for HeapArc<T> {}

impl<T> HeapArc<T> {
    /// Leak the contents of this box, never to be recovered (probably)
    pub fn leak(self) -> NonNull<T> {
        unsafe {
            let nn = Active::<ArcInner<T>>::data(self.ptr);
            forget(self);
            let data_ptr = addr_of_mut!((*nn.as_ptr()).data);
            NonNull::new_unchecked(data_ptr)
        }
    }

    /// Create a clone of a given leaked HeapArc<T>. DOES increase the refcount.
    pub unsafe fn clone_from_leaked(ptr: NonNull<T>) -> Self {
        let new = Self::from_leaked(ptr);

        let aitem_nn = Active::<ArcInner<T>>::data(new.ptr);
        aitem_nn.as_ref().refcnt.fetch_add(1, Ordering::SeqCst);

        new
    }

    /// Re-takes ownership of a leaked HeapArc<T>. Does NOT increase the refcount.
    pub unsafe fn from_leaked(ptr: NonNull<T>) -> Self {
        let arc_inner_nn: NonNull<ArcInner<T>> = ArcInner::from_leaked_ptr(ptr);
        Self {
            ptr: Active::<ArcInner<T>>::from_leaked_ptr(arc_inner_nn),
            pd: PhantomData,
        }
    }

    pub unsafe fn increment_count(ptr: NonNull<T>) {
        let arc_inner_nn: NonNull<ArcInner<T>> = ArcInner::from_leaked_ptr(ptr);
        arc_inner_nn.as_ref().refcnt.fetch_add(1, Ordering::SeqCst);
    }
}

impl<T> Deref for HeapArc<T> {
    type Target = T;

    #[inline(always)]
    fn deref(&self) -> &Self::Target {
        unsafe {
            let aiptr: *mut ArcInner<T> = Active::<ArcInner<T>>::data(self.ptr).as_ptr();
            let dptr: *const T = addr_of!((*aiptr).data);
            &*dptr
        }
    }
}

impl<T> Drop for HeapArc<T> {
    fn drop(&mut self) {
        unsafe {
            let (aiptr, needs_drop) = {
                let aitem_ptr = Active::<ArcInner<T>>::data(self.ptr).as_ptr();
                let old = (*aitem_ptr).refcnt.fetch_sub(1, Ordering::SeqCst);
                debug_assert_ne!(old, 0);
                (aitem_ptr, old == 1)
            };

            if needs_drop {
                drop_in_place(aiptr);
                Active::<ArcInner<T>>::yeet(self.ptr);
            }
        }
    }
}

impl<T> Clone for HeapArc<T> {
    fn clone(&self) -> Self {
        unsafe {
            let aitem_nn = Active::<ArcInner<T>>::data(self.ptr);
            aitem_nn.as_ref().refcnt.fetch_add(1, Ordering::SeqCst);

            HeapArc {
                ptr: self.ptr,
                pd: PhantomData,
            }
        }
    }
}

impl<T: fmt::Display> fmt::Display for HeapArc<T> {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&**self, f)
    }
}

impl<T: fmt::Debug> fmt::Debug for HeapArc<T> {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&**self, f)
    }
}

impl<T> fmt::Pointer for HeapArc<T> {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Pointer::fmt(&self.ptr, f)
    }
}

// === impl HeapArray ===

unsafe impl<T: Send> Send for HeapArray<T> {}
unsafe impl<T: Sync> Sync for HeapArray<T> {}

impl<T> Deref for HeapArray<T> {
    type Target = [T];

    fn deref(&self) -> &Self::Target {
        unsafe {
            let (nn_ptr, count) = ActiveArr::<T>::data(self.ptr);
            from_raw_parts(nn_ptr.as_ptr(), count)
        }
    }
}

impl<T> DerefMut for HeapArray<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe {
            let (nn_ptr, count) = ActiveArr::<T>::data(self.ptr);
            from_raw_parts_mut(nn_ptr.as_ptr(), count)
        }
    }
}

impl<T> HeapArray<T> {
    // pub unsafe fn from_leaked(ptr: *mut T, count: usize) -> Self {
    //     Self { ptr, count }
    // }

    /// Leak the contents of this box, never to be recovered (probably)
    pub fn leak(self) -> (NonNull<T>, usize) {
        unsafe {
            let (nn_ptr, count) = ActiveArr::<T>::data(self.ptr);
            forget(self);
            (nn_ptr, count)
        }
    }
}

impl<T> Drop for HeapArray<T> {
    fn drop(&mut self) {
        unsafe {
            let (start, count) = ActiveArr::<T>::data(self.ptr);
            let start = start.as_ptr();
            for i in 0..count {
                drop_in_place(start.add(i));
            }
            ActiveArr::<T>::yeet(self.ptr);
        }
    }
}

impl<T> fmt::Debug for HeapArray<T>
where
    [T]: fmt::Debug,
{
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&**self, f)
    }
}

impl<T> fmt::Pointer for HeapArray<T> {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Pointer::fmt(&self.ptr, f)
    }
}

// === impl HeapFixedVec ===

unsafe impl<T: Send> Send for HeapFixedVec<T> {}
unsafe impl<T: Sync> Sync for HeapFixedVec<T> {}

impl<T> Deref for HeapFixedVec<T> {
    type Target = [T];

    fn deref(&self) -> &Self::Target {
        unsafe {
            let (nn_ptr, _count) = ActiveArr::<MaybeUninit<T>>::data(self.ptr);
            from_raw_parts(nn_ptr.as_ptr().cast::<T>(), self.len)
        }
    }
}

impl<T> DerefMut for HeapFixedVec<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe {
            let (nn_ptr, _count) = ActiveArr::<MaybeUninit<T>>::data(self.ptr);
            from_raw_parts_mut(nn_ptr.as_ptr().cast::<T>(), self.len)
        }
    }
}

impl<T> HeapFixedVec<T> {
    pub fn push(&mut self, item: T) -> Result<(), T> {
        let (nn_ptr, count) = unsafe { ActiveArr::<MaybeUninit<T>>::data(self.ptr) };
        if count == self.len {
            return Err(item);
        }
        unsafe {
            nn_ptr.as_ptr().cast::<T>().add(self.len).write(item);
            self.len += 1;
        }
        Ok(())
    }

    pub fn is_full(&self) -> bool {
        let (_nn_ptr, count) = unsafe { ActiveArr::<MaybeUninit<T>>::data(self.ptr) };
        count == self.len
    }
}

impl<T> Drop for HeapFixedVec<T> {
    fn drop(&mut self) {
        unsafe {
            let (start, _count) = ActiveArr::<MaybeUninit<T>>::data(self.ptr);
            let start = start.as_ptr().cast::<T>();
            for i in 0..self.len {
                drop_in_place(start.add(i));
            }
            ActiveArr::<MaybeUninit<T>>::yeet(self.ptr);
        }
    }
}

impl<T> fmt::Debug for HeapFixedVec<T>
where
    [T]: fmt::Debug,
{
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&**self, f)
    }
}

impl<T> fmt::Pointer for HeapFixedVec<T> {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Pointer::fmt(&self.ptr, f)
    }
}
