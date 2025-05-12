icmsg-rs
========

A Rust implementation of the ICMsg IPC backend from the Zephyr Project with
support for async/await. Primarily intended for communication between two cores
in an embedded device, ICMsg is very simple, just two ringbuffers in shared
memory.
