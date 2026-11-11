//! Class, interface and enum registration: a small description language for methods (names,
//! modifiers, return types, parameters with types and defaults) turned into the engine's
//! internal arginfo and function tables at MINIT (leaked: they live as long as the process).

use super::{sys, zstr, zval};
use std::ffi::{c_char, CString};
use std::ptr::{null, null_mut};

pub const PUBLIC: u32 = sys::ZEND_ACC_PUBLIC;
pub const PROTECTED: u32 = sys::ZEND_ACC_PROTECTED;
pub const PRIVATE: u32 = sys::ZEND_ACC_PRIVATE;
pub const STATIC: u32 = sys::ZEND_ACC_STATIC;
pub const FINAL: u32 = sys::ZEND_ACC_FINAL;
pub const ABSTRACT: u32 = sys::ZEND_ACC_ABSTRACT;
pub const READONLY: u32 = sys::ZEND_ACC_READONLY;
/// Calls made from these functions use strict_types, as from revolt/event-loop's PHP files.
pub const STRICT: u32 = 1u32 << 31;
pub const NOT_SERIALIZABLE: u32 = 1 << 29;

// zend_type_info.h: MAY_BE_*
pub const T_NULL: u32 = 1 << 1;
pub const T_BOOL: u32 = (1 << 2) | (1 << 3);
pub const T_INT: u32 = 1 << 4;
pub const T_FLOAT: u32 = 1 << 5;
pub const T_STRING: u32 = 1 << 6;
pub const T_ARRAY: u32 = 1 << 7;
pub const T_OBJECT: u32 = 1 << 8;
pub const T_RESOURCE: u32 = 1 << 9;
pub const T_VOID: u32 = 1 << 14;
pub const T_NEVER: u32 = 1 << 17;
pub const T_MIXED: u32 = T_NULL | T_BOOL | T_INT | T_FLOAT | T_STRING | T_ARRAY | T_OBJECT | T_RESOURCE;

/// _ZEND_TYPE_NAME_BIT: the type's pointer is a zend_string class name.
const NAME_BIT: u32 = 1 << 24;
/// The bit of a class name given as a C string in internal arginfo: _ZEND_TYPE_LITERAL_NAME_BIT
/// from PHP 8.3 on, _ZEND_TYPE_NAME_BIT before (PHP 8.2 converts those C strings at registration).
#[cfg(php83)]
const LITERAL_NAME_BIT: u32 = 1 << 23;
#[cfg(not(php83))]
const LITERAL_NAME_BIT: u32 = 1 << 24;
const SEND_BY_REF: u32 = 1 << 25;
const VARIADIC: u32 = 1 << 27;

/// A declared type: a mask of builtin types plus an optional class.
#[derive(Clone, Copy)]
pub struct Ty {
    pub mask: u32,
    pub class: Option<&'static str>,
}

/// No type.
pub const NONE: Ty = Ty { mask: 0, class: None };

pub const fn t(mask: u32) -> Ty {
    Ty { mask, class: None }
}

/// A class type (`nullable`: `?Class`).
pub const fn c(class: &'static str, nullable: bool) -> Ty {
    Ty { mask: if nullable { T_NULL } else { 0 }, class: Some(class) }
}

pub struct Arg {
    pub name: &'static str,
    pub ty: Ty,
    pub default: Option<&'static str>,
    pub variadic: bool,
}

pub const fn a(name: &'static str, ty: Ty) -> Arg {
    Arg { name, ty, default: None, variadic: false }
}

pub const fn ad(name: &'static str, ty: Ty, default: &'static str) -> Arg {
    Arg { name, ty, default: Some(default), variadic: false }
}

pub const fn av(name: &'static str, ty: Ty) -> Arg {
    Arg { name, ty, default: None, variadic: true }
}

pub type Handler = sys::zif_handler;

pub struct Method {
    pub name: &'static str,
    pub handler: Handler,
    pub flags: u32,
    pub ret: Ty,
    pub args: Vec<Arg>,
}

pub fn m(name: &'static str, handler: Handler, flags: u32, ret: Ty, args: Vec<Arg>) -> Method {
    Method { name, handler, flags, ret, args }
}

fn leak_cstr(s: &str) -> *const c_char {
    CString::new(s).unwrap().into_raw()
}

fn zend_type(ty: Ty, extra: u32) -> sys::zend_type {
    match ty.class {
        Some(c) => sys::zend_type { ptr: leak_cstr(c) as *mut _, type_mask: ty.mask | LITERAL_NAME_BIT | extra },
        None => sys::zend_type { ptr: null_mut(), type_mask: ty.mask | extra },
    }
}

/// The function table of `methods` (with its arginfo), terminated by an empty entry.
pub fn functions(methods: Vec<Method>) -> *const sys::zend_function_entry {
    let mut entries: Vec<sys::zend_function_entry> = Vec::with_capacity(methods.len() + 1);
    for me in methods {
        let required = me.args.iter().filter(|a| a.default.is_none() && !a.variadic).count();
        let mut info: Vec<sys::zend_internal_arg_info> = Vec::with_capacity(me.args.len() + 1);
        // the return type, with the number of required arguments in the name slot
        info.push(sys::zend_internal_arg_info { name: required as *const c_char, type_: zend_type(me.ret, 0), default_value: null() });
        for arg in &me.args {
            let extra = if arg.variadic { VARIADIC } else { 0 };
            info.push(sys::zend_internal_arg_info {
                name: leak_cstr(arg.name),
                type_: zend_type(arg.ty, extra),
                default_value: arg.default.map(leak_cstr).unwrap_or(null()),
            });
        }
        let n = me.args.len() as u32;
        let info = Box::leak(info.into_boxed_slice());
        entries.push(fe(leak_cstr(me.name), me.handler, info.as_ptr(), n, me.flags));
    }
    entries.push(fe(null(), None, null(), 0, 0));
    Box::leak(entries.into_boxed_slice()).as_ptr()
}

const fn fe(fname: *const c_char, handler: Handler, arg_info: *const sys::zend_internal_arg_info, num_args: u32, flags: u32) -> sys::zend_function_entry {
    sys::zend_function_entry {
        fname,
        handler,
        arg_info,
        num_args,
        flags: flags as _,
        #[cfg(php84)]
        frameless_function_infos: null(),
        #[cfg(php84)]
        doc_comment: null(),
    }
}

#[allow(unused)]
const _: () = {
    let _ = SEND_BY_REF;
};

/// INIT_CLASS_ENTRY(ce, name, functions).
unsafe fn class_entry(name: &str, functions: *const sys::zend_function_entry) -> sys::zend_class_entry {
    let mut ce: sys::zend_class_entry = std::mem::zeroed();
    ce.name = zstr::interned(name.as_bytes());
    #[cfg(php83)]
    {
        ce.default_object_handlers = std::ptr::addr_of!(sys::std_object_handlers);
    }
    ce.info.internal.builtin_functions = functions;
    ce
}

pub unsafe fn register_class(name: &str, methods: Vec<Method>, parent: *mut sys::zend_class_entry, flags: u32) -> *mut sys::zend_class_entry {
    let mut ce = class_entry(name, functions(methods));
    #[cfg(php84)]
    return sys::zend_register_internal_class_with_flags(&mut ce, parent, flags as _);
    #[cfg(not(php84))]
    {
        let c = sys::zend_register_internal_class_ex(&mut ce, parent);
        (*c).ce_flags |= flags;
        c
    }
}

pub unsafe fn register_interface(name: &str, methods: Vec<Method>) -> *mut sys::zend_class_entry {
    let mut ce = class_entry(name, functions(methods));
    sys::zend_register_internal_interface(&mut ce)
}

/// A pure (not backed) enum with its cases.
pub unsafe fn register_enum(name: &str, cases: &[&str]) -> *mut sys::zend_class_entry {
    let n = CString::new(name).unwrap();
    let ce = sys::zend_register_internal_enum(n.as_ptr(), sys::IS_UNDEF as u8, functions(vec![]));
    // enums are final (as compiled ones are)
    (*ce).ce_flags |= FINAL;
    for case in cases {
        let c = CString::new(*case).unwrap();
        sys::zend_enum_add_case_cstr(ce, c.as_ptr(), null_mut());
    }
    ce
}

pub unsafe fn implements(ce: *mut sys::zend_class_entry, iface: *mut sys::zend_class_entry) {
    sys::zend_class_implements(ce, 1, iface);
}

pub unsafe fn const_long(ce: *mut sys::zend_class_entry, name: &str, v: i64) {
    let mut value = zval::long(v);
    sys::zend_declare_class_constant_ex(ce, zstr::interned(name.as_bytes()), &mut *value, PUBLIC as i32, null_mut());
}

/// A property's default value.
pub enum Default {
    /// typed and uninitialized
    Undef,
    Null,
    Bool(bool),
    Str(&'static str),
    EmptyArray,
}

/// zend_declare_typed_property.
pub unsafe fn prop(ce: *mut sys::zend_class_entry, name: &str, ty: Ty, flags: u32, default: Default) {
    let mut dv = match default {
        Default::Undef => zval::undef(),
        Default::Null => zval::null(),
        Default::Bool(b) => zval::bool(b),
        Default::Str(s) => zval::str_owned(zstr::interned(s.as_bytes())),
        Default::EmptyArray => {
            let mut z = zval::undef();
            z.value.arr = std::ptr::addr_of!(sys::zend_empty_array) as *mut _;
            z.u1.type_info = zval::ARRAY as u32;
            z
        }
    };
    let ty = match ty.class {
        Some(c) => sys::zend_type { ptr: zstr::interned(c.as_bytes()) as *mut _, type_mask: NAME_BIT | ty.mask },
        None => sys::zend_type { ptr: null_mut(), type_mask: ty.mask },
    };
    sys::zend_declare_typed_property(ce, zstr::interned(name.as_bytes()), &mut *dv, flags as i32, null_mut(), ty);
}

/// The slot number of a declared (non-static) property: OBJ_PROP_NUM(obj, n) is the property.
pub unsafe fn prop_slot(ce: *mut sys::zend_class_entry, name: &str) -> usize {
    let z = super::hash::find_str(&(*ce).properties_info, name.as_bytes());
    assert!(!z.is_null(), "revolt: property {name} not declared");
    let info = (*z).value.ptr as *mut sys::zend_property_info;
    let off = (*info).offset as usize;
    (off - std::mem::offset_of!(sys::zend_object, properties_table)) / std::mem::size_of::<sys::zval>()
}
