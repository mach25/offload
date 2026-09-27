//! macOS's power assertions, through IOKit (ADR-0077). The only `unsafe` in the workspace outside
//! `offload-ios`, allowed here and pinned here by a test in `lib.rs`.
//!
//! Two calls and their cleanup: `IOPMAssertionCreateWithName` with `PreventSystemSleep` (what
//! `caffeinate -s` holds), and `IOPMAssertionRelease`. The strings are CoreFoundation strings,
//! created and released around the call.
//!
//! Not `PreventUserIdleSystemSleep` (`caffeinate -i`), which this held first: it stops idle sleep
//! on a machine that is fully awake, and does nothing for one that is already asleep and only
//! dark-woken by a packet. The Mac mini held it for ten minutes, `pmset -g` said "sleep prevented
//! by offloadd", and `pmset -g log` showed it entering sleep twice in that time (session
//! ninety-four). `PreventSystemSleep` holds through a dark wake. It is honoured only on mains
//! power, and a lid, the Sleep menu or a low battery still sleep the machine.

#![allow(unsafe_code)]

use std::ffi::{c_char, c_void, CString};

type CFStringRef = *const c_void;
type IOPMAssertionID = u32;
type IOReturn = i32;

const K_CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;
const K_IOPM_ASSERTION_LEVEL_ON: u32 = 255;
const K_IO_RETURN_SUCCESS: IOReturn = 0;

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFStringCreateWithCString(
        alloc: *const c_void,
        c_str: *const c_char,
        encoding: u32,
    ) -> CFStringRef;
    fn CFRelease(cf: *const c_void);
}

#[link(name = "IOKit", kind = "framework")]
unsafe extern "C" {
    fn IOPMAssertionCreateWithName(
        assertion_type: CFStringRef,
        level: u32,
        name: CFStringRef,
        id: *mut IOPMAssertionID,
    ) -> IOReturn;
    fn IOPMAssertionRelease(id: IOPMAssertionID) -> IOReturn;
}

/// A CoreFoundation string, released when dropped.
struct CfString(CFStringRef);

impl CfString {
    fn new(text: &str) -> Result<CfString, String> {
        let c = CString::new(text).map_err(|_| "a NUL in the assertion's name".to_string())?;
        // SAFETY: `c` is a valid NUL-terminated UTF-8 string that outlives the call; a null
        // allocator means the default one. The result is checked for null before use.
        let s = unsafe {
            CFStringCreateWithCString(std::ptr::null(), c.as_ptr(), K_CF_STRING_ENCODING_UTF8)
        };
        if s.is_null() {
            Err("CoreFoundation could not make a string".into())
        } else {
            Ok(CfString(s))
        }
    }
}

impl Drop for CfString {
    fn drop(&mut self) {
        // SAFETY: `self.0` came from `CFStringCreateWithCString`, is non-null, and is released
        // exactly once, here.
        unsafe { CFRelease(self.0) }
    }
}

/// A held `PreventSystemSleep` assertion, released when dropped.
#[derive(Debug)]
pub(crate) struct Assertion(IOPMAssertionID);

impl Assertion {
    pub(crate) fn prevent_idle_sleep(reason: &str) -> Result<Assertion, String> {
        let kind = CfString::new("PreventSystemSleep")?;
        let name = CfString::new(reason)?;
        let mut id: IOPMAssertionID = 0;
        // SAFETY: both strings are live CoreFoundation strings for the duration of the call, and
        // `id` is a valid place for IOKit to write the new assertion's id.
        let result = unsafe {
            IOPMAssertionCreateWithName(kind.0, K_IOPM_ASSERTION_LEVEL_ON, name.0, &mut id)
        };
        if result == K_IO_RETURN_SUCCESS {
            Ok(Assertion(id))
        } else {
            Err(format!(
                "macOS refused the assertion (IOReturn {result:#x})"
            ))
        }
    }
}

impl Drop for Assertion {
    fn drop(&mut self) {
        // SAFETY: `self.0` is an assertion this process created and has not released; it is
        // released exactly once, here. A failure leaves nothing to do but let it go.
        let _ = unsafe { IOPMAssertionRelease(self.0) };
    }
}
