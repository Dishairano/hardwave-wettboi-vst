//! The CLAP entry point, with the state extension fixed up.
//!
//! This module does what `nih_export_clap!(HardwaveWettBoi)` used to do, plus
//! two things the framework's `clap.state` implementation gets wrong. Both of
//! them are what the CLAP validator fails us on, and neither can be reached
//! from `Plugin::filter_state()`, because both happen before a single byte of
//! our state has been parsed.
//!
//! **A saved state's length prefix is trusted.** nih-plug writes the length of
//! its JSON in front of the JSON, because CLAP gives no way to ask a stream how
//! much is left in it. On load it reads those eight bytes and hands them
//! straight to `Vec::with_capacity`. Feed it a file that is not ours (which is
//! exactly what `state-invalid-random` does) and the first eight random bytes
//! ask for an exabyte, the allocator fails, and Rust aborts the whole process:
//! `memory allocation of 1074606175335821323 bytes failed`, SIGABRT, host and
//! all. A length is input like any other, so [`state_load`] reads the prefix
//! itself, refuses anything that cannot be one of our states, reads exactly
//! that many bytes, and only then replays the whole thing into the framework
//! from memory. The framework's own `with_capacity` then gets a length this
//! code has already bounded.
//!
//! **The host is never told the parameter values changed.** After
//! `clap_plugin_state.load()` the framework schedules `ParameterValuesChanged`,
//! which only notifies our own editor. It calls
//! `clap_host_params.rescan(CLAP_PARAM_RESCAN_VALUES)` on exactly one path, a
//! preset load started from the editor, never on a load started by the host.
//! CLAP requires a plugin that changes its own parameter values to say so, and
//! a state load changes all of them at once. Until the host rescans, its idea
//! of every control is whatever the fresh instance had, which is the default,
//! while the plugin is playing the restored value. That is the
//! `state-reproducibility-*` failure, and in a DAW it is a project whose
//! automation lanes and control displays disagree with what you hear.
//! [`state_load`] asks for the rescan once the load has succeeded.
//!
//! # How the shim is put together
//!
//! [`create_plugin`] builds the framework's plugin as usual and then hands the
//! host a [`Shim`] instead. The shim's `clap_plugin` forwards all eleven
//! methods to the framework's, except `get_extension`, which answers
//! `clap.state` with our own table and everything else with the framework's.
//!
//! The shim carries **the framework's own `plugin_data` pointer**, unchanged.
//! That is what makes this safe and small: every one of nih-plug's extension
//! functions reads its wrapper back out of `(*plugin).plugin_data` and looks at
//! no other field, so a framework extension called with the shim's plugin
//! pointer behaves exactly as it would have. Nothing else about the plugin is
//! wrapped, intercepted or reimplemented here.
//!
//! This is a patch over a framework fault, not a fix of it. The structural fix
//! is four lines in `nih-plug-hardwave`: bound the length in
//! `wrapper/clap/wrapper.rs::ext_state_load` before `Vec::with_capacity`, and
//! schedule `Task::RescanParamValues` at the end of the same function. When the
//! fork carries those, this whole module can go back to being one
//! `nih_export_clap!` line.

use std::ffi::{c_char, c_void, CStr};
use std::sync::{Arc, OnceLock};

use clap_sys::ext::params::{clap_host_params, CLAP_EXT_PARAMS, CLAP_PARAM_RESCAN_VALUES};
use clap_sys::ext::state::{clap_plugin_state, CLAP_EXT_STATE};
use clap_sys::process::{clap_process, clap_process_status, CLAP_PROCESS_ERROR};
use clap_sys::stream::{clap_istream, clap_ostream};
use nih_plug::wrapper::clap::{
    clap_host, clap_plugin, clap_plugin_descriptor, clap_plugin_entry, clap_plugin_factory,
    PluginDescriptor, Wrapper, CLAP_PLUGIN_FACTORY_ID, CLAP_VERSION,
};
use nih_plug::wrapper::setup_logger;

use crate::HardwaveWettBoi;

/// The largest saved state we will read.
///
/// WettBoi's state is a few kilobytes of JSON: one number per parameter, and no
/// persisted fields at all. Sixteen megabytes is a thousand times the room it
/// could ever need and still small enough that reading it can never exhaust
/// memory. Anything claiming to be larger is not a state this plugin wrote.
const MAX_STATE_BYTES: u64 = 16 * 1024 * 1024;

/// The length prefix nih-plug writes in front of its JSON.
const STATE_LENGTH_PREFIX_BYTES: usize = 8;

// ─── The shimmed plugin instance ─────────────────────────────────────────────

/// One plugin instance as the host sees it.
///
/// `plugin` has to stay the first field: the host is handed a pointer to it and
/// every callback below casts that pointer back to a `Shim`.
#[repr(C)]
struct Shim {
    /// What the host holds. Forwards to `inner`, except for `clap.state`.
    plugin: clap_plugin,
    /// The framework's plugin instance. Owns everything; destroyed first.
    inner: *const clap_plugin,
    /// The host, kept so a finished state load can ask it to rescan the values.
    host: *const clap_host,
    /// Our replacement `clap.state` table.
    state: clap_plugin_state,
}

impl Shim {
    /// Recover the shim from the plugin pointer the host was given.
    ///
    /// # Safety
    ///
    /// `plugin` must be a pointer this module's [`create_plugin`] returned and
    /// that has not been destroyed yet.
    unsafe fn from_plugin<'a>(plugin: *const clap_plugin) -> Option<&'a Shim> {
        if plugin.is_null() {
            return None;
        }
        // SAFETY: `Shim` is `#[repr(C)]` with `plugin` as its first field, so a
        // pointer to that field is a pointer to the `Shim`. The host only ever
        // receives pointers produced by `create_plugin`, and `destroy` is what
        // ends their lifetime.
        Some(&*(plugin as *const Shim))
    }

    /// Look up an extension on the framework's plugin.
    ///
    /// # Safety
    ///
    /// `self.inner` must still be alive, and `T` must be the type CLAP defines
    /// for `id`.
    unsafe fn inner_extension<T>(&self, id: &CStr) -> Option<*const T> {
        // SAFETY: `inner` points at the framework's `clap_plugin`, which lives
        // until `destroy` runs.
        let get_extension = (*self.inner).get_extension?;
        let extension = get_extension(self.inner, id.as_ptr()) as *const T;
        if extension.is_null() {
            None
        } else {
            Some(extension)
        }
    }
}

// ─── Reading a state stream ──────────────────────────────────────────────────

/// Fill `buffer` from `stream`, looping until it is full.
///
/// A CLAP stream may hand back fewer bytes than asked for; a host that reads
/// its project file in blocks does exactly that, which is what the validator's
/// `state-reproducibility-buffered` test imitates. Returns `false` if the
/// stream ends or errors before the buffer is full.
///
/// # Safety
///
/// `stream` must be a valid `clap_istream`.
unsafe fn read_exact(stream: *const clap_istream, buffer: &mut [u8]) -> bool {
    // SAFETY: the caller guarantees `stream` is valid.
    let read = match (*stream).read {
        Some(read) => read,
        None => return false,
    };

    let mut filled = 0usize;
    while filled < buffer.len() {
        let wanted = buffer.len() - filled;
        // SAFETY: `filled` is below `buffer.len()`, so this points inside the
        // buffer, and `wanted` is exactly the room left after it.
        let read_now = read(
            stream,
            buffer.as_mut_ptr().add(filled) as *mut c_void,
            wanted as u64,
        );
        // Zero is the end of the stream, negative is an error, and a stream
        // that claims to have written more than we asked for is not one we can
        // keep reading from.
        if read_now <= 0 || read_now as u64 > wanted as u64 {
            return false;
        }
        filled += read_now as usize;
    }

    true
}

/// A `clap_istream` over a byte slice we already hold.
struct Replay<'a> {
    data: &'a [u8],
    pos: usize,
}

/// The read function for [`Replay`].
///
/// # Safety
///
/// `stream`'s `ctx` must point at a live [`Replay`].
unsafe extern "C" fn replay_read(
    stream: *const clap_istream,
    buffer: *mut c_void,
    size: u64,
) -> i64 {
    if stream.is_null() || buffer.is_null() {
        return -1;
    }
    // SAFETY: `state_load` is the only caller and it sets `ctx` to a `Replay`
    // that outlives the inner load call.
    let replay = &mut *((*stream).ctx as *mut Replay);

    let left = replay.data.len() - replay.pos;
    let count = left.min(size as usize);
    if count > 0 {
        // SAFETY: `count` bytes are available from `pos` on, and the host
        // promises `buffer` has room for the `size` bytes it asked for.
        std::ptr::copy_nonoverlapping(replay.data[replay.pos..].as_ptr(), buffer as *mut u8, count);
        replay.pos += count;
    }

    count as i64
}

// ─── The state extension ─────────────────────────────────────────────────────

/// Saving needs no help; hand it straight to the framework.
///
/// # Safety
///
/// Called by the host with a plugin from [`create_plugin`] and a valid stream.
unsafe extern "C" fn state_save(plugin: *const clap_plugin, stream: *const clap_ostream) -> bool {
    let shim = match Shim::from_plugin(plugin) {
        Some(shim) => shim,
        None => return false,
    };
    if stream.is_null() {
        return false;
    }

    // SAFETY: `inner` is alive until `destroy`, and `clap.state` is the type
    // CLAP defines for this ID.
    let state: *const clap_plugin_state = match shim.inner_extension(CLAP_EXT_STATE) {
        Some(state) => state,
        None => return false,
    };
    // SAFETY: the extension pointer came from the framework and is non-null.
    match (*state).save {
        Some(save) => save(shim.inner, stream),
        None => false,
    }
}

/// Check the stream before the framework ever allocates for it, then tell the
/// host the parameter values moved.
///
/// The framework reads the eight byte length prefix and immediately reserves
/// that many bytes. This reads the prefix first, refuses a length that cannot
/// be one of ours, pulls in exactly that many bytes, and replays the prefix and
/// the body together from memory. The framework sees a well formed stream of a
/// size this function has already agreed to.
///
/// # Safety
///
/// Called by the host with a plugin from [`create_plugin`] and a valid stream.
unsafe extern "C" fn state_load(plugin: *const clap_plugin, stream: *const clap_istream) -> bool {
    let shim = match Shim::from_plugin(plugin) {
        Some(shim) => shim,
        None => return false,
    };
    if stream.is_null() {
        return false;
    }

    let mut prefix = [0u8; STATE_LENGTH_PREFIX_BYTES];
    // SAFETY: the host gave us this stream for the duration of the call.
    if !read_exact(stream, &mut prefix) {
        return false;
    }
    let length = u64::from_le_bytes(prefix);
    if length == 0 || length > MAX_STATE_BYTES {
        return false;
    }

    // The bytes themselves are not inspected here, and that is deliberate. What this function owes
    // the framework is a stream whose length it has already agreed to, which is the part that used
    // to abort the process: the framework read the prefix and reserved that many bytes before
    // anything could refuse it. Whether the body is valid UTF-8 or well formed JSON is the
    // deserializer's question, and it answers by returning an error rather than panicking; a value
    // that parses but is out of range is caught after that by filter_state in lib.rs. Checking it
    // twice here would mean a second opinion about our own format living in two places.
    //
    // The prefix and the body together, because that is what the framework
    // expects to read.
    let mut buffer = vec![0u8; STATE_LENGTH_PREFIX_BYTES + length as usize];
    buffer[..STATE_LENGTH_PREFIX_BYTES].copy_from_slice(&prefix);
    // SAFETY: same stream, still valid.
    if !read_exact(stream, &mut buffer[STATE_LENGTH_PREFIX_BYTES..]) {
        return false;
    }

    // SAFETY: `inner` is alive until `destroy`, and `clap.state` is the type
    // CLAP defines for this ID.
    let state: *const clap_plugin_state = match shim.inner_extension(CLAP_EXT_STATE) {
        Some(state) => state,
        None => return false,
    };
    // SAFETY: the extension pointer came from the framework and is non-null.
    let load = match (*state).load {
        Some(load) => load,
        None => return false,
    };

    let mut replay = Replay {
        data: &buffer,
        pos: 0,
    };
    let replay_stream = clap_istream {
        ctx: &mut replay as *mut Replay as *mut c_void,
        read: Some(replay_read),
    };
    // SAFETY: `replay` and `buffer` both outlive this call, and the framework
    // only reads from the stream while it runs.
    let loaded = load(shim.inner, &replay_stream);

    if loaded {
        // SAFETY: `state.load` is a main thread call and so is `rescan`, and
        // the framework has already finished with the plugin by now, so this
        // does not re-enter anything that is still holding a lock.
        request_param_rescan(shim.host);
    }

    loaded
}

/// Tell the host to re-read every parameter value.
///
/// A state load changes all of them at once and the host has no other way to
/// find out. Does nothing if the host has no parameters extension, which no
/// real host does but a test harness may.
///
/// # Safety
///
/// `host` must be the pointer the host passed to [`create_plugin`], and this
/// must run on the main thread.
unsafe fn request_param_rescan(host: *const clap_host) {
    if host.is_null() {
        return;
    }
    // SAFETY: the host outlives every plugin instance it created.
    let get_extension = match (*host).get_extension {
        Some(get_extension) => get_extension,
        None => return,
    };
    let params = get_extension(host, CLAP_EXT_PARAMS.as_ptr()) as *const clap_host_params;
    if params.is_null() {
        return;
    }
    // SAFETY: non-null, and `clap_host_params` is the type CLAP defines for
    // `clap.params` on the host side.
    if let Some(rescan) = (*params).rescan {
        rescan(host, CLAP_PARAM_RESCAN_VALUES);
    }
}

// ─── Forwarding the rest of the plugin ───────────────────────────────────────
//
// Each of these hands the call to the framework unchanged. None of them
// allocates, locks or can panic, which matters for `process` in particular: it
// runs on the audio thread, so a missing function pointer returns an error
// rather than unwrapping.

/// # Safety
///
/// Called by the host with a plugin from [`create_plugin`].
unsafe extern "C" fn shim_init(plugin: *const clap_plugin) -> bool {
    match Shim::from_plugin(plugin) {
        // SAFETY: `inner` is alive between `create_plugin` and `destroy`.
        Some(shim) => match (*shim.inner).init {
            Some(init) => init(shim.inner),
            None => false,
        },
        None => false,
    }
}

/// # Safety
///
/// Called by the host with a plugin from [`create_plugin`]. The pointer must
/// not be used again afterwards.
unsafe extern "C" fn shim_destroy(plugin: *const clap_plugin) {
    if plugin.is_null() {
        return;
    }
    // SAFETY: `create_plugin` produced this pointer with `Box::into_raw`, and
    // CLAP guarantees `destroy` is called at most once per instance.
    let shim = Box::from_raw(plugin as *mut Shim);

    // Read the function pointer before the call, because the call frees the
    // memory it lives in.
    // SAFETY: `inner` is still alive at this point.
    if let Some(destroy) = (*shim.inner).destroy {
        destroy(shim.inner);
    }
    // `shim` is dropped here, after the framework has let go of everything.
}

/// # Safety
///
/// Called by the host with a plugin from [`create_plugin`].
unsafe extern "C" fn shim_activate(
    plugin: *const clap_plugin,
    sample_rate: f64,
    min_frames_count: u32,
    max_frames_count: u32,
) -> bool {
    match Shim::from_plugin(plugin) {
        // SAFETY: `inner` is alive between `create_plugin` and `destroy`.
        Some(shim) => match (*shim.inner).activate {
            Some(activate) => activate(shim.inner, sample_rate, min_frames_count, max_frames_count),
            None => false,
        },
        None => false,
    }
}

/// # Safety
///
/// Called by the host with a plugin from [`create_plugin`].
unsafe extern "C" fn shim_deactivate(plugin: *const clap_plugin) {
    if let Some(shim) = Shim::from_plugin(plugin) {
        // SAFETY: `inner` is alive between `create_plugin` and `destroy`.
        if let Some(deactivate) = (*shim.inner).deactivate {
            deactivate(shim.inner);
        }
    }
}

/// # Safety
///
/// Called by the host with a plugin from [`create_plugin`].
unsafe extern "C" fn shim_start_processing(plugin: *const clap_plugin) -> bool {
    match Shim::from_plugin(plugin) {
        // SAFETY: `inner` is alive between `create_plugin` and `destroy`.
        Some(shim) => match (*shim.inner).start_processing {
            Some(start_processing) => start_processing(shim.inner),
            None => false,
        },
        None => false,
    }
}

/// # Safety
///
/// Called by the host with a plugin from [`create_plugin`].
unsafe extern "C" fn shim_stop_processing(plugin: *const clap_plugin) {
    if let Some(shim) = Shim::from_plugin(plugin) {
        // SAFETY: `inner` is alive between `create_plugin` and `destroy`.
        if let Some(stop_processing) = (*shim.inner).stop_processing {
            stop_processing(shim.inner);
        }
    }
}

/// # Safety
///
/// Called by the host with a plugin from [`create_plugin`].
unsafe extern "C" fn shim_reset(plugin: *const clap_plugin) {
    if let Some(shim) = Shim::from_plugin(plugin) {
        // SAFETY: `inner` is alive between `create_plugin` and `destroy`.
        if let Some(reset) = (*shim.inner).reset {
            reset(shim.inner);
        }
    }
}

/// # Safety
///
/// Called by the host on the audio thread with a plugin from
/// [`create_plugin`].
unsafe extern "C" fn shim_process(
    plugin: *const clap_plugin,
    process: *const clap_process,
) -> clap_process_status {
    match Shim::from_plugin(plugin) {
        // SAFETY: `inner` is alive between `create_plugin` and `destroy`.
        Some(shim) => match (*shim.inner).process {
            Some(process_fn) => process_fn(shim.inner, process),
            None => CLAP_PROCESS_ERROR,
        },
        None => CLAP_PROCESS_ERROR,
    }
}

/// Answer `clap.state` with our table and everything else with the framework's.
///
/// # Safety
///
/// Called by the host with a plugin from [`create_plugin`].
unsafe extern "C" fn shim_get_extension(
    plugin: *const clap_plugin,
    id: *const c_char,
) -> *const c_void {
    let shim = match Shim::from_plugin(plugin) {
        Some(shim) => shim,
        None => return std::ptr::null(),
    };
    if id.is_null() {
        return std::ptr::null();
    }

    // SAFETY: CLAP extension IDs are null-terminated C strings.
    if CStr::from_ptr(id) == CLAP_EXT_STATE {
        return &shim.state as *const clap_plugin_state as *const c_void;
    }

    // SAFETY: `inner` is alive between `create_plugin` and `destroy`. The
    // framework's extension tables read the plugin back out of `plugin_data`,
    // which the shim shares with it, so they work whichever of the two plugin
    // pointers the host later calls them with.
    match (*shim.inner).get_extension {
        Some(get_extension) => get_extension(shim.inner, id),
        None => std::ptr::null(),
    }
}

/// # Safety
///
/// Called by the host with a plugin from [`create_plugin`].
unsafe extern "C" fn shim_on_main_thread(plugin: *const clap_plugin) {
    if let Some(shim) = Shim::from_plugin(plugin) {
        // SAFETY: `inner` is alive between `create_plugin` and `destroy`.
        if let Some(on_main_thread) = (*shim.inner).on_main_thread {
            on_main_thread(shim.inner);
        }
    }
}

// ─── Factory and entry point ─────────────────────────────────────────────────

static PLUGIN_DESCRIPTOR: OnceLock<PluginDescriptor> = OnceLock::new();

fn plugin_descriptor() -> &'static PluginDescriptor {
    PLUGIN_DESCRIPTOR.get_or_init(PluginDescriptor::for_plugin::<HardwaveWettBoi>)
}

/// # Safety
///
/// Called by the host through the factory table.
unsafe extern "C" fn get_plugin_count(_factory: *const clap_plugin_factory) -> u32 {
    1
}

/// # Safety
///
/// Called by the host through the factory table.
unsafe extern "C" fn get_plugin_descriptor(
    _factory: *const clap_plugin_factory,
    index: u32,
) -> *const clap_plugin_descriptor {
    if index == 0 {
        plugin_descriptor().clap_plugin_descriptor()
    } else {
        std::ptr::null()
    }
}

/// Build the framework's plugin, then wrap it in a [`Shim`].
///
/// # Safety
///
/// Called by the host through the factory table. `host` must outlive the
/// returned plugin, which CLAP requires of the host anyway.
unsafe extern "C" fn create_plugin(
    _factory: *const clap_plugin_factory,
    host: *const clap_host,
    plugin_id: *const c_char,
) -> *const clap_plugin {
    if host.is_null() || plugin_id.is_null() {
        return std::ptr::null();
    }
    // SAFETY: CLAP plugin IDs are null-terminated C strings.
    if CStr::from_ptr(plugin_id) != plugin_descriptor().clap_id() {
        return std::ptr::null();
    }

    // As in the framework's own macro: `Arc` has no `leak`, so the reference
    // count is raised here and dropped again inside the framework's `destroy`.
    // SAFETY: the host outlives the plugin instance it is creating.
    let wrapper = Arc::into_raw(Wrapper::<HardwaveWettBoi>::new(host));
    // SAFETY: `wrapper` was just created and the reference count is ours.
    let inner: *const clap_plugin = (*wrapper).clap_plugin.as_ptr();

    let shim = Box::new(Shim {
        plugin: clap_plugin {
            // SAFETY: `inner` was filled in by `Wrapper::new`.
            desc: (*inner).desc,
            // Deliberately the framework's own pointer, not ours: its
            // extensions find their wrapper through this field and read no
            // other, so sharing it is what lets this shim wrap one extension
            // instead of all of them.
            plugin_data: (*inner).plugin_data,
            init: Some(shim_init),
            destroy: Some(shim_destroy),
            activate: Some(shim_activate),
            deactivate: Some(shim_deactivate),
            start_processing: Some(shim_start_processing),
            stop_processing: Some(shim_stop_processing),
            reset: Some(shim_reset),
            process: Some(shim_process),
            get_extension: Some(shim_get_extension),
            on_main_thread: Some(shim_on_main_thread),
        },
        inner,
        host,
        state: clap_plugin_state {
            save: Some(state_save),
            load: Some(state_load),
        },
    });

    // Freed in `shim_destroy`.
    Box::into_raw(shim) as *const clap_plugin
}

const CLAP_PLUGIN_FACTORY: clap_plugin_factory = clap_plugin_factory {
    get_plugin_count: Some(get_plugin_count),
    get_plugin_descriptor: Some(get_plugin_descriptor),
    create_plugin: Some(create_plugin),
};

/// # Safety
///
/// Called by the host once when the bundle is loaded.
unsafe extern "C" fn entry_init(_plugin_path: *const c_char) -> bool {
    setup_logger();
    true
}

/// # Safety
///
/// Called by the host once when the bundle is unloaded.
unsafe extern "C" fn entry_deinit() {}

/// # Safety
///
/// Called by the host with a null-terminated factory ID.
unsafe extern "C" fn entry_get_factory(factory_id: *const c_char) -> *const c_void {
    if factory_id.is_null() {
        return std::ptr::null();
    }
    // SAFETY: CLAP factory IDs are null-terminated C strings.
    if CStr::from_ptr(factory_id) == CLAP_PLUGIN_FACTORY_ID {
        &CLAP_PLUGIN_FACTORY as *const clap_plugin_factory as *const c_void
    } else {
        std::ptr::null()
    }
}

/// The CLAP plugin's entry point.
#[no_mangle]
#[used]
pub static clap_entry: clap_plugin_entry = clap_plugin_entry {
    clap_version: CLAP_VERSION,
    init: Some(entry_init),
    deinit: Some(entry_deinit),
    get_factory: Some(entry_get_factory),
};
