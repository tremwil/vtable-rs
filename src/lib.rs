#![no_std]

use core::ops::{Deref, DerefMut};

pub use proc_macros::vtable;

pub trait VmtInstance<T: 'static>: 'static + VmtLayout {
    const VTABLE: &'static Self::Layout<T>;
}
pub unsafe trait VmtLayout: 'static {
    type Layout<T: 'static>: 'static + Copy;
}

#[repr(C)]
pub struct VPtr<V: VmtLayout + ?Sized, T: 'static>(&'static <V as VmtLayout>::Layout<T>);

impl<V: VmtLayout + ?Sized, T: 'static> Deref for VPtr<V, T> {
    type Target = &'static <V as VmtLayout>::Layout<T>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<V: VmtLayout + ?Sized, T: 'static> DerefMut for VPtr<V, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl<T, V: VmtInstance<T> + ?Sized> Default for VPtr<V, T> {
    fn default() -> Self {
        VPtr(vmt_instance::<V, T>())
    }
}

pub const fn vmt_instance<V: VmtInstance<T> + ?Sized, T>() -> &'static V::Layout<T> {
    V::VTABLE
}
