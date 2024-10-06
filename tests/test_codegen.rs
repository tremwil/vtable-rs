use vtable::{vtable, VPtr};

#[vtable]
pub trait Base {
    fn a(&self, arg1: u32) -> bool {
        false
    }
    unsafe fn b(&self, arg1: *const u8, arg2: u32) -> bool;
}

#[vtable]
pub(crate) trait Derived: Base {
    fn c(&mut self) -> usize;
}

#[derive(Default)]
struct DerivedImpl {
    // VPtr<dyn T, Self: T> supports `Default`, so your compile-time generated vtable
    // can be automatically provided to the object
    vftable: VPtr<dyn Derived, Self>,
}

impl Base for DerivedImpl {
    extern "C" fn a(&self, arg1: u32) -> bool {
        arg1 == 42
    }

    unsafe extern "C" fn b(&self, _arg1: *const u8, _arg2: u32) -> bool {
        false
    }
}

impl Derived for DerivedImpl {
    extern "C" fn c(&mut self) -> usize {
        1234
    }
}

#[test]
fn default_derived_impls_correct() {
    let mut d = DerivedImpl::default();

    assert_eq!(d.a(42), d.vftable.a(&d, 42));

    unsafe {
        let s = b"abc";
        let ptr = s.as_ptr();
        assert_eq!(d.b(ptr, 420), d.vftable.b(&d, ptr, 420));
    }

    assert_eq!(d.c(), d.vftable.c(&mut d))
}
