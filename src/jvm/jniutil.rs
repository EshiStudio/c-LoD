//! JNI micro-helpers: jvalue constructors, exception guards, string IO.
//!
//! All JVM-touching code funnels through these so pending exceptions can
//! never leak into the next call (a pending exception makes JNI calls UB).

use jvmti_bindings::prelude::*;

pub(crate) fn jv_i(v: i32) -> jni::jvalue {
    jni::jvalue { i: v }
}

pub(crate) fn jv_z(v: bool) -> jni::jvalue {
    jni::jvalue {
        z: if v { 1 } else { 0 },
    }
}

pub(crate) fn jv_f(v: f32) -> jni::jvalue {
    jni::jvalue { f: v }
}

pub(crate) fn jv_d(v: f64) -> jni::jvalue {
    jni::jvalue { d: v }
}

pub(crate) fn jv_o(v: jni::jobject) -> jni::jvalue {
    jni::jvalue { l: v }
}

/// Describe + clear any pending exception; returns true when one was pending.
pub(crate) fn swallow(env: &JniEnv, where_: &str) -> bool {
    if env.exception_check() {
        eprintln!("[c-LoD] JNI exception in {where_}:");
        env.exception_describe();
        env.exception_clear();
        true
    } else {
        false
    }
}

/// Read a Java string object into a Rust String.
pub(crate) fn jstring_to_string(env: &JniEnv, s: jni::jstring) -> Option<String> {
    if s.is_null() {
        return None;
    }
    env.get_string_utf(s)
}

/// Create a new Java string (Object for jvalue use).
pub(crate) fn new_jstr(env: &JniEnv, s: &str) -> Option<jni::jobject> {
    env.new_string_utf(s).map(|j| j as jni::jobject)
}

/// Get a method id with exception cleanup + typed error.
pub(crate) fn method(
    env: &JniEnv,
    cls: jni::jclass,
    name: &str,
    sig: &str,
) -> Result<jni::jmethodID, String> {
    env.get_method_id(cls, name, sig).ok_or_else(|| {
        swallow(env, &format!("get_method_id {name}{sig}"));
        format!("method not found: {name}{sig}")
    })
}

pub(crate) fn static_method(
    env: &JniEnv,
    cls: jni::jclass,
    name: &str,
    sig: &str,
) -> Result<jni::jmethodID, String> {
    env.get_static_method_id(cls, name, sig).ok_or_else(|| {
        swallow(env, &format!("get_static_method_id {name}{sig}"));
        format!("static method not found: {name}{sig}")
    })
}

pub(crate) fn field(
    env: &JniEnv,
    cls: jni::jclass,
    name: &str,
    sig: &str,
) -> Result<jni::jfieldID, String> {
    env.get_field_id(cls, name, sig).ok_or_else(|| {
        swallow(env, &format!("get_field_id {name}:{sig}"));
        format!("field not found: {name}:{sig}")
    })
}

pub(crate) fn static_field(
    env: &JniEnv,
    cls: jni::jclass,
    name: &str,
    sig: &str,
) -> Result<jni::jfieldID, String> {
    env.get_static_field_id(cls, name, sig).ok_or_else(|| {
        swallow(env, &format!("get_static_field_id {name}:{sig}"));
        format!("static field not found: {name}:{sig}")
    })
}

/// Object-returning instance call with exception cleanup.
pub(crate) fn call_obj(
    env: &JniEnv,
    obj: jni::jobject,
    mid: jni::jmethodID,
    args: &[jni::jvalue],
    where_: &str,
) -> Option<jni::jobject> {
    let r = env.call_object_method(obj, mid, args);
    if swallow(env, where_) {
        if !r.is_null() {
            env.delete_local_ref(r);
        }
        return None;
    }
    if r.is_null() {
        None
    } else {
        Some(r)
    }
}

/// int-returning instance call with exception cleanup (0 on error).
pub(crate) fn call_int(
    env: &JniEnv,
    obj: jni::jobject,
    mid: jni::jmethodID,
    args: &[jni::jvalue],
    where_: &str,
) -> i32 {
    let r = env.call_int_method(obj, mid, args);
    swallow(env, where_);
    r
}

/// boolean-returning instance call with exception cleanup.
pub(crate) fn call_bool(
    env: &JniEnv,
    obj: jni::jobject,
    mid: jni::jmethodID,
    args: &[jni::jvalue],
    where_: &str,
) -> bool {
    let r = env.call_boolean_method(obj, mid, args);
    swallow(env, where_);
    r
}

/// void instance call with exception cleanup.
pub(crate) fn call_void(
    env: &JniEnv,
    obj: jni::jobject,
    mid: jni::jmethodID,
    args: &[jni::jvalue],
    where_: &str,
) -> bool {
    env.call_void_method(obj, mid, args);
    !swallow(env, where_)
}

/// object-returning static call with exception cleanup.
pub(crate) fn call_static_obj(
    env: &JniEnv,
    cls: jni::jclass,
    mid: jni::jmethodID,
    args: &[jni::jvalue],
    where_: &str,
) -> Option<jni::jobject> {
    let r = env.call_static_object_method(cls, mid, args);
    if swallow(env, where_) {
        if !r.is_null() {
            env.delete_local_ref(r);
        }
        return None;
    }
    if r.is_null() {
        None
    } else {
        Some(r)
    }
}
