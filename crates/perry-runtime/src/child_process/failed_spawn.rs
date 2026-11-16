//! A failed spawn has no live reactor entry. On unix its pipe slots are
//! sockets whose child ends never reached a child, so they read EOF and close
//! by themselves, and `'close'` is counted on the ChildProcess
//! (`reactor::cp_attach_failed_pipe_sockets`). Elsewhere the empty streams are
//! finished after the child error and before child close.
use super::*;

/// Node's `onexit` for a spawn that failed: destroy stdin, emit the error,
/// then count the exit toward `'close'` (unix), or schedule close only after
/// delivering the error (elsewhere: a close timer armed at spawn time can
/// already be overdue before the error's setImmediate callback runs).
pub(super) extern "C" fn emit_error_then_close(
    closure: *const ClosureHeader,
    this: crate::closure::JsThis,
) -> f64 {
    let unhandled = {
        let scope = crate::gc::RuntimeHandleScope::new();
        let cp = scope.root_nanbox_f64(cp_this(this, closure));
        reactor::cp_destroy_stdin(cp.get_nanbox_f64());
        let unhandled = emit_spawn_error(cp.get_nanbox_f64());
        // An unhandled error ends the process in Node before `close` fires.
        if unhandled.is_none() {
            reactor::cp_failed_count_close(cp.get_nanbox_f64());
        }
        unhandled
    };
    throw_if_unhandled(unhandled);
    cp_undefined()
}

/// Emit the deferred spawn/fork failure `error` on the ChildProcess `cp`.
///
/// Returns the error when no `error` listener took it (#10730). Node's
/// EventEmitter throws an `error` event nobody listens for, so a failed
/// `spawn("/no/such/file")` without a handler dies with `Unhandled 'error'
/// event`. Perry used to drop it and carry on, which made a missing binary
/// look like a child that ran and exited.
pub(super) fn emit_spawn_error(cp: f64) -> Option<f64> {
    let scope = crate::gc::RuntimeHandleScope::new();
    let cp = scope.root_nanbox_f64(cp);
    let err = scope.root_nanbox_f64(cp_get_field(cp.get_nanbox_f64(), b"__cpError"));
    if JSValue::from_bits(err.get_nanbox_f64().to_bits()).is_undefined() {
        return None;
    }
    let handled = cp_emit(cp.get_nanbox_f64(), "error", &[err.get_nanbox_f64()]);
    cp_set_field(cp.get_nanbox_f64(), b"signalCode", TAG_NULL_F64);
    (!handled).then(|| err.get_nanbox_f64())
}

/// Throw `unhandled` as an uncaught exception. Called only once every handle
/// scope in the emitting frame has dropped, so the throw skips no cleanup.
pub(super) fn throw_if_unhandled(unhandled: Option<f64>) {
    if let Some(err) = unhandled {
        crate::exception::throw_open(err);
    }
}



#[cfg(test)]
mod tests {
    use super::*;


    extern "C" fn swallow_error(
        _closure: *const ClosureHeader,
        _this: crate::closure::JsThis,
        _err: f64,
    ) -> f64 {
        cp_undefined()
    }

    /// A failed-spawn ChildProcess stand-in: just the fields the emit reads.
    fn failed_child<'s>(scope: &'s crate::gc::RuntimeHandleScope) -> crate::gc::RuntimeHandle<'s> {
        let cp = scope.root_nanbox_f64(cp_box_ptr(crate::object::js_object_alloc(0, 4).cast()));
        let err = cp_make_error(
            "spawn /missing ENOENT",
            &[("code", cp_box_string("ENOENT"))],
        );
        cp_set_field(cp.get_nanbox_f64(), b"__cpError", err);
        cp
    }

    /// #10730: with no `error` listener the error comes back to be thrown.
    /// Before the fix the emit reported nothing, so the error was dropped.
    #[test]
    fn spawn_error_without_listener_is_unhandled() {
        let scope = crate::gc::RuntimeHandleScope::new();
        let cp = failed_child(&scope);
        let err = cp_get_field(cp.get_nanbox_f64(), b"__cpError");
        let unhandled = emit_spawn_error(cp.get_nanbox_f64())
            .expect("an error event nobody listens for must be thrown");
        assert_eq!(
            unhandled.to_bits(),
            err.to_bits(),
            "Node rethrows the same object"
        );
    }

    #[test]
    fn spawn_error_with_listener_is_handled() {
        let scope = crate::gc::RuntimeHandleScope::new();
        let cp = failed_child(&scope);
        let listener = crate::closure::js_closure_alloc(crate::fn_info!(swallow_error, 1), 0);
        super::super::emitter::cp_register(
            cp.get_nanbox_f64(),
            cp_box_string("error"),
            cp_box_ptr(listener.cast()),
        );
        assert!(emit_spawn_error(cp.get_nanbox_f64()).is_none());
    }

}
