//! Is the C runtime linked into the Windows build?
//!
//! The released plug-in imported VCRUNTIME140.dll and VCRUNTIME140_1.dll. Those
//! are not part of Windows, so on a computer without the Visual C++
//! Redistributable the DAW could not load it (LoadLibrary error 126). The fix is
//! `+crt-static` for the MSVC targets in .cargo/config.toml.
//!
//! The flag reaches every crate a build compiles for the target, this test as
//! much as the plug-in. It fails when the config entry is gone, and also when a
//! RUSTFLAGS environment variable is set: Cargo then ignores the rustflags in
//! .cargo/config.toml, and the C runtime is quietly dynamic again.

#![cfg(all(windows, target_env = "msvc"))]

#[test]
fn the_c_runtime_is_linked_statically() {
    assert!(
        cfg!(target_feature = "crt-static"),
        "built without +crt-static: the plug-in would import VCRUNTIME140.dll and fail to load \
         without the Visual C++ Redistributable. Check .cargo/config.toml and unset RUSTFLAGS."
    );
}
