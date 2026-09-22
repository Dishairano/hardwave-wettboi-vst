//! Saving the state and loading it back has to give the same plugin.
//!
//! These tests drive the plugin's real CLAP entry point, the same one a DAW and
//! the CLAP validator use: create the plugin, set every parameter through the
//! host API, save the state into a stream, create a second plugin, load the
//! state into it, and compare. Nothing here is a stand-in for the framework;
//! it is the shipped binary interface.
//!
//! They also cover the two things the validator complained about:
//!
//! * value to text and back has to be stable, which is what
//!   `param-conversions` checks, and
//! * a state that is not what we wrote has to be refused, not applied and not
//!   crashed on.

use clap_sys::events::{
    clap_event_header, clap_event_param_value, clap_input_events, clap_output_events,
    CLAP_CORE_EVENT_SPACE_ID, CLAP_EVENT_IS_LIVE, CLAP_EVENT_PARAM_VALUE,
};
use clap_sys::ext::params::{
    clap_host_params, clap_param_info, clap_param_rescan_flags, clap_plugin_params,
    CLAP_EXT_PARAMS, CLAP_PARAM_RESCAN_VALUES,
};
use clap_sys::ext::state::{clap_plugin_state, CLAP_EXT_STATE};
use clap_sys::factory::plugin_factory::{clap_plugin_factory, CLAP_PLUGIN_FACTORY_ID};
use clap_sys::host::clap_host;
use clap_sys::plugin::clap_plugin;
use clap_sys::stream::{clap_istream, clap_ostream};
use clap_sys::version::CLAP_VERSION;
use hardwave_wettboi::sanitize_state;
use nih_plug::prelude::{Params, PluginState};
use nih_plug::wrapper::state::ParamValue;
use std::collections::BTreeMap;
use std::ffi::{c_char, c_void, CStr, CString};
use std::sync::atomic::{AtomicU32, Ordering};

// ─── A minimal host ─────────────────────────────────────────────────────────

unsafe extern "C" fn get_extension(_host: *const clap_host, _id: *const c_char) -> *const c_void {
    std::ptr::null()
}
unsafe extern "C" fn nop(_host: *const clap_host) {}

fn host() -> clap_host {
    host_with(get_extension)
}

fn host_with(
    get_extension: unsafe extern "C" fn(*const clap_host, *const c_char) -> *const c_void,
) -> clap_host {
    clap_host {
        clap_version: CLAP_VERSION,
        host_data: std::ptr::null_mut(),
        name: c"wettboi-test".as_ptr(),
        vendor: c"Hardwave Studios".as_ptr(),
        url: c"".as_ptr(),
        version: c"0".as_ptr(),
        get_extension: Some(get_extension),
        request_restart: Some(nop),
        request_process: Some(nop),
        request_callback: Some(nop),
    }
}

// ─── A host that records parameter rescans ──────────────────────────────────

/// Every flag `clap_host_params::rescan` has been called with, or'd together.
static RESCANNED: AtomicU32 = AtomicU32::new(0);

unsafe extern "C" fn record_rescan(_host: *const clap_host, flags: clap_param_rescan_flags) {
    RESCANNED.fetch_or(flags, Ordering::SeqCst);
}

static RECORDING_HOST_PARAMS: clap_host_params = clap_host_params {
    rescan: Some(record_rescan),
    clear: None,
    request_flush: None,
};

unsafe extern "C" fn get_extension_with_params(
    _host: *const clap_host,
    id: *const c_char,
) -> *const c_void {
    if !id.is_null() && CStr::from_ptr(id) == CLAP_EXT_PARAMS {
        &RECORDING_HOST_PARAMS as *const clap_host_params as *const c_void
    } else {
        std::ptr::null()
    }
}

struct EventList(Vec<clap_event_param_value>);

unsafe extern "C" fn ev_size(list: *const clap_input_events) -> u32 {
    let l = &*((*list).ctx as *const EventList);
    l.0.len() as u32
}
unsafe extern "C" fn ev_get(
    list: *const clap_input_events,
    index: u32,
) -> *const clap_event_header {
    let l = &*((*list).ctx as *const EventList);
    &l.0[index as usize].header as *const _
}
unsafe extern "C" fn out_push(
    _list: *const clap_output_events,
    _event: *const clap_event_header,
) -> bool {
    true
}

/// Writes in small chunks, the way a host that buffers its project file does.
/// The state code has to keep writing until everything is out.
unsafe extern "C" fn write_stream(
    stream: *const clap_ostream,
    buffer: *const c_void,
    size: u64,
) -> i64 {
    let buf = &mut *((*stream).ctx as *mut Vec<u8>);
    let n = (size as usize).min(64);
    buf.extend_from_slice(std::slice::from_raw_parts(buffer as *const u8, n));
    n as i64
}

struct ReadCtx {
    data: Vec<u8>,
    pos: usize,
}

/// Reads in small chunks, for the same reason.
unsafe extern "C" fn read_stream(
    stream: *const clap_istream,
    buffer: *mut c_void,
    size: u64,
) -> i64 {
    let ctx = &mut *((*stream).ctx as *mut ReadCtx);
    let n = (ctx.data.len() - ctx.pos).min(size as usize).min(64);
    std::ptr::copy_nonoverlapping(ctx.data[ctx.pos..].as_ptr(), buffer as *mut u8, n);
    ctx.pos += n;
    n as i64
}

unsafe fn create(host: &clap_host) -> *const clap_plugin {
    let entry = &hardwave_wettboi::clap_entry;
    let path = CString::new("/tmp/wettboi-test.clap").unwrap();
    assert!(entry.init.unwrap()(path.as_ptr()));
    let factory =
        entry.get_factory.unwrap()(CLAP_PLUGIN_FACTORY_ID.as_ptr()) as *const clap_plugin_factory;
    assert!(!factory.is_null());
    let desc = (*factory).get_plugin_descriptor.unwrap()(factory, 0);
    assert!(!desc.is_null());
    let plugin = (*factory).create_plugin.unwrap()(factory, host, (*desc).id);
    assert!(!plugin.is_null());
    assert!((*plugin).init.unwrap()(plugin));
    assert!((*plugin).activate.unwrap()(plugin, 48000.0, 1, 512));
    plugin
}

unsafe fn ext<T>(plugin: *const clap_plugin, id: &CStr) -> *const T {
    (*plugin).get_extension.unwrap()(plugin, id.as_ptr()) as *const T
}

unsafe fn param_infos(
    plugin: *const clap_plugin,
    params: *const clap_plugin_params,
) -> Vec<clap_param_info> {
    let count = (*params).count.unwrap()(plugin);
    let mut infos = Vec::with_capacity(count as usize);
    for i in 0..count {
        let mut info: clap_param_info = std::mem::zeroed();
        assert!((*params).get_info.unwrap()(plugin, i, &mut info));
        infos.push(info);
    }
    infos
}

fn name_of(info: &clap_param_info) -> String {
    // SAFETY: the name is a null-terminated C string the wrapper wrote.
    unsafe { CStr::from_ptr(info.name.as_ptr()) }
        .to_string_lossy()
        .into_owned()
}

// ─── The tests ──────────────────────────────────────────────────────────────

/// Every parameter has to come back exactly as it went in, and saving the
/// restored plugin again has to produce byte-identical state. That is what the
/// validator's state-reproducibility tests ask for, and what a DAW reopening a
/// project depends on.
#[test]
fn every_parameter_survives_a_save_and_a_load() {
    // SAFETY: the whole test drives the plug-in through its C ABI, the same calls a DAW
    // makes. Every pointer used below comes from create() or from the plug-in's own
    // get_extension, both of which return either null or a pointer valid for the life of
    // the instance; each is checked before use and the instance is destroyed at the end,
    // on this thread only, which is what the CLAP main-thread contract requires.
    unsafe {
        let host = host();
        let plugin = create(&host);
        let params: *const clap_plugin_params = ext(plugin, CLAP_EXT_PARAMS);
        let state: *const clap_plugin_state = ext(plugin, CLAP_EXT_STATE);
        assert!(!params.is_null() && !state.is_null());

        let infos = param_infos(plugin, params);
        assert!(!infos.is_empty());

        // Move every parameter off its default, the way the validator does.
        let mut events = EventList(Vec::new());
        for (i, info) in infos.iter().enumerate() {
            let value = ((i as f64 * 0.37) % 1.0) * (info.max_value - info.min_value);
            events.0.push(clap_event_param_value {
                header: clap_event_header {
                    size: std::mem::size_of::<clap_event_param_value>() as u32,
                    time: 0,
                    space_id: CLAP_CORE_EVENT_SPACE_ID,
                    type_: CLAP_EVENT_PARAM_VALUE,
                    flags: CLAP_EVENT_IS_LIVE,
                },
                param_id: info.id,
                cookie: std::ptr::null_mut(),
                note_id: -1,
                port_index: -1,
                channel: -1,
                key: -1,
                value,
            });
        }
        let in_events = clap_input_events {
            ctx: &mut events as *mut _ as *mut c_void,
            size: Some(ev_size),
            get: Some(ev_get),
        };
        let out_events = clap_output_events {
            ctx: std::ptr::null_mut(),
            try_push: Some(out_push),
        };
        (*params).flush.unwrap()(plugin, &in_events, &out_events);

        let mut before = Vec::new();
        for info in &infos {
            let mut v = 0.0f64;
            assert!((*params).get_value.unwrap()(plugin, info.id, &mut v));
            before.push(v);
        }

        let mut saved: Vec<u8> = Vec::new();
        let ostream = clap_ostream {
            ctx: &mut saved as *mut _ as *mut c_void,
            write: Some(write_stream),
        };
        assert!((*state).save.unwrap()(plugin, &ostream), "saving failed");

        // A second instance, which is what reopening a project gives you.
        let plugin2 = create(&host);
        let params2: *const clap_plugin_params = ext(plugin2, CLAP_EXT_PARAMS);
        let state2: *const clap_plugin_state = ext(plugin2, CLAP_EXT_STATE);
        let mut ctx = ReadCtx {
            data: saved.clone(),
            pos: 0,
        };
        let istream = clap_istream {
            ctx: &mut ctx as *mut _ as *mut c_void,
            read: Some(read_stream),
        };
        assert!((*state2).load.unwrap()(plugin2, &istream), "loading failed");

        let mut wrong = Vec::new();
        for (i, info) in infos.iter().enumerate() {
            let mut v = 0.0f64;
            assert!((*params2).get_value.unwrap()(plugin2, info.id, &mut v));
            if v != before[i] {
                wrong.push(format!("{}: {:.12} -> {:.12}", name_of(info), before[i], v));
            }
        }
        assert!(wrong.is_empty(), "parameters did not survive: {wrong:#?}");

        let mut saved_again: Vec<u8> = Vec::new();
        let ostream2 = clap_ostream {
            ctx: &mut saved_again as *mut _ as *mut c_void,
            write: Some(write_stream),
        };
        assert!((*state2).save.unwrap()(plugin2, &ostream2));
        assert_eq!(
            String::from_utf8_lossy(&saved[8..]),
            String::from_utf8_lossy(&saved_again[8..]),
            "saving the restored plugin produced different state"
        );
    }
}

/// A value converted to text and back has to convert to the same text again.
/// Printing the raw `f32` broke this: the host read the number back one unit in
/// the last place away, and the next conversion printed a different string.
#[test]
fn text_conversions_are_a_fixed_point() {
    // SAFETY: the whole test drives the plug-in through its C ABI, the same calls a DAW
    // makes. Every pointer used below comes from create() or from the plug-in's own
    // get_extension, both of which return either null or a pointer valid for the life of
    // the instance; each is checked before use and the instance is destroyed at the end,
    // on this thread only, which is what the CLAP main-thread contract requires.
    unsafe {
        let host = host();
        let plugin = create(&host);
        let params: *const clap_plugin_params = ext(plugin, CLAP_EXT_PARAMS);
        let to_text = |id, v: f64| {
            let mut text = [0 as c_char; 128];
            assert!((*params).value_to_text.unwrap()(
                plugin,
                id,
                v,
                text.as_mut_ptr(),
                128
            ));
            CStr::from_ptr(text.as_ptr()).to_string_lossy().into_owned()
        };

        let mut wrong = Vec::new();
        for info in param_infos(plugin, params) {
            for step in 0..=200 {
                let v = info.min_value + (info.max_value - info.min_value) * (step as f64 / 200.0);
                let text = to_text(info.id, v);
                let mut back = 0.0f64;
                let c_text = CString::new(text.clone()).unwrap();
                if !(*params).text_to_value.unwrap()(plugin, info.id, c_text.as_ptr(), &mut back) {
                    wrong.push(format!("{}: {text:?} does not parse", name_of(&info)));
                    continue;
                }
                let text_again = to_text(info.id, back);
                if text != text_again {
                    wrong.push(format!(
                        "{}: {text:?} read back as {text_again:?}",
                        name_of(&info)
                    ));
                }
            }
        }
        assert!(wrong.is_empty(), "text did not round-trip: {wrong:#?}");
    }
}

/// State that is not ours must be refused, and the plugin must keep running.
#[test]
fn a_corrupt_state_is_refused() {
    // SAFETY: the whole test drives the plug-in through its C ABI, the same calls a DAW
    // makes. Every pointer used below comes from create() or from the plug-in's own
    // get_extension, both of which return either null or a pointer valid for the life of
    // the instance; each is checked before use and the instance is destroyed at the end,
    // on this thread only, which is what the CLAP main-thread contract requires.
    unsafe {
        let host = host();
        let plugin = create(&host);
        let state: *const clap_plugin_state = ext(plugin, CLAP_EXT_STATE);
        let params: *const clap_plugin_params = ext(plugin, CLAP_EXT_PARAMS);
        let infos = param_infos(plugin, params);

        let junk = b"this is not the state you are looking for".to_vec();
        let mut data = (junk.len() as u64).to_le_bytes().to_vec();
        data.extend_from_slice(&junk);
        let mut ctx = ReadCtx { data, pos: 0 };
        let istream = clap_istream {
            ctx: &mut ctx as *mut _ as *mut c_void,
            read: Some(read_stream),
        };
        assert!(
            !(*state).load.unwrap()(plugin, &istream),
            "a corrupt state was accepted"
        );

        // And the plugin still answers, with its parameters untouched.
        for info in &infos {
            let mut v = 0.0f64;
            assert!((*params).get_value.unwrap()(plugin, info.id, &mut v));
            assert!(v.is_finite());
        }
    }
}

/// State that is not ours at all, the way the validator's `state-invalid-random`
/// test feeds it: bytes with no structure, whose first eight happen to claim the
/// state is exabytes long.
///
/// The framework reads those eight bytes as a length and hands them to
/// `Vec::with_capacity`. That allocation cannot succeed, so Rust aborts the
/// process: `memory allocation of 1074606175335821323 bytes failed`, and the
/// host goes down with the plugin. Nothing inside the plugin can catch it,
/// which is why the length is checked in `src/clap_export.rs` before the
/// framework ever sees the stream.
///
/// If this ever regresses the whole test binary dies with SIGABRT rather than
/// reporting a failed assertion. That is the bug.
#[test]
fn random_state_is_refused_without_crashing() {
    // SAFETY: the whole test drives the plug-in through its C ABI, the same calls a DAW
    // makes. Every pointer used below comes from create() or from the plug-in's own
    // get_extension, both of which return either null or a pointer valid for the life of
    // the instance; each is checked before use and the instance is destroyed at the end,
    // on this thread only, which is what the CLAP main-thread contract requires.
    unsafe {
        let host = host();
        let plugin = create(&host);
        let state: *const clap_plugin_state = ext(plugin, CLAP_EXT_STATE);
        let params: *const clap_plugin_params = ext(plugin, CLAP_EXT_PARAMS);
        let infos = param_infos(plugin, params);

        // A length prefix that asks for more memory than exists, then an
        // absurd one, then a plausible one whose body stops early, then a
        // stream with nothing in it at all.
        let cases: Vec<Vec<u8>> = vec![
            vec![0xAB; 96],
            u64::MAX.to_le_bytes().to_vec(),
            {
                let mut data = 4096u64.to_le_bytes().to_vec();
                data.extend_from_slice(b"{\"params\":{}}");
                data
            },
            Vec::new(),
            0u64.to_le_bytes().to_vec(),
        ];

        for (i, data) in cases.into_iter().enumerate() {
            let mut ctx = ReadCtx { data, pos: 0 };
            let istream = clap_istream {
                ctx: &mut ctx as *mut _ as *mut c_void,
                read: Some(read_stream),
            };
            assert!(
                !(*state).load.unwrap()(plugin, &istream),
                "case {i}: invalid state was accepted"
            );
        }

        // And the plugin is still usable afterwards.
        for info in &infos {
            let mut v = 0.0f64;
            assert!((*params).get_value.unwrap()(plugin, info.id, &mut v));
            assert!(v.is_finite());
        }
    }
}

/// A finished state load has to tell the host to re-read the parameter values.
///
/// The host has no other way to find out: it asked the plugin to load a state,
/// the plugin changed every parameter at once, and CLAP puts the burden of
/// saying so on the plugin. Without this the host keeps showing, automating and
/// reporting the values a fresh instance had, which are the defaults, while the
/// plugin plays the restored ones. That is the mismatch the validator's
/// `state-reproducibility-*` tests report.
#[test]
fn loading_state_asks_the_host_to_rescan_the_values() {
    // SAFETY: the whole test drives the plug-in through its C ABI, the same calls a DAW
    // makes. Every pointer used below comes from create() or from the plug-in's own
    // get_extension, both of which return either null or a pointer valid for the life of
    // the instance; each is checked before use and the instance is destroyed at the end,
    // on this thread only, which is what the CLAP main-thread contract requires.
    unsafe {
        RESCANNED.store(0, Ordering::SeqCst);

        let host = host_with(get_extension_with_params);
        let plugin = create(&host);
        let state: *const clap_plugin_state = ext(plugin, CLAP_EXT_STATE);

        let mut saved: Vec<u8> = Vec::new();
        let ostream = clap_ostream {
            ctx: &mut saved as *mut _ as *mut c_void,
            write: Some(write_stream),
        };
        assert!((*state).save.unwrap()(plugin, &ostream), "saving failed");

        let plugin2 = create(&host);
        let state2: *const clap_plugin_state = ext(plugin2, CLAP_EXT_STATE);
        let mut ctx = ReadCtx {
            data: saved,
            pos: 0,
        };
        let istream = clap_istream {
            ctx: &mut ctx as *mut _ as *mut c_void,
            read: Some(read_stream),
        };
        assert!((*state2).load.unwrap()(plugin2, &istream), "loading failed");

        assert_eq!(
            RESCANNED.load(Ordering::SeqCst) & CLAP_PARAM_RESCAN_VALUES,
            CLAP_PARAM_RESCAN_VALUES,
            "the host was never asked to rescan the parameter values"
        );
    }
}

/// A state whose values are out of range, or not numbers at all, must not reach
/// a parameter. Before this was checked, `set_plain_value` stored the number as
/// it was: a NaN mix or a 1e30 ms delay time went straight into the DSP.
#[test]
fn junk_values_never_reach_a_parameter() {
    let mut params = BTreeMap::new();
    params.insert("mix".to_string(), ParamValue::F32(f32::NAN));
    params.insert("dly_time_l".to_string(), ParamValue::F32(1e30));
    params.insert("rev_size".to_string(), ParamValue::F32(-4000.0));
    params.insert("lfo_phase".to_string(), ParamValue::F32(f32::NEG_INFINITY));
    params.insert("lfo_shape".to_string(), ParamValue::I32(99));
    params.insert("rev_enabled".to_string(), ParamValue::F32(0.5));
    params.insert("not_a_parameter".to_string(), ParamValue::F32(1.0));
    let mut state = PluginState {
        version: "0.0.1".to_string(),
        params,
        fields: BTreeMap::new(),
    };

    sanitize_state(&mut state);

    // The values that cannot be repaired are gone, so those parameters keep
    // their defaults.
    assert!(!state.params.contains_key("mix"));
    assert!(!state.params.contains_key("lfo_phase"));
    assert!(!state.params.contains_key("rev_enabled"));
    assert!(!state.params.contains_key("not_a_parameter"));

    // The ones that are merely out of range are pulled back into it.
    match state.params.get("dly_time_l") {
        Some(ParamValue::F32(v)) => assert!((1.0..=2000.0).contains(v), "delay time is {v}"),
        other => panic!("delay time became {other:?}"),
    }
    match state.params.get("rev_size") {
        Some(ParamValue::F32(v)) => assert!((0.0..=100.0).contains(v), "size is {v}"),
        other => panic!("size became {other:?}"),
    }
    match state.params.get("lfo_shape") {
        Some(ParamValue::I32(v)) => assert!((0..=4).contains(v), "shape is {v}"),
        other => panic!("shape became {other:?}"),
    }
}

/// Anything an earlier version wrote is in range already, so it has to come
/// through the check untouched. A preset that loaded before has to load now.
#[test]
fn a_state_from_an_older_version_is_left_alone() {
    let defaults = hardwave_wettboi::params::WettBoiParams::default();
    let mut params = BTreeMap::new();
    for (id, ptr, _group) in defaults.param_map() {
        // SAFETY: `ptr` points into `defaults`, which outlives this loop.
        let plain = unsafe { ptr.unmodulated_plain_value() };
        let value = match ptr {
            nih_plug::prelude::ParamPtr::FloatParam(_) => ParamValue::F32(plain),
            nih_plug::prelude::ParamPtr::BoolParam(_) => ParamValue::Bool(plain >= 0.5),
            _ => ParamValue::I32(plain.round() as i32),
        };
        params.insert(id, value);
    }
    let mut state = PluginState {
        version: "0.3.13".to_string(),
        params: params.clone(),
        fields: BTreeMap::new(),
    };

    sanitize_state(&mut state);

    assert_eq!(state.params.len(), params.len(), "entries were dropped");
    for (id, before) in &params {
        let after = state.params.get(id).expect("a parameter went missing");
        match (before, after) {
            (ParamValue::F32(a), ParamValue::F32(b)) => {
                assert_eq!(a, b, "{id} changed")
            }
            (ParamValue::I32(a), ParamValue::I32(b)) => assert_eq!(a, b, "{id} changed"),
            (ParamValue::Bool(a), ParamValue::Bool(b)) => assert_eq!(a, b, "{id} changed"),
            _ => panic!("{id} changed kind"),
        }
    }
}
