//! The PHP classes of revolt/event-loop 1.0.9, registered with the same names, modifiers,
//! signatures, properties and constants (checked by tools/dump-api.php), and their behaviour.

pub mod callbacks;
pub mod driver;
pub mod errors;
pub mod eventloop;
pub mod factory;
pub mod fiber_local;
pub mod suspension;
pub mod tracing;

use crate::zend::class::{self as k, a, ad, c, m, t, Default as Dv, Ty, NONE};
use crate::zend::class::av as avv;
use crate::zend::sys;
use crate::zh;
use std::ptr::null_mut;

/// The class entries (registered at MINIT by the owning copy).
pub struct Ces {
    pub event_loop: *mut sys::zend_class_entry,
    pub driver: *mut sys::zend_class_entry,
    pub suspension: *mut sys::zend_class_entry,
    pub callback_type: *mut sys::zend_class_entry,
    pub driver_factory: *mut sys::zend_class_entry,
    pub fiber_local: *mut sys::zend_class_entry,
    pub invalid_callback_error: *mut sys::zend_class_entry,
    pub uncaught_throwable: *mut sys::zend_class_entry,
    pub unsupported_feature: *mut sys::zend_class_entry,
    pub abstract_driver: *mut sys::zend_class_entry,
    pub stream_select: *mut sys::zend_class_entry,
    pub ev: *mut sys::zend_class_entry,
    pub event: *mut sys::zend_class_entry,
    pub uv: *mut sys::zend_class_entry,
    pub gc_driver: *mut sys::zend_class_entry,
    pub tracing: *mut sys::zend_class_entry,
    pub closure_helper: *mut sys::zend_class_entry,
    pub driver_callback: *mut sys::zend_class_entry,
    pub defer_callback: *mut sys::zend_class_entry,
    pub timer_callback: *mut sys::zend_class_entry,
    pub stream_callback: *mut sys::zend_class_entry,
    pub readable_callback: *mut sys::zend_class_entry,
    pub writable_callback: *mut sys::zend_class_entry,
    pub signal_callback: *mut sys::zend_class_entry,
    pub driver_suspension: *mut sys::zend_class_entry,
    pub timer_queue: *mut sys::zend_class_entry,
}

static mut CE: Ces = Ces {
    event_loop: null_mut(),
    driver: null_mut(),
    suspension: null_mut(),
    callback_type: null_mut(),
    driver_factory: null_mut(),
    fiber_local: null_mut(),
    invalid_callback_error: null_mut(),
    uncaught_throwable: null_mut(),
    unsupported_feature: null_mut(),
    abstract_driver: null_mut(),
    stream_select: null_mut(),
    ev: null_mut(),
    event: null_mut(),
    uv: null_mut(),
    gc_driver: null_mut(),
    tracing: null_mut(),
    closure_helper: null_mut(),
    driver_callback: null_mut(),
    defer_callback: null_mut(),
    timer_callback: null_mut(),
    stream_callback: null_mut(),
    readable_callback: null_mut(),
    writable_callback: null_mut(),
    signal_callback: null_mut(),
    driver_suspension: null_mut(),
    timer_queue: null_mut(),
};

#[inline(always)]
pub fn ce() -> &'static Ces {
    unsafe { &*std::ptr::addr_of!(CE) }
}

const PUB: u32 = k::PUBLIC | k::STRICT;
const PRO: u32 = k::PROTECTED | k::STRICT;
const PRI: u32 = k::PRIVATE | k::STRICT;
const STA: u32 = k::STATIC;
const ABS_PUB: u32 = k::PUBLIC | k::ABSTRACT;
const ABS_PRO: u32 = k::PROTECTED | k::ABSTRACT;

const S: Ty = t(k::T_STRING);
const V: Ty = t(k::T_VOID);
const B: Ty = t(k::T_BOOL);
const F: Ty = t(k::T_FLOAT);
const I: Ty = t(k::T_INT);
const A: Ty = t(k::T_ARRAY);
const MIX: Ty = t(k::T_MIXED);
const NEVER: Ty = t(k::T_NEVER);
const CL: Ty = c("Closure", false);
const CLN: Ty = c("Closure", true);
const DRV: Ty = c("Revolt\\EventLoop\\Driver", false);
const SUSP: Ty = c("Revolt\\EventLoop\\Suspension", false);
const CBT: Ty = c("Revolt\\EventLoop\\CallbackType", false);
const DCB: Ty = c("Revolt\\EventLoop\\Internal\\DriverCallback", false);
const THR: Ty = c("Throwable", false);
// `: self` of the static factories: Reflection shows "self" before PHP 8.5, the class after
#[cfg(php85)]
const SELF_ICE: Ty = c("Revolt\\EventLoop\\InvalidCallbackError", false);
#[cfg(not(php85))]
const SELF_ICE: Ty = c("self", false);
#[cfg(php85)]
const SELF_UT: Ty = c("Revolt\\EventLoop\\UncaughtThrowable", false);
#[cfg(not(php85))]
const SELF_UT: Ty = c("self", false);

/// The methods of the Driver interface, with handlers (`None`: abstract).
fn driver_methods(flags: u32, h: &dyn Fn(&'static str) -> k::Handler) -> Vec<k::Method> {
    vec![
        m("run", h("run"), flags, V, vec![]),
        m("stop", h("stop"), flags, V, vec![]),
        m("getSuspension", h("getSuspension"), flags, SUSP, vec![]),
        m("isRunning", h("isRunning"), flags, B, vec![]),
        m("queue", h("queue"), flags, V, vec![a("closure", CL), avv("args", MIX)]),
        m("defer", h("defer"), flags, S, vec![a("closure", CL)]),
        m("delay", h("delay"), flags, S, vec![a("delay", F), a("closure", CL)]),
        m("repeat", h("repeat"), flags, S, vec![a("interval", F), a("closure", CL)]),
        m("onReadable", h("onReadable"), flags, S, vec![a("stream", MIX), a("closure", CL)]),
        m("onWritable", h("onWritable"), flags, S, vec![a("stream", MIX), a("closure", CL)]),
        m("onSignal", h("onSignal"), flags, S, vec![a("signal", I), a("closure", CL)]),
        m("enable", h("enable"), flags, S, vec![a("callbackId", S)]),
        m("cancel", h("cancel"), flags, V, vec![a("callbackId", S)]),
        m("disable", h("disable"), flags, S, vec![a("callbackId", S)]),
        m("reference", h("reference"), flags, S, vec![a("callbackId", S)]),
        m("unreference", h("unreference"), flags, S, vec![a("callbackId", S)]),
        m("setErrorHandler", h("setErrorHandler"), flags, V, vec![a("errorHandler", CLN)]),
        m("getErrorHandler", h("getErrorHandler"), flags, CLN, vec![]),
        m("getHandle", h("getHandle"), flags, MIX, vec![]),
        m("getIdentifiers", h("getIdentifiers"), flags, A, vec![]),
        m("getType", h("getType"), flags, CBT, vec![a("callbackId", S)]),
        m("isEnabled", h("isEnabled"), flags, B, vec![a("callbackId", S)]),
        m("isReferenced", h("isReferenced"), flags, B, vec![a("callbackId", S)]),
        m("__debugInfo", h("__debugInfo"), flags, A, vec![]),
    ]
}

/// Registers every class (MINIT of the owning copy).
pub unsafe fn register_all() {
    use driver as dr;
    let ce = &mut *std::ptr::addr_of_mut!(CE);

    // ---- interfaces and the enum
    ce.driver = k::register_interface("Revolt\\EventLoop\\Driver", driver_methods(ABS_PUB, &|_| None));
    ce.suspension = k::register_interface(
        "Revolt\\EventLoop\\Suspension",
        vec![
            m("resume", None, ABS_PUB, V, vec![ad("value", MIX, "null")]),
            m("suspend", None, ABS_PUB, MIX, vec![]),
            m("throw", None, ABS_PUB, V, vec![a("throwable", THR)]),
        ],
    );
    ce.callback_type = k::register_enum("Revolt\\EventLoop\\CallbackType", &["Defer", "Delay", "Repeat", "Readable", "Writable", "Signal"]);

    // ---- exceptions
    ce.invalid_callback_error = k::register_class(
        "Revolt\\EventLoop\\InvalidCallbackError",
        vec![
            m("nonNullReturn", zh!(errors::icr_non_null_return), PUB | STA, SELF_ICE, vec![a("callbackId", S), a("closure", CL)]),
            m("invalidIdentifier", zh!(errors::icr_invalid_identifier), PUB | STA, SELF_ICE, vec![a("callbackId", S)]),
            m("__construct", zh!(errors::icr_construct), PRI, NONE, vec![a("callbackId", S), a("code", I), a("message", S)]),
            m("getCallbackId", zh!(errors::icr_get_callback_id), PUB, S, vec![]),
            m("addInfo", zh!(errors::icr_add_info), PUB, V, vec![a("key", S), a("message", S)]),
        ],
        sys::zend_ce_error,
        k::FINAL,
    );
    k::const_long(ce.invalid_callback_error, "E_NONNULL_RETURN", 1);
    k::const_long(ce.invalid_callback_error, "E_INVALID_IDENTIFIER", 2);
    k::prop(ce.invalid_callback_error, "rawMessage", S, k::PRIVATE | k::READONLY, Dv::Undef);
    k::prop(ce.invalid_callback_error, "callbackId", S, k::PRIVATE | k::READONLY, Dv::Undef);
    k::prop(ce.invalid_callback_error, "info", A, k::PRIVATE, Dv::EmptyArray);

    ce.uncaught_throwable = k::register_class(
        "Revolt\\EventLoop\\UncaughtThrowable",
        vec![
            m("throwingCallback", zh!(errors::ut_throwing_callback), PUB | STA, SELF_UT, vec![a("closure", CL), a("previous", THR)]),
            m("throwingErrorHandler", zh!(errors::ut_throwing_error_handler), PUB | STA, SELF_UT, vec![a("closure", CL), a("previous", THR)]),
            m("__construct", zh!(errors::ut_construct), PRI, NONE, vec![a("message", S), a("closure", CL), a("previous", THR)]),
        ],
        sys::zend_ce_error,
        k::FINAL,
    );
    ce.unsupported_feature = k::register_class("Revolt\\EventLoop\\UnsupportedFeatureException", vec![], sys::zend_ce_exception, k::FINAL);

    // ---- Internal: callbacks, the timer queue, closure descriptions
    ce.closure_helper = k::register_class("Revolt\\EventLoop\\Internal\\ClosureHelper", vec![m("getDescription", zh!(errors::closure_helper_get_description), PUB | STA, S, vec![a("closure", CL)])], null_mut(), k::FINAL);
    callbacks::register(ce);

    // ---- drivers
    let adh = |n: &'static str| -> k::Handler { dr::handler(n) };
    let mut abs = driver_methods(PUB, &adh);
    abs.retain(|me| me.name != "getHandle");
    abs.extend(vec![
        m("__construct", zh!(dr::construct), PUB, NONE, vec![]),
        m("__serialize", zh!(dr::serialize), PUB | k::FINAL, NEVER, vec![]),
        m("__unserialize", zh!(dr::unserialize), PUB | k::FINAL, NEVER, vec![a("data", A)]),
        m("activate", None, ABS_PRO, V, vec![a("callbacks", A)]),
        m("dispatch", None, ABS_PRO, V, vec![a("blocking", B)]),
        m("deactivate", None, ABS_PRO, V, vec![a("callback", DCB)]),
        m("now", None, ABS_PRO, F, vec![]),
        m("enqueueCallback", zh!(dr::enqueue_callback), PRO | k::FINAL, V, vec![a("callback", DCB)]),
        m("error", zh!(dr::error), PRO | k::FINAL, V, vec![a("closure", CL), a("exception", THR)]),
        m("invokeMicrotasks", zh!(dr::private_noop), PRI, V, vec![]),
        m("isEmpty", zh!(dr::is_empty), PRI, B, vec![]),
        m("tick", zh!(dr::private_noop), PRI, V, vec![a("previousIdle", B)]),
        m("invokeCallbacks", zh!(dr::private_noop), PRI, V, vec![]),
        m("setInterrupt", zh!(dr::private_noop), PRI, V, vec![a("interrupt", CL)]),
        m("invokeInterrupt", zh!(dr::private_noop), PRI, V, vec![]),
        m("createLoopFiber", zh!(dr::private_noop), PRI, V, vec![]),
        m("createCallbackFiber", zh!(dr::private_noop), PRI, V, vec![]),
        m("createErrorCallback", zh!(dr::private_noop), PRI, V, vec![]),
        m("callbackId", zh!(dr::callback_id), PRI, S, vec![]),
    ]);
    // AbstractDriver::onWritable($stream, ...) has no type (Revolt's signature)
    for me in abs.iter_mut() {
        if me.name == "onWritable" {
            me.args[0].ty = NONE;
        }
    }
    ce.abstract_driver = k::register_class("Revolt\\EventLoop\\Internal\\AbstractDriver", abs, null_mut(), k::ABSTRACT);
    k::implements(ce.abstract_driver, ce.driver);
    let ad = ce.abstract_driver;
    (*ad).__bindgen_anon_2.create_object = Some(dr::create_object);
    let pr = k::PRIVATE;
    k::prop(ad, "nextId", S, pr, Dv::Str("a"));
    k::prop(ad, "fiber", c("Fiber", false), pr, Dv::Undef);
    k::prop(ad, "callbackFiber", c("Fiber", false), pr, Dv::Undef);
    k::prop(ad, "errorCallback", CL, pr, Dv::Undef);
    k::prop(ad, "callbacks", A, pr, Dv::EmptyArray);
    k::prop(ad, "enableQueue", A, pr, Dv::EmptyArray);
    k::prop(ad, "enableDeferQueue", A, pr, Dv::EmptyArray);
    k::prop(ad, "errorHandler", CLN, pr, Dv::Null);
    k::prop(ad, "interrupt", CLN, pr, Dv::Null);
    k::prop(ad, "interruptCallback", CL, pr | k::READONLY, Dv::Undef);
    k::prop(ad, "queueCallback", CL, pr | k::READONLY, Dv::Undef);
    k::prop(ad, "runCallback", CL, pr | k::READONLY, Dv::Undef);
    k::prop(ad, "internalSuspensionMarker", c("stdClass", false), pr | k::READONLY, Dv::Undef);
    k::prop(ad, "microtaskQueue", c("SplQueue", false), pr | k::READONLY, Dv::Undef);
    k::prop(ad, "callbackQueue", c("SplQueue", false), pr | k::READONLY, Dv::Undef);
    k::prop(ad, "idle", B, pr, Dv::Bool(false));
    k::prop(ad, "stopped", B, pr, Dv::Bool(false));
    k::prop(ad, "suspensions", c("WeakMap", false), pr, Dv::Undef);
    dr::register_drivers(ce);

    tracing::register(ce);
    suspension::register(ce);
    fiber_local::register(ce);
    factory::register(ce);
    eventloop::register(ce);
}

/// Lower-case class name lookups used across the classes.
pub unsafe fn class_lc(name: &[u8]) -> *mut sys::zend_class_entry {
    crate::zend::call::lookup_class(name)
}

pub use crate::zend::class::Method;
pub(crate) use crate::zend::class::{a as arg, ad as arg_default, m as method};
pub(crate) use crate::zend::class::av as av;
pub(crate) const T_S: Ty = S;
pub(crate) const T_V: Ty = V;
pub(crate) const T_B: Ty = B;
pub(crate) const T_F: Ty = F;
pub(crate) const T_I: Ty = I;
pub(crate) const T_A: Ty = A;
pub(crate) const T_MIX: Ty = MIX;
pub(crate) const T_NEVER: Ty = NEVER;
pub(crate) const T_CL: Ty = CL;
pub(crate) const T_CLN: Ty = CLN;
pub(crate) const T_DRV: Ty = DRV;
pub(crate) const T_SUSP: Ty = SUSP;
pub(crate) const T_CBT: Ty = CBT;
pub(crate) const T_DCB: Ty = DCB;
pub(crate) const T_THR: Ty = THR;
pub(crate) const F_PUB: u32 = PUB;
pub(crate) const F_PRO: u32 = PRO;
pub(crate) const F_PRI: u32 = PRI;
pub(crate) const F_STA: u32 = STA;
pub(crate) fn driver_method_list(flags: u32, h: &dyn Fn(&'static str) -> k::Handler) -> Vec<k::Method> {
    driver_methods(flags, h)
}
