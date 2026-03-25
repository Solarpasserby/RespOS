// os/src/sync/up.rs

use core::any::type_name;
use core::cell::{RefCell, RefMut};

pub struct UPSafeCell<T> {
    /// inner data
    inner: RefCell<T>,
}

unsafe impl<T> Sync for UPSafeCell<T> {}

impl<T> UPSafeCell<T> {
    /// User is responsible to guarantee that inner struct is only used in
    /// uniprocessor.
    pub unsafe fn new(value: T) -> Self {
        Self {
            inner: RefCell::new(value),
        }
    }
    /// Exclusive access inner data in UPSafeCell. Panic if the data has been borrowed.
    pub fn exclusive_access(&self) -> RefMut<'_, T> {
        self.inner
            .try_borrow_mut()
            .unwrap_or_else(|_| panic!("[kernel] UPSafeCell reborrowed: {}", type_name::<T>()))
    }
}

