//! Re-exports loom types

#[cfg(loom)]
pub(crate) use {
    loom::alloc,
    loom::sync,
    loom::thread,
};

#[cfg(all(not(loom), not(test)))]
pub(crate) use core::sync;

#[cfg(all(not(loom), test))]
extern crate std;
#[cfg(all(not(loom), test))]
pub(crate) use {
    std::sync,
    std::alloc,
    std::thread,
};
