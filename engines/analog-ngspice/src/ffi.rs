//! Hand-written bindings to the ngspice shared library (`libngspice.so.0`).
//!
//! Every type and signature here is copied from the installed ngspice 42
//! header `/usr/include/ngspice/sharedspice.h` (package `libngspice0-dev`,
//! `42+ds-3build1`). `NG_BOOL` is C `bool` when the header is used by a caller,
//! which is ABI-compatible with Rust `bool`.
//!
//! The library is opened at runtime with `libloading` (no link-time
//! dependency). It is opened once per process and never unloaded, because
//! ngspice keeps process-global state and its background thread may still be
//! winding down when an engine is dropped.

use std::ffi::{c_char, c_int, c_short, c_void};
use std::sync::OnceLock;

/// `struct ngcomplex`.
#[repr(C)]
pub struct NgComplex {
    pub cx_real: f64,
    pub cx_imag: f64,
}

/// `struct vector_info` (returned by `ngGet_Vec_Info`).
#[repr(C)]
pub struct VectorInfo {
    pub v_name: *mut c_char,
    pub v_type: c_int,
    pub v_flags: c_short,
    pub v_realdata: *mut f64,
    pub v_compdata: *mut NgComplex,
    pub v_length: c_int,
}

/// `typedef int (SendChar)(char*, int, void*);`
pub type SendChar = unsafe extern "C" fn(*mut c_char, c_int, *mut c_void) -> c_int;
/// `typedef int (SendStat)(char*, int, void*);`
pub type SendStat = unsafe extern "C" fn(*mut c_char, c_int, *mut c_void) -> c_int;
/// `typedef int (ControlledExit)(int, NG_BOOL, NG_BOOL, int, void*);`
pub type ControlledExit = unsafe extern "C" fn(c_int, bool, bool, c_int, *mut c_void) -> c_int;
/// `typedef int (SendData)(pvecvaluesall, int, int, void*);` (unused: NULL)
pub type SendData = unsafe extern "C" fn(*mut c_void, c_int, c_int, *mut c_void) -> c_int;
/// `typedef int (SendInitData)(pvecinfoall, int, void*);` (unused: NULL)
pub type SendInitData = unsafe extern "C" fn(*mut c_void, c_int, *mut c_void) -> c_int;
/// `typedef int (BGThreadRunning)(NG_BOOL, int, void*);`
pub type BgThreadRunning = unsafe extern "C" fn(bool, c_int, *mut c_void) -> c_int;
/// `typedef int (GetVSRCData)(double*, double, char*, int, void*);`
pub type GetVsrcData =
    unsafe extern "C" fn(*mut f64, f64, *mut c_char, c_int, *mut c_void) -> c_int;
/// `typedef int (GetISRCData)(double*, double, char*, int, void*);`
pub type GetIsrcData =
    unsafe extern "C" fn(*mut f64, f64, *mut c_char, c_int, *mut c_void) -> c_int;
/// `typedef int (GetSyncData)(double, double*, double, int, int, int, void*);`
/// Arguments: actual time, *delta, old delta, redostep, ident, location, user.
pub type GetSyncData =
    unsafe extern "C" fn(f64, *mut f64, f64, c_int, c_int, c_int, *mut c_void) -> c_int;

type FnInit = unsafe extern "C" fn(
    Option<SendChar>,
    Option<SendStat>,
    Option<ControlledExit>,
    Option<SendData>,
    Option<SendInitData>,
    Option<BgThreadRunning>,
    *mut c_void,
) -> c_int;
type FnInitSync = unsafe extern "C" fn(
    Option<GetVsrcData>,
    Option<GetIsrcData>,
    Option<GetSyncData>,
    *mut c_int,
    *mut c_void,
) -> c_int;
type FnCommand = unsafe extern "C" fn(*mut c_char) -> c_int;
type FnGetVecInfo = unsafe extern "C" fn(*mut c_char) -> *mut VectorInfo;
type FnCirc = unsafe extern "C" fn(*mut *mut c_char) -> c_int;
type FnCurPlot = unsafe extern "C" fn() -> *mut c_char;
type FnRunning = unsafe extern "C" fn() -> bool;
type FnSetBkpt = unsafe extern "C" fn(f64) -> bool;

/// Resolved entry points of `libngspice.so.0`.
pub struct Lib {
    _lib: libloading::Library,
    pub init: FnInit,
    pub init_sync: FnInitSync,
    pub command: FnCommand,
    pub get_vec_info: FnGetVecInfo,
    pub circ: FnCirc,
    pub cur_plot: FnCurPlot,
    pub running: FnRunning,
    pub set_bkpt: FnSetBkpt,
}

// SAFETY: the struct only holds the library handle and plain function
// pointers; synchronization of the calls is the caller's job (see lib.rs).
unsafe impl Send for Lib {}
unsafe impl Sync for Lib {}

/// Default library name; override with the `BB_LIBNGSPICE` environment
/// variable (a file name or an absolute path).
pub const DEFAULT_LIB: &str = "libngspice.so.0";

impl Lib {
    fn open() -> Result<Lib, String> {
        let name = std::env::var("BB_LIBNGSPICE").unwrap_or_else(|_| DEFAULT_LIB.to_string());
        // SAFETY: loading ngspice runs its (trivial) ELF constructors only.
        let lib = unsafe { libloading::Library::new(&name) }
            .map_err(|e| format!("cannot load {name}: {e}"))?;
        macro_rules! sym {
            ($name:literal, $ty:ty) => {{
                // SAFETY: the type matches the prototype in sharedspice.h.
                let s: libloading::Symbol<$ty> = unsafe { lib.get($name) }
                    .map_err(|e| format!("{name}: missing symbol: {e}"))?;
                *s
            }};
        }
        let init = sym!(b"ngSpice_Init\0", FnInit);
        let init_sync = sym!(b"ngSpice_Init_Sync\0", FnInitSync);
        let command = sym!(b"ngSpice_Command\0", FnCommand);
        let get_vec_info = sym!(b"ngGet_Vec_Info\0", FnGetVecInfo);
        let circ = sym!(b"ngSpice_Circ\0", FnCirc);
        let cur_plot = sym!(b"ngSpice_CurPlot\0", FnCurPlot);
        let running = sym!(b"ngSpice_running\0", FnRunning);
        let set_bkpt = sym!(b"ngSpice_SetBkpt\0", FnSetBkpt);
        Ok(Lib {
            _lib: lib,
            init,
            init_sync,
            command,
            get_vec_info,
            circ,
            cur_plot,
            running,
            set_bkpt,
        })
    }
}

static LIB: OnceLock<Result<Lib, String>> = OnceLock::new();

/// The process-wide library, opened (and `init` run through `on_first_open`)
/// on first use.
pub fn lib(on_first_open: impl FnOnce(&Lib) -> Result<(), String>) -> Result<&'static Lib, String> {
    LIB.get_or_init(|| {
        let l = Lib::open()?;
        on_first_open(&l)?;
        Ok(l)
    })
    .as_ref()
    .map_err(Clone::clone)
}
