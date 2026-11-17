use super::*;

use crate::closure::{
    js_closure_alloc, js_closure_get_capture_ptr, js_closure_set_capture_ptr, js_native_call_value,
    native_call_value_this, ClosureHeader, JsThis,
};
use crate::string::js_string_from_bytes;
use crate::value::JSValue;

/// Hidden field key holding the listener array for `event`.
pub(crate) fn cp_listener_key(event: &str) -> Vec<u8> {
    let mut k = b"__cpL_".to_vec();
    k.extend_from_slice(event.as_bytes());
    k
}

/// Append a listener closure to `target`'s `event` list (the `.on` body).
pub(crate) fn cp_register(target: f64, event: f64, cb: f64) {
    let name = match cp_value_to_string(event) {
        Some(n) => n,
        None => return,
    };
    let key = cp_listener_key(&name);
    let arr = match cp_array_ptr(cp_get_field(target, &key)) {
        Some(a) => a,
        None => crate::array::js_array_alloc(2),
    };
    let arr = crate::array::js_array_push_f64(arr, cb);
    cp_set_field(target, &key, cp_box_ptr(arr as *const u8));
}

/// Invoke every listener registered on `target` for `event`. Returns whether
/// any fired. The listener array is re-read each iteration so a moving GC
/// during a handler call can't strand us on a stale array pointer.
pub(crate) fn cp_emit(target: f64, event: &str, args: &[f64]) -> bool {
    let scope = crate::gc::RuntimeHandleScope::new();
    let target = scope.root_nanbox_f64(target);
    let args = scope.root_nanbox_f64_slice(args);
    if event == "message"
        && args.first().is_some_and(|msg| {
            crate::cluster::consume_internal_message(target.get_nanbox_f64(), msg.get_nanbox_f64())
        })
    {
        return true;
    }

    let async_ids = cp_handle_of(target.get_nanbox_f64())
        .and_then(|handle| reactor::cp_async_scope_for_target(handle, target.get_nanbox_f64()));
    if let Some(ids) = async_ids {
        crate::async_hook_slots::enter_resource_scope(ids);
    }

    let key = cp_listener_key(event);
    let mut i: u32 = 0;
    let mut fired = false;
    loop {
        let arr = match cp_array_ptr(cp_get_field(target.get_nanbox_f64(), &key)) {
            Some(a) => a,
            None => break,
        };
        if i >= crate::array::js_array_length(arr) {
            break;
        }
        let cb = crate::array::js_array_get_f64(arr, i);
        let current_args = crate::gc::RuntimeHandleScope::refreshed_nanbox_f64_slice(&args);
        unsafe {
            let _ = native_call_value_this(
                cb,
                JsThis::from_f64(target.get_nanbox_f64()),
                current_args.as_ptr(),
                current_args.len(),
            );
        }
        fired = true;
        i += 1;
    }

    // `cp_build_readable` installs node:stream's async iterator on stdout/stderr,
    // and that iterator registers its `data`/`end`/`error` listeners in node:stream's
    // registry rather than the one above. Forward there too, so a `for await` over a
    // child's output sees the chunks the reactor delivers.
    if !JSValue::from_bits(cp_get_field(target.get_nanbox_f64(), b"readable").to_bits())
        .is_undefined()
    {
        let current_args = crate::gc::RuntimeHandleScope::refreshed_nanbox_f64_slice(&args);
        crate::node_stream::emit_to_stream_listeners(
            target.get_nanbox_f64(),
            event.as_bytes(),
            &current_args,
        );
    }

    if let Some(ids) = async_ids {
        crate::async_hook_slots::leave_resource_scope(ids.async_id);
    }
    fired
}

#[cfg(test)]
mod relocation_tests;


// ----- method bodies (each receives the closure; slot 0 = host `this`) -----

pub(crate) extern "C" fn cp_method_on(
    closure: *const ClosureHeader,
    _this: crate::closure::JsThis,
    event: f64,
    cb: f64,
) -> f64 {
    let this = cp_this(_this, closure);
    cp_register(this, event, cb);
    this
}
pub(crate) extern "C" fn cp_method_emit(
    closure: *const ClosureHeader,
    _this: crate::closure::JsThis,
    event: f64,
    arg: f64,
) -> f64 {
    let this = cp_this(_this, closure);
    let name = match cp_value_to_string(event) {
        Some(n) => n,
        None => return TAG_FALSE_F64,
    };
    if cp_emit(this, &name, &[arg]) {
        TAG_TRUE_F64
    } else {
        TAG_FALSE_F64
    }
}
pub(crate) extern "C" fn cp_method_this0(
    closure: *const ClosureHeader,
    this: crate::closure::JsThis,
) -> f64 {
    cp_this(this, closure)
}
/// Keep a live child attached to the event loop. Calls are idempotent, matching
/// Node and Bun's process-handle contract.
pub(crate) extern "C" fn cp_method_ref(
    closure: *const ClosureHeader,
    _this: crate::closure::JsThis,
) -> f64 {
    let this = cp_this(_this, closure);
    if let Some(handle) = cp_handle_of(this) {
        reactor::cp_live_set_refed(handle, true);
    }
    this
}

/// Let the program terminate while this child continues running. The reactor
/// still pumps and roots the child whenever another handle keeps the loop
/// alive; only the active-handle accounting changes.
pub(crate) extern "C" fn cp_method_unref(
    closure: *const ClosureHeader,
    _this: crate::closure::JsThis,
) -> f64 {
    let this = cp_this(_this, closure);
    if let Some(handle) = cp_handle_of(this) {
        reactor::cp_live_set_refed(handle, false);
    }
    this
}

/// Low-level `new ChildProcess().spawn(options)` validation boundary. Node's
/// constructor is public even though normal callers use `spawn()`; keep the
/// constructed idle object inert after its setup checks.
pub(crate) extern "C" fn cp_method_child_spawn(
    closure: *const ClosureHeader,
    this: crate::closure::JsThis,
    options: f64,
) -> f64 {
    crate::child_process::validate::cp_validate_child_process_spawn(options);
    cp_this(this, closure)
}
pub(crate) extern "C" fn cp_method_kill(
    closure: *const ClosureHeader,
    _this: crate::closure::JsThis,
    signal: f64,
) -> f64 {
    cp_validate_signal(signal);
    let this = cp_this(_this, closure);
    cp_set_field(this, b"killed", TAG_TRUE_F64);
    // #1934: signal the live child if one is still running. `__cpHandle` is the
    // reactor registry key set by `spawn`. Returns true when the signal was
    // delivered (Node's `kill()` returns a boolean).
    if let Some(handle) = cp_handle_of(this) {
        if reactor::cp_live_kill(handle, signal) {
            return TAG_TRUE_F64;
        }
    }
    TAG_FALSE_F64
}
/// `child[Symbol.dispose]()` — Node aliases this to `kill()` and returns
/// `undefined`, so `using child = spawn(...)` terminates the subprocess on
/// scope exit. #2556.
pub(crate) extern "C" fn cp_method_dispose(
    closure: *const ClosureHeader,
    this: crate::closure::JsThis,
) -> f64 {
    let _ = cp_method_kill(closure, this, cp_undefined());
    cp_undefined()
}
pub(crate) fn js_fork_child(args_len: usize) -> f64 {
    cp_require_process_support();
    if args_len < 2 {
        crate::node_submodules::diagnostics::throw_type_error_no_code(
            b"Cannot destructure property 'initMessageChannel' of 'serialization[serializationMode]' as it is undefined.",
        );
    }
    f64::from_bits(JSValue::undefined().bits())
}
/// `removeListener(event, cb)` / `off(event, cb)` — rebuild the `event`
/// listener array without the matching closure (compared by NaN-boxed bits).
/// #1780.
pub(crate) extern "C" fn cp_method_remove_listener(
    closure: *const ClosureHeader,
    _this: crate::closure::JsThis,
    event: f64,
    cb: f64,
) -> f64 {
    let this = cp_this(_this, closure);
    if let Some(name) = cp_value_to_string(event) {
        let key = cp_listener_key(&name);
        if let Some(arr) = cp_array_ptr(cp_get_field(this, &key)) {
            let n = crate::array::js_array_length(arr);
            let mut out = crate::array::js_array_alloc(n);
            for i in 0..n {
                let v = crate::array::js_array_get_f64(arr, i);
                if v.to_bits() != cb.to_bits() {
                    out = crate::array::js_array_push_f64(out, v);
                }
            }
            cp_set_field(this, &key, cp_box_ptr(out as *const u8));
        }
    }
    this
}

/// `removeAllListeners([event])` — clear one event's listener list, or every
/// `__cpL_*` list when called with no event. #1780.
pub(crate) extern "C" fn cp_method_remove_all_listeners(
    closure: *const ClosureHeader,
    _this: crate::closure::JsThis,
    event: f64,
) -> f64 {
    let this = cp_this(_this, closure);
    if let Some(name) = cp_value_to_string(event) {
        let key = cp_listener_key(&name);
        let empty = crate::array::js_array_alloc(0);
        cp_set_field(this, &key, cp_box_ptr(empty as *const u8));
        return this;
    }
    // No event argument: clear every listener array on the object.
    if let Some(obj) = cp_object_ptr(this) {
        let keys = crate::object::js_object_keys(obj);
        if !keys.is_null() {
            let n = crate::array::js_array_length(keys);
            for i in 0..n {
                if let Some(k) = cp_value_to_string(crate::array::js_array_get_f64(keys, i)) {
                    if k.as_bytes().starts_with(b"__cpL_") {
                        let empty = crate::array::js_array_alloc(0);
                        cp_set_field(this, k.as_bytes(), cp_box_ptr(empty as *const u8));
                    }
                }
            }
        }
    }
    this
}








/// `child.send(message[, sendHandle][, options][, callback])` — serialize
/// `message` and write it to the IPC channel of a `fork()`ed child (#1933 /
/// #3316). The `this` is the ChildProcess.
///
/// Node semantics this matches (`subprocess.send.length === 4`):
/// - Returns `true` when the message was queued on an open channel, `false`
///   once the channel is closed (after `disconnect()`).
/// - The optional trailing `callback` fires asynchronously (on the next tick)
///   with `null` on success or an `Error [ERR_IPC_CHANNEL_CLOSED]`
///   (`message: "Channel closed"`) when the channel is closed.
///
/// The four value slots map to `message, sendHandle, options, callback`; the
/// callback is detected as the last *function* argument so the documented
/// optional `sendHandle` / `options` slots are skipped (those handle-/serialize-
/// option forms are otherwise no-ops here, matching the prior behavior).
pub(crate) extern "C" fn cp_method_send(
    closure: *const ClosureHeader,
    _this: crate::closure::JsThis,
    message: f64,
    a2: f64,
    a3: f64,
    a4: f64,
) -> f64 {
    let message_value = JSValue::from_bits(message.to_bits());
    if message_value.is_undefined() {
        crate::fs::validate::throw_type_error_with_code(
            "The \"message\" argument must be specified",
            "ERR_MISSING_ARGS",
        );
    }
    if unsafe { crate::symbol::js_is_symbol(message) != 0 }
        || (message_value.is_pointer() && !crate::fs::extract_closure_ptr(message).is_null())
    {
        crate::fs::validate::throw_type_error_with_code(
            "The \"message\" argument must be one of type string, object, number, or boolean",
            "ERR_INVALID_ARG_TYPE",
        );
    }
    let this = cp_this(_this, closure);

    // The callback is the last argument when it is a function. dispatch pads
    // missing slots with `undefined`, so scan slots 4→2 for a closure.
    let callback = [a4, a3, a2].into_iter().find(|v| {
        JSValue::from_bits(v.to_bits()).is_pointer()
            && !crate::fs::extract_closure_ptr(*v).is_null()
    });

    // A closed IPC channel (after `disconnect()`, or never connected) returns
    // `false` and reports `ERR_IPC_CHANNEL_CLOSED` to the callback.
    let connected = cp_get_field(this, b"connected");
    let channel_open = connected.to_bits() == TAG_TRUE_F64.to_bits();

    let ok = if channel_open {
        match cp_handle_of(this) {
            Some(handle) => reactor::cp_ipc_send(handle, message),
            None => false,
        }
    } else {
        false
    };

    if let Some(cb) = callback {
        cp_defer_send_callback(cb, ok);
    }

    if ok {
        TAG_TRUE_F64
    } else {
        TAG_FALSE_F64
    }
}

/// Schedule the `send` callback to fire on the next tick (Node delivers it
/// asynchronously). `ok` selects the argument: `null` on success, otherwise an
/// `Error [ERR_IPC_CHANNEL_CLOSED]` (`message: "Channel closed"`). The deferred
/// closure captures the callback in slot 0 and the success flag in slot 1.
pub(crate) fn cp_defer_send_callback(cb: f64, ok: bool) {
    let deferred = js_closure_alloc(
        crate::fn_info!(cp_send_callback_thunk, 0; with_declared(0)),
        2,
    );
    js_closure_set_capture_ptr(deferred, 0, cb.to_bits() as i64);
    let flag = if ok { TAG_TRUE_F64 } else { TAG_FALSE_F64 };
    js_closure_set_capture_ptr(deferred, 1, flag.to_bits() as i64);
    crate::builtins::js_queue_next_tick(deferred as i64);
}

/// Deferred `send` callback body. Slot 0 = the user callback; slot 1 = the
/// success flag. Invokes `callback(null)` on success or `callback(err)` with a
/// Node-shaped `ERR_IPC_CHANNEL_CLOSED` error on failure.
pub(crate) extern "C" fn cp_send_callback_thunk(
    closure: *const ClosureHeader,
    _this: crate::closure::JsThis,
) -> f64 {
    let cb = f64::from_bits(js_closure_get_capture_ptr(closure, 0) as u64);
    if crate::fs::extract_closure_ptr(cb).is_null() {
        return cp_undefined();
    }
    let flag = f64::from_bits(js_closure_get_capture_ptr(closure, 1) as u64);
    let ok = flag.to_bits() == TAG_TRUE_F64.to_bits();
    let arg = if ok {
        TAG_NULL_F64
    } else {
        cp_channel_closed_error()
    };
    let args = [arg];
    unsafe {
        js_native_call_value(
            cb,
            crate::closure::plain_call_receiver(),
            args.as_ptr(),
            args.len(),
        )
    };
    cp_undefined()
}

/// Build a Node-shaped `Error [ERR_IPC_CHANNEL_CLOSED]` value (`message:
/// "Channel closed"`, `code: "ERR_IPC_CHANNEL_CLOSED"`).
pub(crate) fn cp_channel_closed_error() -> f64 {
    let msg = js_string_from_bytes(b"Channel closed".as_ptr(), 14);
    crate::node_submodules::register_error_code_pub(msg, "ERR_IPC_CHANNEL_CLOSED");
    let err = crate::error::js_error_new_with_message(msg);
    crate::value::js_nanbox_pointer(err as i64)
}

/// `child.disconnect()` — close the IPC channel (#1933). Flips `connected` to
/// `false`, `channel` to `null`, and emits a `disconnect` event.
pub(crate) extern "C" fn cp_method_disconnect(
    closure: *const ClosureHeader,
    _this: crate::closure::JsThis,
) -> f64 {
    let this = cp_this(_this, closure);
    if let Some(handle) = cp_handle_of(this) {
        reactor::cp_ipc_disconnect(handle);
    }
    cp_set_field(this, b"connected", TAG_FALSE_F64);
    cp_set_field(this, b"channel", TAG_NULL_F64);

    let deferred = js_closure_alloc(
        crate::fn_info!(cp_disconnect_emit_thunk, 0; with_declared(0)),
        1,
    );
    js_closure_set_capture_ptr(deferred, 0, this.to_bits() as i64);
    crate::builtins::js_queue_next_tick(deferred as i64);
    cp_undefined()
}

extern "C" fn cp_disconnect_emit_thunk(
    closure: *const ClosureHeader,
    _this: crate::closure::JsThis,
) -> f64 {
    let child = f64::from_bits(js_closure_get_capture_ptr(closure, 0) as u64);
    cp_emit(child, "disconnect", &[]);
    cp_undefined()
}





/// Read the reactor registry key (`__cpHandle`) off a ChildProcess / stdio
/// sub-object, set by `spawn`. `None` when absent (e.g. a buffered child).
pub(crate) fn cp_handle_of(this: f64) -> Option<u64> {
    let h = cp_get_field(this, b"__cpHandle");
    if JSValue::from_bits(h.to_bits()).is_undefined() {
        return None;
    }
    if h.is_finite() && h >= 0.0 {
        Some(h as u64)
    } else {
        None
    }
}
